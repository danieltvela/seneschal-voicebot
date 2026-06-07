use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::config::Config;

use super::client::StreamToken;
use super::provider::LlmProvider;
use super::session::Message;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

/// Shared llama.cpp backend — `LlamaBackend::init()` is once-per-process and
/// its `Drop` calls `llama_backend_free()`, so manual construction is unsound.
static LLAMA_BACKEND: Mutex<Option<Arc<LlamaBackend>>> = Mutex::new(None);

/// Local LLM provider backed by `llama-cpp-2`.
///
/// Loads a GGUF model in-process and runs inference locally.
/// Each generation call creates a fresh context, so the KV-cache is not
/// reused across turns. This is a known limitation that trades efficiency
/// for implementation simplicity.
pub struct LlamaCppLlmProvider {
    backend: Arc<LlamaBackend>,
    model: Arc<LlamaModel>,
    n_ctx: u32,
    n_threads: u32,
    temperature: f32,
}

// LlamaModel uses raw pointers internally and is !Send by default.
// We wrap it in Arc and promise to only access it from one thread at a time
// (via Mutex when needed, or by creating fresh contexts per call).
unsafe impl Send for LlamaCppLlmProvider {}
unsafe impl Sync for LlamaCppLlmProvider {}

impl LlamaCppLlmProvider {
    pub fn new(config: &Config) -> Result<Self> {
        let model_path = config.llama_model_path.as_ref().ok_or_else(|| {
            anyhow::anyhow!("LLAMA_MODEL_PATH is required when LLM_PROVIDER=llama-cpp")
        })?;

        if !Path::new(model_path).exists() {
            bail!("Model file not found: {}", model_path);
        }

        info!(target: "llm", "Loading local model from {}", model_path);

        let backend = {
            let mut guard = LLAMA_BACKEND.lock().unwrap();
            if let Some(backend) = guard.as_ref() {
                backend.clone()
            } else {
                let backend = LlamaBackend::init()
                    .map_err(|e| anyhow::anyhow!("Failed to initialize llama backend: {e}"))?;
                let arc = Arc::new(backend);
                *guard = Some(arc.clone());
                arc
            }
        };

        let model_params = LlamaModelParams::default().with_n_gpu_layers(config.llama_n_gpu_layers);

        let model = LlamaModel::load_from_file(&backend, Path::new(model_path), &model_params)?;

        info!(
            target: "llm",
            "Model loaded: {} (context size: {}, GPU layers: {})",
            model_path,
            config.llama_context_size,
            config.llama_n_gpu_layers,
        );

        Ok(Self {
            backend,
            model: Arc::new(model),
            n_ctx: config.llama_context_size,
            n_threads: config.llama_n_threads,
            temperature: config.llm_temperature,
        })
    }

    /// Build a formatted prompt from messages using the model's built-in chat template.
    /// Falls back to ChatML when the model template is missing or not recognised by
    /// llama.cpp (ApplyChatTemplateError::FfiError(-1)).
    fn build_prompt(&self, messages: &[serde_json::Value]) -> Result<String> {
        let chat_messages: Vec<LlamaChatMessage> = messages
            .iter()
            .filter_map(|m| {
                let role = m["role"].as_str()?;
                let content = m["content"].as_str()?;
                LlamaChatMessage::new(role.to_string(), content.to_string()).ok()
            })
            .collect();

        let template_result = match self.model.chat_template(None) {
            Ok(t) => Ok(t),
            Err(llama_cpp_2::ChatTemplateError::MissingTemplate) => {
                LlamaChatTemplate::new("chatml")
                    .map_err(|e| anyhow::anyhow!("Invalid fallback template: {e}"))
            }
            Err(e) => Err(e.into()),
        };
        let template = template_result?;

        match self
            .model
            .apply_chat_template(&template, &chat_messages, true)
        {
            Ok(prompt) => Ok(prompt),
            Err(llama_cpp_2::ApplyChatTemplateError::FfiError(-1)) => {
                // Template not recognised by llama.cpp; fall back to ChatML.
                let fallback = LlamaChatTemplate::new("chatml")
                    .map_err(|e| anyhow::anyhow!("Invalid fallback template: {e}"))?;
                self.model
                    .apply_chat_template(&fallback, &chat_messages, true)
                    .map_err(|e| anyhow::anyhow!("Failed to apply fallback chat template: {e}"))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Run inference and return the full generated text.
    ///
    /// This is the core synchronous routine that must run inside `spawn_blocking`.
    fn generate_sync(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(std::num::NonZeroU32::new(self.n_ctx).unwrap()));

        let mut ctx = self.model.new_context(&self.backend, ctx_params)?;

        // Tokenize prompt
        let prompt_tokens = self.model.str_to_token(prompt, AddBos::Always)?;

        // Build batch
        let mut batch = LlamaBatch::new(self.n_ctx as usize, 1);
        let last_idx = (prompt_tokens.len().saturating_sub(1)) as i32;
        for (i, token) in prompt_tokens.iter().enumerate() {
            let is_last = i as i32 == last_idx;
            batch.add(*token, i as i32, &[0], is_last)?;
        }

        // Decode prompt
        ctx.decode(&mut batch)?;

        // Sampler
        let mut sampler = if self.temperature <= 0.0 {
            LlamaSampler::greedy()
        } else {
            LlamaSampler::chain_simple([
                LlamaSampler::temp(self.temperature),
                LlamaSampler::top_k(40),
                LlamaSampler::top_p(0.90, 1),
                LlamaSampler::dist(42),
            ])
        };

        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut n_cur = batch.n_tokens();
        let mut output = String::new();

        for _ in 0..max_tokens {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);

            if self.model.is_eog_token(token) {
                break;
            }

            sampler.accept(token);

            let piece = self.model.token_to_piece(token, &mut decoder, true, None)?;
            output.push_str(&piece);

            batch.clear();
            batch.add(token, n_cur, &[0], true)?;
            n_cur += 1;

            ctx.decode(&mut batch)?;
        }

        Ok(output)
    }
}

#[async_trait]
impl LlmProvider for LlamaCppLlmProvider {
    fn provider_name(&self) -> &'static str {
        "llama-cpp"
    }

    async fn stream(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
    ) -> Result<(mpsc::Receiver<StreamToken>, tokio::task::JoinHandle<()>)> {
        let prompt = self.build_prompt(messages)?;
        let max_tokens = 1024u32; // TODO: use config or parameter
        let model = Arc::clone(&self.model);
        let backend = Arc::clone(&self.backend);
        let n_ctx = self.n_ctx;
        let temperature = self.temperature;

        let (tx, rx) = mpsc::channel::<StreamToken>(256);

        let has_tools = !tools.is_empty();

        let handle = tokio::task::spawn_blocking(move || {
            let result = stream_generate(
                model,
                backend,
                n_ctx,
                temperature,
                &prompt,
                max_tokens,
                has_tools,
                tx.clone(),
            );
            if let Err(e) = result {
                error!(target: "llm", "Local generation error: {}", e);
                let _ = tx.blocking_send(StreamToken::Content(format!("\n[Error: {}]", e)));
            }
        });

        Ok((rx, handle))
    }

    async fn complete(&self, messages: &[Message]) -> Result<String> {
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect();

        let prompt = self.build_prompt(&messages_json)?;
        let model = Arc::clone(&self.model);
        let backend = Arc::clone(&self.backend);
        let n_ctx = self.n_ctx;
        let temperature = self.temperature;
        let max_tokens = 512u32;

        tokio::task::spawn_blocking(move || {
            generate_sync_static(model, backend, n_ctx, temperature, &prompt, max_tokens)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Join error: {}", e))?
    }

    async fn complete_short(&self, messages: &[Message]) -> Result<String> {
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect();

        let prompt = self.build_prompt(&messages_json)?;
        let model = Arc::clone(&self.model);
        let backend = Arc::clone(&self.backend);
        let n_ctx = self.n_ctx;
        let temperature = self.temperature;
        let max_tokens = 256u32;

        tokio::task::spawn_blocking(move || {
            generate_sync_static(model, backend, n_ctx, temperature, &prompt, max_tokens)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Join error: {}", e))?
    }

    async fn complete_multimodal(
        &self,
        _image_data_url: &str,
        _text_prompt: &str,
    ) -> Result<String> {
        bail!("Multimodal completion is not supported by the llama-cpp provider yet")
    }
}

/// Synchronous generation helper used by `stream`.
///
/// When `has_tools` is true the entire response is accumulated and parsed for
/// Gemma 4 tool calls after generation finishes. This avoids the "derailment"
/// problem where the model reconsiders a tool call mid-stream. When false,
/// tokens are streamed incrementally as they are generated.
fn stream_generate(
    model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    n_ctx: u32,
    temperature: f32,
    prompt: &str,
    max_tokens: u32,
    has_tools: bool,
    tx: mpsc::Sender<StreamToken>,
) -> Result<()> {
    let ctx_params =
        LlamaContextParams::default().with_n_ctx(Some(std::num::NonZeroU32::new(n_ctx).unwrap()));

    let mut ctx = model.new_context(&backend, ctx_params)?;

    let prompt_tokens = model.str_to_token(prompt, AddBos::Always)?;

    let mut batch = LlamaBatch::new(n_ctx as usize, 1);
    let last_idx = (prompt_tokens.len().saturating_sub(1)) as i32;
    for (i, token) in prompt_tokens.iter().enumerate() {
        let is_last = i as i32 == last_idx;
        batch.add(*token, i as i32, &[0], is_last)?;
    }

    ctx.decode(&mut batch)?;

    let mut sampler = if temperature <= 0.0 {
        LlamaSampler::greedy()
    } else {
        LlamaSampler::chain_simple([
            LlamaSampler::temp(temperature),
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.90, 1),
            LlamaSampler::dist(42),
        ])
    };

    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut n_cur = batch.n_tokens();
    let mut accumulated = String::new();

    for _ in 0..max_tokens {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);

        if model.is_eog_token(token) {
            break;
        }

        sampler.accept(token);

        let piece = model.token_to_piece(token, &mut decoder, true, None)?;

        if has_tools {
            accumulated.push_str(&piece);
        } else if tx.blocking_send(StreamToken::Content(piece)).is_err() {
            break; // receiver dropped
        }

        batch.clear();
        batch.add(token, n_cur, &[0], true)?;
        n_cur += 1;

        ctx.decode(&mut batch)?;
    }

    // Post-generation parsing for tool calls (Gemma 4 safe mode).
    if has_tools && !accumulated.is_empty() {
        let cleaned = super::gemma4_parser::Gemma4ToolCallParser::strip_reasoning(&accumulated);
        if let Some((name, args)) = super::gemma4_parser::Gemma4ToolCallParser::parse(&cleaned) {
            // Emit everything before the tool call as content, then the tool call.
            if let Some(pos) = cleaned.find("<|tool_call>") {
                let before = cleaned[..pos].trim();
                if !before.is_empty() {
                    let _ = tx.blocking_send(StreamToken::Content(before.to_string()));
                }
            }
            let _ = tx.blocking_send(StreamToken::ToolCall { name, args });
        } else {
            let _ = tx.blocking_send(StreamToken::Content(cleaned));
        }
    }

    Ok(())
}

/// Synchronous generation helper used by `complete` and `complete_short`.
fn generate_sync_static(
    model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    n_ctx: u32,
    temperature: f32,
    prompt: &str,
    max_tokens: u32,
) -> Result<String> {
    let ctx_params =
        LlamaContextParams::default().with_n_ctx(Some(std::num::NonZeroU32::new(n_ctx).unwrap()));

    let mut ctx = model.new_context(&backend, ctx_params)?;

    let prompt_tokens = model.str_to_token(prompt, AddBos::Always)?;

    let mut batch = LlamaBatch::new(n_ctx as usize, 1);
    let last_idx = (prompt_tokens.len().saturating_sub(1)) as i32;
    for (i, token) in prompt_tokens.iter().enumerate() {
        let is_last = i as i32 == last_idx;
        batch.add(*token, i as i32, &[0], is_last)?;
    }

    ctx.decode(&mut batch)?;

    let mut sampler = if temperature <= 0.0 {
        LlamaSampler::greedy()
    } else {
        LlamaSampler::chain_simple([
            LlamaSampler::temp(temperature),
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.90, 1),
            LlamaSampler::dist(42),
        ])
    };

    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut n_cur = batch.n_tokens();
    let mut output = String::new();

    for _ in 0..max_tokens {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);

        if model.is_eog_token(token) {
            break;
        }

        sampler.accept(token);

        let piece = model.token_to_piece(token, &mut decoder, true, None)?;
        output.push_str(&piece);

        batch.clear();
        batch.add(token, n_cur, &[0], true)?;
        n_cur += 1;

        ctx.decode(&mut batch)?;
    }

    Ok(output)
}

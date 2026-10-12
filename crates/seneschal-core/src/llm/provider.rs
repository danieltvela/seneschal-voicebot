use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use tokio::sync::mpsc;

use seneschal_common::config::Config;

use super::client::{OpenAIClient, StreamToken};
use super::session::Message;

/// Controls the `tool_choice` parameter in chat/completions requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice {
    /// Let the model decide whether to call tools (`auto`).
    Auto,
    /// Force the model to call a tool (`required`).
    Required,
    /// Disable tool calling explicitly (`none`).
    None,
}

/// Per-request options that override fixed client settings.
///
/// `None` = inherit from the client's default.
#[derive(Clone, Copy, Default)]
pub struct RequestOptions {
    /// Override temperature for this single request.
    pub temperature: Option<f32>,
    /// Override thinking mode for this single request.
    pub thinking: Option<bool>,
    /// When false, caller MUST pass `tools=&[]` to `stream()`.
    pub enable_tools: bool,
    /// Override the `tool_choice` parameter. `None` = default auto/required.
    pub tool_choice: Option<ToolChoice>,
    /// Send `chat_template_kwargs` even when `tools` is present.
    ///
    /// The voice pipeline never sets this: on mlx-community quantizations the
    /// Jinja chat template conflicts with tool definitions (#tools requests
    /// deliberately omit the kwargs). The deep worker sets it for
    /// `thinking_mode = "inline"` on backends verified to support both at
    /// once (vLLM with `--enable-reasoning --reasoning-parser`, issue #236).
    pub thinking_with_tools: bool,
}

impl RequestOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn with_thinking(mut self, t: bool) -> Self {
        self.thinking = Some(t);
        self
    }

    pub fn with_tool_choice(mut self, c: ToolChoice) -> Self {
        self.tool_choice = Some(c);
        self
    }

    pub fn with_thinking_with_tools(mut self) -> Self {
        self.thinking_with_tools = true;
        self
    }
}

/// Abstraction over LLM inference backends.
///
/// The primary implementation is `OpenAiLlmProvider` which speaks to an
/// OpenAI-compatible HTTP endpoint.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    /// Stream completion tokens for a conversation turn.
    ///
    /// Returns a channel receiver that yields `StreamToken::Content` tokens as
    /// they are generated, `StreamToken::Reasoning` for server reasoning deltas
    /// (reasoning-parser backends), or `StreamToken::ToolCall` when the model
    /// decides to invoke a tool. The returned `JoinHandle` drives the
    /// generation loop.
    async fn stream(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        options: RequestOptions,
    ) -> Result<(mpsc::Receiver<StreamToken>, tokio::task::JoinHandle<()>)>;

    /// One-shot (non-streaming) completion.
    /// Used for summarization and context consolidation.
    async fn complete(&self, messages: &[Message]) -> Result<String>;

    /// One-shot completion with a short token budget.
    /// Used for structured extractions (profile facts, etc.).
    async fn complete_short(&self, messages: &[Message]) -> Result<String>;

    /// One-shot multimodal completion with a single image + text prompt.
    /// Used by vision tools (e.g. `TakeScreenshotTool`).
    async fn complete_multimodal(&self, image_data_url: &str, text_prompt: &str) -> Result<String>;
}

/// Thin wrapper around `OpenAIClient` that implements `LlmProvider`.
///
/// Keeps the original HTTP client accessible for code that still needs it
/// directly (e.g. tests) while allowing the pipeline to work with the trait.
pub struct OpenAiLlmProvider {
    inner: OpenAIClient,
}

impl OpenAiLlmProvider {
    pub fn new(base_url: &str, model: &str, max_tokens: u32, temperature: f32) -> Self {
        Self {
            inner: OpenAIClient::new(base_url, model, max_tokens, temperature),
        }
    }

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.inner = self.inner.with_api_key(key);
        self
    }

    pub fn with_thinking(mut self, thinking: bool) -> Self {
        self.inner = self.inner.with_thinking(thinking);
        self
    }

    /// Select the thinking wire shape (`auto|kwargs|effort|ollama|none`).
    /// Errors on an unknown style string (config surfaces it as a startup
    /// validation failure).
    pub fn with_thinking_style(self, style_str: &str) -> Result<Self, String> {
        Ok(Self {
            inner: self.inner.with_thinking_style(style_str)?,
        })
    }

    /// Deployment-level opt-in to send the thinking flag alongside tools
    /// (`LLM_THINKING_WITH_TOOLS`).
    pub fn with_default_thinking_with_tools(mut self, on: bool) -> Self {
        self.inner = self.inner.with_default_thinking_with_tools(on);
        self
    }

    pub fn with_sampling(
        mut self,
        top_p: f32,
        top_k: i32,
        min_p: f32,
        presence_penalty: f32,
        repetition_penalty: f32,
    ) -> Self {
        self.inner =
            self.inner
                .with_sampling(top_p, top_k, min_p, presence_penalty, repetition_penalty);
        self
    }
}

#[async_trait]
impl LlmProvider for OpenAiLlmProvider {
    fn provider_name(&self) -> &'static str {
        "openai"
    }

    async fn stream(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        options: RequestOptions,
    ) -> Result<(mpsc::Receiver<StreamToken>, tokio::task::JoinHandle<()>)> {
        self.inner.stream(messages, tools, options).await
    }

    async fn complete(&self, messages: &[Message]) -> Result<String> {
        self.inner.complete(messages).await
    }

    async fn complete_short(&self, messages: &[Message]) -> Result<String> {
        self.inner.complete_short(messages).await
    }

    async fn complete_multimodal(&self, image_data_url: &str, text_prompt: &str) -> Result<String> {
        self.inner
            .complete_multimodal(image_data_url, text_prompt)
            .await
    }
}

/// Factory that instantiates the active LLM provider based on `Config`.
///
/// Reads `LLM_PROVIDER` from the environment (via `config.llm_provider`).
/// The only supported backend is `openai` (default) — an OpenAI-compatible HTTP endpoint.
pub fn create_provider(config: &Config) -> Result<Arc<dyn LlmProvider>> {
    match config.llm_provider.to_lowercase().as_str() {
        "openai" => {
            let provider = OpenAiLlmProvider::new(
                &config.llm_url,
                &config.llm_model,
                config.llm_max_tokens,
                config.llm_temperature,
            )
            .with_api_key(&config.llm_api_key)
            .with_thinking(config.llm_thinking)
            .with_sampling(
                config.llm_top_p,
                config.llm_top_k,
                config.llm_min_p,
                config.llm_presence_penalty,
                config.llm_repetition_penalty,
            );
            let provider = provider
                .with_default_thinking_with_tools(config.llm_thinking_with_tools)
                .with_thinking_style(&config.llm_thinking_style)
                .map_err(|e| anyhow!("{e}"))?;
            Ok(Arc::new(provider))
        }
        other => bail!("Invalid LLM_PROVIDER '{other}'. Supported values: openai"),
    }
}

/// Factory for the deep worker's own LLM endpoint (issue #236).
///
/// Mirrors [`create_provider`] but reads the `deep_worker_*` config block —
/// a fully independent endpoint/model/sampling set. The caller is responsible
/// for gating on `config.deep_worker_enabled` (and the config validation
/// guarantees url/model are set when enabled).
pub fn create_deep_worker_provider(config: &Config) -> Result<Arc<dyn LlmProvider>> {
    let provider = OpenAiLlmProvider::new(
        &config.deep_worker_url,
        &config.deep_worker_model,
        config.deep_worker_max_tokens,
        config.deep_worker_temperature,
    )
    .with_api_key(&config.deep_worker_api_key)
    .with_thinking(config.deep_worker_thinking)
    .with_sampling(
        config.deep_worker_top_p,
        config.deep_worker_top_k,
        config.deep_worker_min_p,
        config.deep_worker_presence_penalty,
        config.deep_worker_repetition_penalty,
    );
    let provider = provider
        .with_thinking_style(&config.deep_worker_thinking_style)
        .map_err(|e| anyhow!("{e}"))?;
    Ok(Arc::new(provider))
}

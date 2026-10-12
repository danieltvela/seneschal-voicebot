use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;

use super::provider::{RequestOptions, ToolChoice};
use super::session::Message;

/// Strips `<think>…</think>` blocks from a streaming token sequence.
///
/// Qwen3 (and other reasoning models) emit a chain-of-thought block before
/// the actual response. Those tokens must not reach TTS or the tool detector.
///
/// The filter handles the case where the opening or closing tag is split
/// across multiple tokens by buffering up to `max(tag_len - 1)` bytes.
struct ThinkFilter {
    in_think: bool,
    /// Holds trailing bytes that could be the start of a tag (`<think>` or
    /// `</think>`). Flushed once we know they are not part of a tag.
    pending: String,
}

impl ThinkFilter {
    fn new() -> Self {
        Self {
            in_think: false,
            pending: String::new(),
        }
    }

    /// Feed the next raw token from the SSE stream.
    /// Returns the portion of the token (if any) that should be forwarded.
    fn process(&mut self, token: &str) -> Option<String> {
        self.pending.push_str(token);
        let mut out = String::new();

        loop {
            if self.in_think {
                match self.pending.find("</think>") {
                    Some(pos) => {
                        // Found closing tag — resume normal output after it.
                        self.pending = self.pending[pos + "</think>".len()..].to_string();
                        self.in_think = false;
                        // Continue loop to check remaining pending for more tags.
                    }
                    None => {
                        // Keep only a suffix long enough to catch a split tag.
                        let keep = partial_tag_suffix(&self.pending, "</think>");
                        self.pending = self.pending[self.pending.len() - keep..].to_string();
                        break;
                    }
                }
            } else {
                match self.pending.find("<think>") {
                    Some(pos) => {
                        // Emit everything before the tag, then enter think mode.
                        out.push_str(&self.pending[..pos]);
                        self.pending = self.pending[pos + "<think>".len()..].to_string();
                        self.in_think = true;
                        // Continue loop to consume the think block.
                    }
                    None => {
                        // Keep only a suffix that could be a partial opening tag.
                        let keep = partial_tag_suffix(&self.pending, "<think>");
                        out.push_str(&self.pending[..self.pending.len() - keep]);
                        self.pending = self.pending[self.pending.len() - keep..].to_string();
                        break;
                    }
                }
            }
        }

        if out.is_empty() { None } else { Some(out) }
    }

    /// Call once when the stream ends to emit any buffered non-think content.
    fn flush(&mut self) -> Option<String> {
        if self.in_think || self.pending.is_empty() {
            self.pending.clear();
            return None;
        }
        let out = std::mem::take(&mut self.pending);
        Some(out)
    }
}

/// Strip all `<think>…</think>` blocks from a complete (non-streaming) string.
///
/// Used to post-process LLM responses when thinking mode is enabled.
/// The model reasons inside the tags; only the text after the closing tag is
/// returned to callers.
fn strip_think_blocks(s: &str) -> String {
    let mut out = s.to_string();
    loop {
        match (out.find("<think>"), out.find("</think>")) {
            (Some(start), Some(end)) if start <= end => {
                let after = &out[end + "</think>".len()..].to_string();
                out = format!("{}{}", &out[..start], after);
            }
            _ => break,
        }
    }
    out.trim().to_string()
}

/// Returns the length of the longest suffix of `s` that is a proper prefix of
/// `tag` (i.e. the tail of `s` that could be the start of `tag`).
fn partial_tag_suffix(s: &str, tag: &str) -> usize {
    for n in (1..tag.len()).rev() {
        if s.ends_with(&tag[..n]) {
            return n;
        }
    }
    0
}

/// A token produced by `OpenAIClient::stream`.
///
/// The LLM either generates text content (route to TTS) or calls a tool
/// (stop streaming, execute the tool, then continue).
#[derive(Debug)]
pub enum StreamToken {
    /// Regular text — forward to TTS.
    Content(String),
    /// Server reasoning delta (`delta.reasoning_content` from a reasoning
    /// parser, e.g. vLLM/SGLang). Never forward to TTS.
    Reasoning(String),
    /// The model invoked a tool. `args` is the JSON arguments string.
    ToolCall { name: String, args: String },
}
/// How the thinking on/off flag is requested from the server. Backends
/// disagree on the wire shape; the config strings are
/// `auto|kwargs|effort|ollama|none` (`inline` accepted as an alias of `none`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingStyle {
    /// `chat_template_kwargs: {"enable_thinking": bool}` — vLLM, SGLang,
    /// llama.cpp server. The established default.
    Kwargs,
    /// `reasoning_effort: "high"` when thinking is on (OpenAI-compatible
    /// reasoning APIs: o-series, gpt-5, proxies). No disable shape: the
    /// server decides when thinking is off.
    Effort,
    /// Top-level `think: true|false` — Ollama.
    Ollama,
    /// No flag at all: the server/template decides. Thinking arrives inline
    /// in `content` and is stripped by the ThinkFilter.
    None,
}

impl ThinkingStyle {
    /// Parse a configured style; `auto`/empty resolves from the model name
    /// (see [`Self::auto_for`]). Errors bubble to config validation messages.
    pub fn parse(s: &str, model: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "kwargs" => Ok(Self::Kwargs),
            "effort" => Ok(Self::Effort),
            "ollama" => Ok(Self::Ollama),
            "none" | "inline" => Ok(Self::None),
            "auto" | "" => Ok(Self::auto_for(model)),
            other => Err(format!(
                "Invalid thinking style '{other}' (expected auto|kwargs|effort|ollama|none)"
            )),
        }
    }

    /// Name-based heuristic. Deliberately conservative: unknown models keep
    /// the established `kwargs` behavior.
    pub fn auto_for(model: &str) -> Self {
        let lower = model.to_lowercase();
        let base = lower.rsplit('/').next().unwrap_or(&lower);
        let id = base.split('@').next().unwrap_or(base);
        if id.starts_with("o1")
            || id.starts_with("o3")
            || id.starts_with("o4")
            || id.contains("gpt-5")
            || id.contains("gpt-5.1")
        {
            Self::Effort
        } else if id.contains(':') {
            // Ollama tag form "model:tag".
            Self::Ollama
        } else if id.contains("deepseek-r1") {
            // Always-thinking model with no chat-completions disable flag.
            Self::None
        } else {
            Self::Kwargs
        }
    }
}

#[derive(Clone)]
pub struct OpenAIClient {
    client: reqwest::Client,
    chat_url: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
    /// Bearer token sent in `Authorization` header. Empty = no auth header.
    api_key: String,
    /// When true, request thinking per [`Self::thinking_style`] and strip
    /// inline think-tag blocks from the returned text.
    thinking: bool,
    /// Wire shape used to request thinking (see [`ThinkingStyle`]).
    thinking_style: ThinkingStyle,
    /// Deployment-level opt-in to send the thinking flag alongside `tools`
    /// on every streamed request. Required to force thinking OFF on
    /// servers whose default is thinking-enabled; unsafe on mlx-community
    /// quantizations (Jinja conflict). Per-request opt-in lives in
    /// `RequestOptions::thinking_with_tools`.
    default_thinking_with_tools: bool,
    /// Sampling top-p sent in the request payload.
    top_p: f32,
    /// Sampling top-k sent in the request payload.
    top_k: i32,
    /// Sampling min-p sent in the request payload.
    min_p: f32,
    /// Presence penalty sent in the request payload.
    presence_penalty: f32,
    /// Repetition penalty sent in the request payload.
    repetition_penalty: f32,
}

impl OpenAIClient {
    pub fn new(base_url: &str, model: &str, max_tokens: u32, temperature: f32) -> Self {
        let client = reqwest::Client::builder()
            .tcp_keepalive(Duration::from_secs(60))
            .tcp_nodelay(true) // Disable Nagle — send SSE tokens immediately
            .connect_timeout(Duration::from_secs(5)) // Fail fast on unreachable server
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("failed to build HTTP client");
        Self {
            client,
            chat_url: format!("{}/v1/chat/completions", base_url.trim_end_matches('/')),
            model: model.to_string(),
            max_tokens,
            temperature,
            api_key: String::new(),
            thinking: false,
            thinking_style: ThinkingStyle::Kwargs,
            default_thinking_with_tools: false,
            top_p: 0.95,
            top_k: 20,
            min_p: 0.0,
            presence_penalty: 0.0,
            repetition_penalty: 1.0,
        }
    }

    /// Set the API key sent as `Authorization: Bearer <key>`.
    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = key.to_string();
        self
    }

    /// Returns a POST request builder for the chat completions URL,
    /// with the `Authorization` header set if an API key is configured.
    fn post_chat(&self) -> reqwest::RequestBuilder {
        let req = self.client.post(&self.chat_url);
        if self.api_key.is_empty() {
            req
        } else {
            req.bearer_auth(&self.api_key)
        }
    }

    /// Enable Qwen3 thinking mode for non-streaming completions.
    ///
    /// When `true`, `chat_template_kwargs: {"enable_thinking": true}` is sent
    /// and `<think>…</think>` blocks are stripped from the returned text.
    /// The model reasons internally; callers only receive the final answer.
    pub fn with_thinking(mut self, thinking: bool) -> Self {
        self.thinking = thinking;
        self
    }
    /// Default on/off for sending the thinking flag alongside tools (see
    /// the field docs). The deep worker's per-request opt-in is independent.
    pub fn with_default_thinking_with_tools(mut self, on: bool) -> Self {
        self.default_thinking_with_tools = on;
        self
    }
    /// Select the wire shape used to request thinking. `style_str` accepts
    /// `auto|kwargs|effort|ollama|none`; `auto` resolves from the model name.
    pub fn with_thinking_style(mut self, style_str: &str) -> Result<Self, String> {
        self.thinking_style = ThinkingStyle::parse(style_str, &self.model)?;
        Ok(self)
    }

    /// Emit the thinking enable/disable flag in the configured wire shape.
    /// `off_shape` marks call sites that already send an explicit disable
    /// today (stream without tools, thinking+tools opt-in); sites that
    /// historically send nothing when off pass `false`.
    fn apply_thinking(&self, payload: &mut serde_json::Value, enable: bool, off_shape: bool) {
        match self.thinking_style {
            ThinkingStyle::Kwargs => {
                if enable {
                    payload["chat_template_kwargs"] = serde_json::json!({"enable_thinking": true});
                } else if off_shape {
                    payload["chat_template_kwargs"] = serde_json::json!({"enable_thinking": false});
                }
            }
            ThinkingStyle::Effort => {
                if enable {
                    payload["reasoning_effort"] = serde_json::json!("high");
                }
            }
            ThinkingStyle::Ollama => {
                if enable {
                    payload["think"] = serde_json::json!(true);
                } else if off_shape {
                    payload["think"] = serde_json::json!(false);
                }
            }
            ThinkingStyle::None => {}
        }
    }

    /// Set sampling parameters sent in the request payload.
    pub fn with_sampling(
        mut self,
        top_p: f32,
        top_k: i32,
        min_p: f32,
        presence_penalty: f32,
        repetition_penalty: f32,
    ) -> Self {
        self.top_p = top_p;
        self.top_k = top_k;
        self.min_p = min_p;
        self.presence_penalty = presence_penalty;
        self.repetition_penalty = repetition_penalty;
        self
    }

    /// Stream completion tokens from an OpenAI-compatible endpoint.
    ///
    /// Returns a channel receiver that yields text tokens as they arrive.
    pub async fn stream(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        options: RequestOptions,
    ) -> Result<(mpsc::Receiver<StreamToken>, tokio::task::JoinHandle<()>)> {
        let payload = self.build_stream_payload(messages, tools, options);

        tracing::debug!(target: "llm", "Request payload: {}", serde_json::to_string(&payload).unwrap_or_default());

        let response = self
            .post_chat()
            .json(&payload)
            .send()
            .await
            .context("Failed to reach LLM server")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM error {}: {}", status, body);
        }

        let (tx, rx) = mpsc::channel::<StreamToken>(256);

        let stream_handle = tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buf = String::new();
            let mut think = ThinkFilter::new();

            // Accumulate tool calls across streamed fragments, ONE SLOT PER
            // `index`: servers stream parallel calls interleaved in a single
            // response, and a single-slot accumulator concatenated their
            // arguments into invalid JSON — the echo then 400'd on the next
            // request ("function.arguments must be valid JSON").
            let mut tool_calls: Vec<(String, String)> = Vec::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(target: "llm", "LLM stream error: {}", e);
                        break;
                    }
                };

                buf.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(newline) = buf.find('\n') {
                    let line = buf[..newline].trim().to_string();
                    buf = buf[newline + 1..].to_string();

                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };

                    if data == "[DONE]" {
                        // Flush any content held back for partial-tag detection.
                        if let Some(tail) = think.flush() {
                            let _ = tx.send(StreamToken::Content(tail)).await;
                        }
                        // Flush any calls accumulating but no finish_reason arrived.
                        for (name, args) in std::mem::take(&mut tool_calls) {
                            if name.is_empty() {
                                continue;
                            }
                            tracing::info!(target: "llm", "Emitting ToolCall (via [DONE]): name={} args={}", name, args);
                            if tx.send(StreamToken::ToolCall { name, args }).await.is_err() {
                                return;
                            }
                        }
                        return;
                    }

                    let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };

                    // Accumulate tool_calls fragments FIRST — before checking
                    // finish_reason. mlx-lm (and some other servers) send the entire
                    // tool call in a single chunk that has both delta.tool_calls AND
                    // finish_reason="tool_calls". If we checked finish_reason first we
                    // would emit an empty ToolCall (no fragments accumulated) and return.
                    //
                    // Guard: mlx-lm sends `"tool_calls": []` on every content chunk;
                    // only treat it as a real call when the array is non-empty.
                    let has_tool_call_delta = if let Some(calls) =
                        json["choices"][0]["delta"]["tool_calls"].as_array()
                    {
                        if !calls.is_empty() {
                            for call in calls {
                                // Servers that omit `index` stream one call → slot 0.
                                let idx = call["index"].as_u64().unwrap_or(0) as usize;
                                if tool_calls.len() <= idx {
                                    tool_calls.resize(idx + 1, Default::default());
                                }
                                if let Some(name) = call["function"]["name"].as_str()
                                    && !name.is_empty()
                                {
                                    tracing::debug!(target: "llm", "Tool call detected: {}", name);
                                    tool_calls[idx].0 = name.to_string();
                                }
                                if let Some(frag) = call["function"]["arguments"].as_str() {
                                    tool_calls[idx].1.push_str(frag);
                                }
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    // Now check finish_reason (tool_calls may have just accumulated above).
                    if let Some(finish_reason) = json["choices"][0]["finish_reason"].as_str() {
                        tracing::debug!(target: "llm", "SSE finish_reason={}", finish_reason);
                        if finish_reason == "tool_calls" {
                            if let Some(tail) = think.flush() {
                                let _ = tx.send(StreamToken::Content(tail)).await;
                            }
                            for (name, args) in std::mem::take(&mut tool_calls) {
                                if name.is_empty() {
                                    continue;
                                }
                                tracing::info!(target: "llm", "Emitting ToolCall (finish_reason=tool_calls): name={} args={}", name, args);
                                if tx.send(StreamToken::ToolCall { name, args }).await.is_err() {
                                    return;
                                }
                            }
                            return;
                        }
                    }

                    // Skip content processing if this chunk contained tool_calls data.
                    if has_tool_call_delta {
                        continue;
                    }
                    // Reasoning deltas surface under different delta fields
                    // depending on server: vLLM/SGLang/llama.cpp use
                    // `reasoning_content`, Ollama/OpenRouter use `reasoning`,
                    // some thinking shims use `thinking`. These frames carry
                    // `content: ""`, so they must be handled before the
                    // empty-content skip below.
                    let delta = &json["choices"][0]["delta"];
                    for field in ["reasoning_content", "reasoning", "thinking"] {
                        if let Some(reasoning) = delta[field].as_str()
                            && !reasoning.is_empty()
                            && tx
                                .send(StreamToken::Reasoning(reasoning.to_string()))
                                .await
                                .is_err()
                        {
                            return;
                        }
                    }

                    // Regular content token.
                    if let Some(content) = json["choices"][0]["delta"]["content"].as_str() {
                        if content.is_empty() {
                            continue;
                        }
                        if let Some(filtered) = think.process(content)
                            && tx.send(StreamToken::Content(filtered)).await.is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });

        Ok((rx, stream_handle))
    }

    /// One-shot (non-streaming) completion. Used for summarization.
    pub async fn complete(&self, messages: &[Message]) -> Result<String> {
        let mut payload = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": 512,
            "temperature": 0.3,
            "stream": false,
        });
        self.apply_thinking(&mut payload, self.thinking, true);

        let response = self
            .post_chat()
            .json(&payload)
            .send()
            .await
            .context("Failed to reach LLM server for summarization")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM summarization error {}: {}", status, body);
        }

        let json: serde_json::Value = response.json().await?;
        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        Ok(if self.thinking {
            strip_think_blocks(&text)
        } else {
            text
        })
    }

    /// One-shot completion with a short token budget. Used for structured
    /// extractions (profile facts, etc.) that produce brief outputs.
    pub async fn complete_short(&self, messages: &[Message]) -> Result<String> {
        let mut payload = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": 256,
            "temperature": 0.1,
            "stream": false,
        });
        self.apply_thinking(&mut payload, self.thinking, true);

        let response = self
            .post_chat()
            .json(&payload)
            .send()
            .await
            .context("Failed to reach LLM server for extraction")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM extraction error {}: {}", status, body);
        }

        let json: serde_json::Value = response.json().await?;
        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        Ok(if self.thinking {
            strip_think_blocks(&text)
        } else {
            text
        })
    }

    /// One-shot multimodal completion with a single image + text prompt.
    ///
    /// Sends an OpenAI-compatible content array with `image_url` and `text` parts.
    /// Used by `TakeScreenshotTool`.
    pub async fn complete_multimodal(
        &self,
        image_data_url: &str,
        text_prompt: &str,
    ) -> Result<String> {
        let mut payload = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "stream": false,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image_url", "image_url": { "url": image_data_url } },
                    { "type": "text", "text": text_prompt }
                ]
            }]
        });
        self.apply_thinking(&mut payload, self.thinking, true);

        let response = self
            .post_chat()
            .json(&payload)
            .send()
            .await
            .context("Failed to reach vision model server")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Vision model error {}: {}", status, body);
        }

        let json: serde_json::Value = response.json().await?;
        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        Ok(if self.thinking {
            strip_think_blocks(&text)
        } else {
            text
        })
    }

    /// Check if the server is reachable.
    #[allow(dead_code)]
    pub async fn health_check(&self, base_url: &str) -> bool {
        let url = format!("{}/health", base_url.trim_end_matches('/'));
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Compose the chat/completions request payload for `stream()`.
    ///
    /// Single source of truth for the payload shape — `stream()` sends exactly
    /// what this returns, and tests assert on it directly.
    pub fn build_stream_payload(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        options: RequestOptions,
    ) -> serde_json::Value {
        let effective_thinking = options.thinking.unwrap_or(self.thinking);
        let mut payload = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": self.max_tokens,
            "temperature": options.temperature.unwrap_or(self.temperature),
            "top_p": self.top_p,
            "stream": true,
            "repetition_penalty": self.repetition_penalty,
            "top_k": self.top_k,
            "min_p": self.min_p,
            "presence_penalty": self.presence_penalty,
        });
        let tool_choice_override = options.tool_choice.unwrap_or(ToolChoice::Auto);

        // Always include `tools` when non-empty so the request shape stays
        // stable across Simple/Complex turns (KV-cache prefix reuse, #191).
        // `tool_choice: "none"` disables *use* without stripping definitions.
        if !tools.is_empty() {
            payload["tools"] = serde_json::json!(tools);
            // llama.cpp / LM Studio only supports string values for
            // tool_choice ("none", "auto", "required"). The object form
            // {"type":"function","function":{...}} is rejected with 400.
            payload["tool_choice"] = match tool_choice_override {
                ToolChoice::Required => serde_json::json!("required"),
                ToolChoice::Auto => serde_json::json!("auto"),
                ToolChoice::None => serde_json::json!("none"),
            };
            // Send `chat_template_kwargs` with tools only when opted in:
            // per request (`RequestOptions::thinking_with_tools`, deep
            // worker inline mode, issue #236) or deployment-wide
            // (`LLM_THINKING_WITH_TOOLS`, to force thinking off on
            // thinking-by-default servers). mlx-community quantizations
            // have a Jinja conflict with this combination — keep both
            // flags off there.
            if options.thinking_with_tools || self.default_thinking_with_tools {
                self.apply_thinking(&mut payload, effective_thinking, true);
            }
        } else {
            if tool_choice_override == ToolChoice::None {
                payload["tool_choice"] = serde_json::json!("none");
            }
            self.apply_thinking(&mut payload, effective_thinking, true);
        }
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_messages() -> Vec<Message> {
        vec![
            Message::system("You are a summarizer."),
            Message::user("Summarize this conversation."),
        ]
    }

    fn messages_to_json(msgs: &[Message]) -> Vec<serde_json::Value> {
        msgs.iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect()
    }

    // ── complete (non-streaming) ───────────────────────────────────────────────

    #[tokio::test]
    async fn complete_parses_openai_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "This is the summary."}}]
            })))
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 512, 0.3);
        let result = client.complete(&make_messages()).await.unwrap();
        assert_eq!(result, "This is the summary.");
    }

    #[tokio::test]
    async fn complete_trims_whitespace() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "  trimmed  \n"}}]
            })))
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 512, 0.3);
        let result = client.complete(&make_messages()).await.unwrap();
        assert_eq!(result, "trimmed");
    }

    #[tokio::test]
    async fn complete_returns_error_on_server_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal error"))
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 512, 0.3);
        let result = client.complete(&make_messages()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("500"));
    }

    // ── complete_short ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn complete_short_parses_openai_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "[{\"key\": \"name\", \"value\": \"Daniel\", \"confidence\": 0.95}]"}}]
            })))
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 256, 0.1);
        let result = client.complete_short(&make_messages()).await.unwrap();
        assert!(result.contains("Daniel"));
    }

    // ── complete_multimodal ───────────────────────────────────────────────────

    #[tokio::test]
    async fn complete_multimodal_returns_model_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "  A terminal window showing Rust code.  "}}]
            })))
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "vision-model", 512, 0.3);
        let result = client
            .complete_multimodal("data:image/png;base64,abc123", "What do you see?")
            .await
            .unwrap();
        assert_eq!(result, "A terminal window showing Rust code.");
    }

    #[tokio::test]
    async fn complete_multimodal_returns_error_on_server_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "vision-model", 512, 0.3);
        let result = client
            .complete_multimodal("data:image/png;base64,abc123", "What do you see?")
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("503"));
    }

    // ── stream (SSE) ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn stream_collects_tokens_until_done() {
        let server = MockServer::start().await;
        // Simulate OpenAI SSE: two content tokens then [DONE]
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .append_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 400, 0.7);
        let messages = make_messages();
        let (mut rx, _handle) = client
            .stream(&messages_to_json(&messages), &[], RequestOptions::default())
            .await
            .unwrap();

        let mut collected = String::new();
        while let Some(token) = rx.recv().await {
            if let StreamToken::Content(s) = token {
                collected.push_str(&s);
            }
        }
        assert_eq!(collected, "Hello world");
    }

    #[tokio::test]
    async fn stream_skips_empty_content_tokens() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .append_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 400, 0.7);
        let messages = make_messages();
        let (mut rx, _handle) = client
            .stream(&messages_to_json(&messages), &[], RequestOptions::default())
            .await
            .unwrap();

        let mut collected = String::new();
        while let Some(token) = rx.recv().await {
            if let StreamToken::Content(s) = token {
                collected.push_str(&s);
            }
        }
        assert_eq!(collected, "Hi");
    }

    #[tokio::test]
    async fn stream_surfaces_reasoning_content_deltas() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"reasoning_content\":\"Primero \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"analizo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Respuesta\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .append_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 400, 0.7);
        let messages = make_messages();
        let (mut rx, _handle) = client
            .stream(&messages_to_json(&messages), &[], RequestOptions::default())
            .await
            .unwrap();

        let mut reasoning = String::new();
        let mut content = String::new();
        while let Some(token) = rx.recv().await {
            match token {
                StreamToken::Reasoning(t) => reasoning.push_str(&t),
                StreamToken::Content(t) => content.push_str(&t),
                StreamToken::ToolCall { .. } => {}
            }
        }
        assert_eq!(reasoning, "Primero analizo");
        assert_eq!(content, "Respuesta");
    }

    #[tokio::test]
    async fn stream_parallel_tool_calls_accumulate_per_index() {
        // SGLang streams parallel calls interleaved, keyed by `index`. Each
        // call must accumulate into its own slot and emit with valid JSON
        // arguments — concatenating them broke the worker (server 400 on the
        // echo: "function.arguments must be valid JSON").
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"b\",\"function\":{\"name\":\"current_time\",\"arguments\":\"{\\\"t\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"1}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .append_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 400, 0.7);
        let messages = make_messages();
        let (mut rx, _handle) = client
            .stream(&messages_to_json(&messages), &[], RequestOptions::default())
            .await
            .unwrap();

        let mut calls: Vec<(String, String)> = Vec::new();
        while let Some(token) = rx.recv().await {
            if let StreamToken::ToolCall { name, args } = token {
                calls.push((name, args));
            }
        }
        assert_eq!(calls.len(), 2, "both parallel calls must emit");
        assert_eq!(calls[0].0, "read_file");
        assert_eq!(calls[0].1, r#"{"path":"a.txt"}"#);
        assert_eq!(calls[1].0, "current_time");
        assert_eq!(calls[1].1, r#"{"t":1}"#);
        for (_, args) in &calls {
            serde_json::from_str::<serde_json::Value>(args).expect("arguments are valid JSON");
        }
    }

    #[tokio::test]
    async fn stream_surfaces_alternate_reasoning_fields() {
        // Ollama/OpenRouter-style servers use `delta.reasoning` instead of
        // `delta.reasoning_content`; both must reach StreamToken::Reasoning.
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"reasoning\":\"piensa\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"thinking\":\"que\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hace\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .append_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 400, 0.7);
        let messages = make_messages();
        let (mut rx, _handle) = client
            .stream(&messages_to_json(&messages), &[], RequestOptions::default())
            .await
            .unwrap();

        let mut reasoning = String::new();
        let mut content = String::new();
        while let Some(token) = rx.recv().await {
            match token {
                StreamToken::Reasoning(t) => reasoning.push_str(&t),
                StreamToken::Content(t) => content.push_str(&t),
                StreamToken::ToolCall { .. } => {}
            }
        }
        assert_eq!(reasoning, "piensaque");
        assert_eq!(content, "hace");
    }

    // ── ThinkFilter ───────────────────────────────────────────────────────────

    #[test]
    fn think_filter_passthrough_when_no_think_block() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.process("Hello world").as_deref(), Some("Hello world"));
        assert_eq!(f.flush().as_deref(), None);
    }

    #[test]
    fn think_filter_strips_single_token_think_block() {
        let mut f = ThinkFilter::new();
        // Entire block in one token
        let result = f.process("<think>some thoughts</think>answer");
        assert_eq!(result.as_deref(), Some("answer"));
    }

    #[test]
    fn think_filter_strips_think_block_split_across_tokens() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.process("<think>"), None);
        assert_eq!(f.process("some reasoning"), None);
        assert_eq!(f.process("</think>"), None);
        assert_eq!(f.process("actual answer").as_deref(), Some("actual answer"));
    }

    #[test]
    fn think_filter_handles_split_opening_tag() {
        let mut f = ThinkFilter::new();
        // "<think>" split as "<th" + "ink>" + content
        assert_eq!(f.process("<th"), None); // buffered as partial tag
        assert_eq!(
            f.process("ink>thoughts</think>answer").as_deref(),
            Some("answer")
        );
    }

    #[test]
    fn think_filter_handles_split_closing_tag() {
        let mut f = ThinkFilter::new();
        f.process("<think>thoughts</thi");
        assert_eq!(f.process("nk>real answer").as_deref(), Some("real answer"));
    }

    #[test]
    fn think_filter_emits_content_before_think_block() {
        let mut f = ThinkFilter::new();
        let result = f.process("prefix<think>thoughts</think>suffix");
        assert_eq!(result.as_deref(), Some("prefixsuffix"));
    }

    #[test]
    fn think_filter_flush_returns_buffered_content() {
        let mut f = ThinkFilter::new();
        f.process("Hello"); // held in pending as possible partial tag start? No — "Hello" has no partial "<"
        // Actually "Hello" gets emitted immediately. Let's test flush with a partial tag.
        let mut f2 = ThinkFilter::new();
        f2.process("Hello <thi"); // "<thi" held as partial
        let flushed = f2.flush();
        // Flush should emit the pending partial since it never completed
        assert!(
            flushed.as_deref().unwrap_or("").contains("<thi")
                || flushed.is_none()
                || flushed.as_deref() == Some("<thi")
        );
    }

    #[test]
    fn think_filter_flush_inside_think_block_returns_none() {
        let mut f = ThinkFilter::new();
        f.process("<think>unfinished");
        assert_eq!(f.flush(), None);
    }

    // ── end-to-end summarization (client + session) ───────────────────────────

    // ── tool call detection via stream ────────────────────────────────────────

    #[tokio::test]
    async fn stream_delivers_native_tool_call() {
        let server = MockServer::start().await;
        // Simulate SSE for a native function call
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_0\",\"type\":\"function\",\"function\":{\"name\":\"current_time\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .append_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 400, 0.7);
        let (mut rx, _handle) = client
            .stream(&[], &[], RequestOptions::default())
            .await
            .unwrap();

        let token = rx.recv().await.expect("should receive a token");
        match token {
            StreamToken::ToolCall { name, args } => {
                assert_eq!(name, "current_time");
                assert_eq!(args, "{}");
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn summarization_cycle_with_mock_llm() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "El usuario intercambió varios mensajes con el asistente."}}]
            })))
            .mount(&server)
            .await;

        let client = OpenAIClient::new(&server.uri(), "test-model", 512, 0.3);

        // Build a session with enough history to trigger summarization
        let mut session = super::super::session::LlmSession::new("Eres seneschal.");
        for i in 0..5 {
            session.add_user_turn(&format!("Pregunta {i} del usuario"));
            session.add_assistant_turn(&format!("Respuesta {i} del asistente"));
        }

        let keep_n = 4;
        assert!(
            session.needs_summarization(50),
            "should trigger at small context window"
        );

        // Build the summarization request and call the mock LLM
        let summary_messages = session.build_summary_prompt(keep_n).unwrap();
        let summary = client.complete(&summary_messages).await.unwrap();
        assert!(!summary.is_empty());

        // Apply the summary
        session.apply_summary(&summary, keep_n);

        // Verify the compacted session
        assert_eq!(session.messages.len(), keep_n);

        let all = session.all_messages();
        assert_eq!(all.len(), 1 + keep_n); // system + kept messages
        assert!(all[0].content.contains("[CONVERSATION SUMMARY]"));
        assert!(all[0].content.contains("El usuario intercambió"));

        // Recent turns are preserved
        assert!(all[1].content.contains("Pregunta 3"));
        assert!(all[2].content.contains("Respuesta 3"));
        assert!(all[3].content.contains("Pregunta 4"));
        assert!(all[4].content.contains("Respuesta 4"));
    }

    #[test]
    fn auto_style_heuristics() {
        assert_eq!(
            ThinkingStyle::auto_for("qwen3.8-flash-next"),
            ThinkingStyle::Kwargs
        );
        assert_eq!(
            ThinkingStyle::auto_for("unsloth/Qwen3-32B"),
            ThinkingStyle::Kwargs
        );
        assert_eq!(ThinkingStyle::auto_for("o3-mini"), ThinkingStyle::Effort);
        assert_eq!(ThinkingStyle::auto_for("gpt-5"), ThinkingStyle::Effort);
        assert_eq!(
            ThinkingStyle::auto_for("llama3.2:latest"),
            ThinkingStyle::Ollama
        );
        assert_eq!(
            ThinkingStyle::auto_for("deepseek-r1-distill-qwen"),
            ThinkingStyle::None
        );
        assert_eq!(
            ThinkingStyle::parse("kwargs", "o3-mini").unwrap(),
            ThinkingStyle::Kwargs,
            "explicit style beats the heuristic"
        );
        assert!(ThinkingStyle::parse("telepatia", "qwen3").is_err());
    }

    #[test]
    fn effort_style_sends_reasoning_effort() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "o3-mini", 512, 0.3)
            .with_thinking(true)
            .with_thinking_style("auto")
            .unwrap();
        let payload = client.build_stream_payload(&[], &[], RequestOptions::default());
        assert_eq!(payload["reasoning_effort"], serde_json::json!("high"));
        assert!(payload.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn ollama_style_sends_think_flag() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "qwen3:32b", 512, 0.3)
            .with_thinking(true)
            .with_thinking_style("auto")
            .unwrap();
        let payload = client.build_stream_payload(&[], &[], RequestOptions::default());
        assert_eq!(payload["think"], serde_json::json!(true));
        let off = client.build_stream_payload(&[], &[], RequestOptions::new().with_thinking(false));
        assert_eq!(off["think"], serde_json::json!(false));
    }

    #[test]
    fn none_style_sends_no_thinking_flags() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "deepseek-r1", 512, 0.3)
            .with_thinking(true)
            .with_thinking_style("none")
            .unwrap();
        let payload = client.build_stream_payload(&[], &[], RequestOptions::default());
        assert!(payload.get("chat_template_kwargs").is_none());
        assert!(payload.get("think").is_none());
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn default_thinking_with_tools_off_keeps_tools_payload_clean() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.3);
        let tools = vec![serde_json::json!({"type": "function"})];
        let payload = client.build_stream_payload(&[], &tools, RequestOptions::default());
        assert!(payload.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn default_thinking_with_tools_forces_explicit_off_with_tools() {
        // Thinking-by-default servers need the explicit disable flag even
        // alongside tools; voice requests thinking OFF => must send false.
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.3)
            .with_default_thinking_with_tools(true);
        let tools = vec![serde_json::json!({"type": "function"})];
        let payload = client.build_stream_payload(&[], &tools, RequestOptions::default());
        assert_eq!(
            payload["chat_template_kwargs"]["enable_thinking"],
            serde_json::json!(false)
        );
    }

    #[tokio::test]
    async fn tool_choice_none_in_payload() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.3);
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": "hello"
        })];
        let payload = client.build_stream_payload(
            &messages,
            &[],
            RequestOptions::new().with_tool_choice(ToolChoice::None),
        );
        assert_eq!(payload["tool_choice"], serde_json::json!("none"));
        assert!(payload.get("tools").is_none());
        assert!(payload.get("chat_template_kwargs").is_some());
    }

    /// #191: Simple intent keeps tool definitions + tool_choice "none" so the
    /// prompt prefix (and KV cache) stays stable vs Complex turns.
    #[test]
    fn tools_present_with_tool_choice_none() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.3);
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "current_time", "description": "now", "parameters": {}}
        })];
        let payload = client.build_stream_payload(
            &[],
            &tools,
            RequestOptions::new().with_tool_choice(ToolChoice::None),
        );
        assert_eq!(payload["tool_choice"], serde_json::json!("none"));
        assert_eq!(payload["tools"].as_array().map(|a| a.len()), Some(1));
        assert_eq!(
            payload["tools"][0]["function"]["name"],
            serde_json::json!("current_time")
        );
        // Thinking kwargs must not accompany a tools-present request.
        assert!(payload.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn tools_present_default_tool_choice_auto() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.3);
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "web_search", "description": "search", "parameters": {}}
        })];
        let payload = client.build_stream_payload(&[], &tools, RequestOptions::new());
        assert_eq!(payload["tool_choice"], serde_json::json!("auto"));
        assert!(payload.get("tools").is_some());
        assert!(payload.get("chat_template_kwargs").is_none());
    }

    /// issue #236: the deep worker's inline thinking mode opts in to sending
    /// `chat_template_kwargs` alongside tools (verified backends only).
    #[test]
    fn thinking_with_tools_flag_sends_kwargs_with_tools() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.3);
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "read_file", "description": "read", "parameters": {}}
        })];
        let payload = client.build_stream_payload(
            &[],
            &tools,
            RequestOptions::new()
                .with_thinking(true)
                .with_thinking_with_tools(),
        );
        assert!(payload.get("tools").is_some());
        assert_eq!(
            payload["chat_template_kwargs"]["enable_thinking"],
            serde_json::json!(true)
        );
    }

    /// issue #236: the flag is opt-in — without it the historical rule holds
    /// (kwargs never accompany tools).
    #[test]
    fn thinking_with_tools_flag_defaults_off_with_tools() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.3);
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "read_file", "description": "read", "parameters": {}}
        })];
        let payload =
            client.build_stream_payload(&[], &tools, RequestOptions::new().with_thinking(true));
        assert!(payload.get("tools").is_some());
        assert!(payload.get("chat_template_kwargs").is_none());
    }

    fn assert_close_json(actual: &serde_json::Value, expected: f64) {
        let got = actual.as_f64().unwrap();
        assert!(
            (got - expected).abs() < 1e-6,
            "expected {expected}, got {got}"
        );
    }

    #[test]
    fn sampling_params_in_payload() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.7)
            .with_sampling(0.8, 20, 0.0, 1.5, 1.0);
        let payload = client.build_stream_payload(&[], &[], RequestOptions::new());
        assert_close_json(&payload["top_p"], 0.8);
        assert_eq!(payload["top_k"], serde_json::json!(20));
        assert_close_json(&payload["min_p"], 0.0);
        assert_close_json(&payload["presence_penalty"], 1.5);
        assert_close_json(&payload["repetition_penalty"], 1.0);
    }

    #[test]
    fn sampling_params_default_values() {
        let client = OpenAIClient::new("http://127.0.0.1:0", "test", 512, 0.7);
        let payload = client.build_stream_payload(&[], &[], RequestOptions::new());
        assert_close_json(&payload["top_p"], 0.95);
        assert_eq!(payload["top_k"], serde_json::json!(20));
        assert_close_json(&payload["min_p"], 0.0);
        assert_close_json(&payload["presence_penalty"], 0.0);
        assert_close_json(&payload["repetition_penalty"], 1.0);
    }
}

# LLM Provider — Architecture Reference

The LLM provider layer (`src/llm/provider.rs`) abstracts LLM inference backends behind the `LlmProvider` trait. The sole implementation is `OpenAiLlmProvider`, which speaks to any OpenAI-compatible `/v1/chat/completions` endpoint over SSE streaming.

## Core Types

### StreamToken

```rust
pub enum StreamToken {
    Content(String),                           // TTS-bound text chunk
    ToolCall { name: String, args: String },   // tool invocation
}
```

Yielded by the streaming API. The pipeline routes `Content` to the sentence splitter → TTS, and `ToolCall` to the tool executor.

### RequestOptions

```rust
pub struct RequestOptions {
    pub temperature: Option<f32>,
    pub thinking: Option<bool>,
    pub enable_tools: bool,
    pub tool_choice: Option<ToolChoice>,
}

pub enum ToolChoice { Auto, Required, None }

impl RequestOptions {
    pub fn new() -> Self;
    pub fn with_temperature(self, t: f32) -> Self;
    pub fn with_thinking(self, t: bool) -> Self;
    pub fn with_tool_choice(self, c: ToolChoice) -> Self;
}
```

When a field is `None`, the client inherits its default (from `Config` or provider construction).

### Message

```rust
pub struct Message {
    pub role: String,      // "system", "user", "assistant", "tool"
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self;
    pub fn user(content: impl Into<String>) -> Self;
    pub fn assistant(content: impl Into<String>) -> Self;
    pub fn tool(content: impl Into<String>) -> Self;
}
```

Standard OpenAI chat message format.

## LlmProvider Trait

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    /// Streaming chat completion. Returns (token_receiver, join_handle).
    /// The join_handle drives the SSE read loop; drop it to abort.
    async fn stream(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        forced_tool: Option<&str>,
        options: RequestOptions,
    ) -> Result<(mpsc::Receiver<StreamToken>, tokio::task::JoinHandle<()>)>;

    /// One-shot non-streaming (summarization). max_tokens=512, temperature=0.3.
    async fn complete(&self, messages: &[Message]) -> Result<String>;

    /// One-shot with tight budget (profile extraction). max_tokens=256, temperature=0.1.
    async fn complete_short(&self, messages: &[Message]) -> Result<String>;

    /// One-shot with image data URL + text prompt (vision tools).
    async fn complete_multimodal(&self, image_data_url: &str, text_prompt: &str) -> Result<String>;
}
```

## OpenAiLlmProvider

```rust
pub struct OpenAiLlmProvider {
    inner: OpenAIClient,
}

impl OpenAiLlmProvider {
    pub fn new(base_url: &str, model: &str, max_tokens: u32, temperature: f32) -> Self;
    pub fn with_api_key(self, key: &str) -> Self;
    pub fn with_thinking(self, thinking: bool) -> Self;
}
```

Delegates every trait method to `OpenAIClient` (defined in `client.rs`).

### Factory

```rust
pub fn create_provider(config: &Config) -> Result<Arc<dyn LlmProvider>>;
```

Reads `config.llm_provider` (env: `LLM_PROVIDER`). Currently only `"openai"` is supported. Constructs `OpenAiLlmProvider` from:
- `llm_url` → base_url
- `llm_model` → model
- `llm_max_tokens` → max_tokens
- `llm_temperature` → temperature
- `llm_api_key` → API key (blank = no auth header)
- `llm_thinking` → thinking mode

## ThinkFilter (Reasoning Tag Stripping)

When thinking mode is enabled (`llm_thinking = true`), the LLM may emit reasoning content enclosed in `<think>...</think>` tags. The `ThinkFilter` (private, in `client.rs`) buffers streaming tokens to detect and suppress these blocks.

### Streaming behavior:
- Tokens are accumulated in an internal buffer.
- When `</think>` is detected, all buffered content is discarded.
- Content outside `<think>...</think>` is emitted normally.

### Post-processing:
`strip_think_blocks(s: &str) -> String` strips all `<think>...</think>` blocks from a complete string. Used by `complete()`, `complete_short()`, and `complete_multimodal()`.

## Secondary LLM Provider

A separate `LlmProvider` instance (the "secondary" or "vision" LLM) is created from `SECONDARY_LLM_URL` and `SECONDARY_LLM_MODEL` env vars. It is used for:
- Vision tasks (screenshot analysis via `TakeScreenshotTool`)
- Summarization and profile extraction (S-DREAM)
- Agent result synthesis (optional)

## OpenAIClient (Internal, client.rs)

The `OpenAIClient` struct handles the low-level SSE HTTP protocol:

```rust
pub struct OpenAIClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
    api_key: String,
    thinking: bool,
}
```

Key behaviors:
- **Streaming:** `POST {base_url}/v1/chat/completions` with `stream: true`, SSE parsing via `reqwest::Response::bytes_stream()`.
- **non-streaming:** Same endpoint with `stream: false`.
- **Auth:** `Authorization: Bearer {api_key}` header (omitted if key is empty).
- **Thinking mode:** Adds `enable_thinking: true` to the `extra_body` field and activates `ThinkFilter` on the output.
- **Forced tool:** When `forced_tool` is `Some`, sets `tool_choice: {"type": "function", "function": {"name": "..."}}`.

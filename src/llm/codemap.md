# src/llm/ — LLM Client, Session Management, and Process Supervision

## Responsibility

Provides the LLM abstraction layer: an async HTTP client for OpenAI-compatible `/v1/chat/completions` endpoints, a conversation session manager with summarization/compaction, a provider trait for backend interchangeability, and a process supervisor for self-managed LLM servers.

## Design

### `LlmProvider` Trait (`provider.rs`)

Async trait with four methods:
- `stream(messages, tools, forced_tool)` — SSE streaming with tool-call detection. Returns `(mpsc::Receiver<StreamToken>, JoinHandle)` so the caller can consume tokens while the stream handle drives the HTTP response.
- `complete(messages)` — One-shot non-streaming completion (512 max tokens, 0.3 temperature). Used for summarization.
- `complete_short(messages)` — One-shot with short budget (256 tokens, 0.1 temperature). Used for structured extractions (profile facts, memory extraction).
- `complete_multimodal(image_data_url, text_prompt)` — One-shot with image + text. Used by `TakeScreenshotTool` and `EyesDaemon`.

Single implementation: `OpenAiLlmProvider` — thin wrapper around `OpenAIClient` that delegates all methods.

### `OpenAIClient` (`client.rs`)

HTTP client with:
- `reqwest::Client` configured with TCP keepalive (60s), Nagle disabled (`tcp_nodelay`), 5s connect timeout, connection pooling (4 max idle per host, 90s idle timeout).
- Builder pattern: `new(base_url, model, max_tokens, temperature) → with_api_key(key) → with_thinking(bool)`.
- `chat_url` constructed as `{base_url}/v1/chat/completions`.

**SSE Stream Parsing (`stream` method):**
- Sends POST with `stream: true`, tool definitions, sampling params (`repetition_penalty: 1.1`, `top_k: 40`, `top_p: 0.90`).
- When tools are active: `tool_choice = "required"` (if `forced_tool`) or `"auto"`. Does NOT send `chat_template_kwargs` (avoids Jinja2 template conflicts with tool calling).
- When no tools: sends `chat_template_kwargs: {"enable_thinking": true}` if `thinking` is enabled.
- Spawns a tokio task that reads the SSE byte stream, buffers partial lines, parses `data:` JSON, and sends `StreamToken` variants through the channel.
- Handles `finish_reason: "tool_calls"` and `data: [DONE]` stream termination.
- Accumulates tool call fragments across multiple SSE chunks (mlx-lm may split tool_calls across chunks).

**ThinkFilter (`client.rs`):**
- Stateful stream filter that strips `<think>…</think>` blocks from token sequences.
- Handles tags split across token boundaries by buffering up to `max(tag_len - 1)` trailing bytes.
- `partial_tag_suffix(s, tag)` computes the longest suffix of `s` that is a proper prefix of `tag`.
- `flush()` emits any buffered non-think content when the stream ends.

**Non-streaming methods:**
- `complete()` — POST with `stream: false`, 512 max tokens. Returns `choices[0].message.content`.
- `complete_short()` — Same but 256 tokens, 0.1 temperature.
- `complete_multimodal()` — Sends `image_url` + `text` content array. Returns trimmed text.
- All non-streaming methods apply `strip_think_blocks()` when `thinking` is enabled.

### `LlmSession` (`session.rs`)

In-memory conversation state with:
- `original_system_prompt` — Base system prompt, updated at runtime after consolidation.
- `summary` — Optional conversation summary, injected into system message as `[CONVERSATION SUMMARY]`.
- `messages: Vec<serde_json::Value>` — Conversation turns in OpenAI JSON format (supports tool-call exchanges with `tool_calls` arrays).
- `is_user_message_pending` — Flag: true when last turn is user without assistant response. Controls barge-in append vs new turn.
- `cached_formatted_history` — Cached text history for agent tools.
- `injection_role` — Role used for `add_internal_notification()` (user/system/developer).

**Key methods:**
- `add_user_turn(text)` — Pushes user message, sets pending flag.
- `update_last_user_turn(extra)` — Appends to last user message if pending (barge-in support). Returns false if no appendable turn exists.
- `add_assistant_turn(text)` — Pushes assistant message, clears pending flag.
- `add_tool_exchange(exchanges)` — Extends messages with tool-call + tool-result messages.
- `add_internal_notification(text)` — Injects system/developer/user message based on `injection_role`.
- `all_messages_api()` — Returns `Vec<serde_json::Value>` for API calls (system + all messages).
- `all_messages()` — Returns `Vec<Message>` for legacy callers (skips tool-call null-content messages).
- `format_history()` — Returns cached `[User]: ...\n[Jarvis]: ...` text for agent tools.
- `approx_tokens()` — Rough estimate: `total_chars * 10 / 35`.
- `needs_consolidation(context_limit_tokens, threshold_pct)` — True when estimated tokens exceed threshold percentage of context limit and message count >= 4.
- `summarizable_turn_count(keep_n)` — Messages to summarize = `len - keep_n`.
- `build_summary_prompt(keep_n)` — Builds system + user messages for summarization LLM call.
- `apply_summary(summary, keep_n)` — Discards old messages, keeps last `keep_n`, stores summary.
- `set_system_prompt(new_prompt)` — Replaces base system prompt at runtime (used after consolidation).
- `from_history(system_prompt, summary, history, injection_role)` — Restores session from DB history, replaying tool exchanges.

### `LlmManager` (`manager.rs`)

Process supervisor for self-managed LLM servers:
- `start_and_wait_ready(command, health_url)` — Spawns the LLM process, polls `/v1/models` until ready (1000ms interval, 120s timeout).
- `supervise(child, command, health_url, notify_tx)` — Monitors process exit, restarts up to `MAX_RESTARTS = 3` times, sends error notification on exhaustion.
- `spawn_process(command)` — Splits command string, spawns with stdout to log file, stderr to null.
- `wait_until_ready(health_url)` — Polls `{health_url}/v1/models`; accepts 2xx or 404 (some servers return 404 for unknown models).

## Flow

### Streaming Turn Flow

```
llm_task → llm_client.stream(messages, tool_defs, forced_tool)
  → POST /v1/chat/completions { model, messages, tools, max_tokens, temperature, stream: true, ... }
  → reqwest response bytes_stream()
  → tokio task: buffer → parse SSE lines → parse JSON → emit StreamToken
    → StreamToken::Content(text) → llm_tx → sen_task → SentenceSplitter → TTS
    → StreamToken::ToolCall { name, args } → llm_task → tool execution
  → On finish_reason="tool_calls" or data:[DONE]: stream task returns
  → JoinHandle completes
```

### Non-streaming Completion Flow

```
consolidation_task → background_client.complete(summary_prompt)
  → POST /v1/chat/completions { stream: false, max_tokens: 512, temperature: 0.3 }
  → Parse JSON response → extract choices[0].message.content → trim → return String

eyes.rs → vision_client.complete_multimodal(data_url, prompt)
  → POST /v1/chat/completions { messages: [{ role: "user", content: [{ image_url }, { text }] }] }
  → Parse JSON → extract content → strip think blocks → return String
```

### Session Lifecycle

```
Startup:
  LlmSession::from_history(system_prompt, db_summary, db_history, injection_role)
    → Replays User/Assistant/System/ToolExchanges from SQLite

Per turn:
  llm_session.lock().unwrap().add_user_turn(text)  // or update_last_user_turn for barge-in
  → llm_client.stream(all_messages_api(), tool_defs, forced_tool)
  → llm_session.lock().unwrap().add_assistant_turn(response)
  → llm_session.lock().unwrap().add_tool_exchange(exchanges)  // if tools were called

Consolidation:
  llm_session.lock().unwrap().needs_consolidation(context_tokens, threshold_pct)
  → build_summary_prompt(keep_turns) → background_client.complete() → apply_summary()
  → set_system_prompt(rebuilt_prompt_with_updated_memories)
```

### Self-managed LLM Supervisor

```
main.rs (if config.llm_self_managed):
  → start_and_wait_ready(command, llm_url)
    → spawn_process(command) → wait_until_ready(llm_url)
      → Poll GET {llm_url}/v1/models until 2xx/404 (1000ms interval, 120s timeout)
  → supervise(child, command, llm_url, notify_tx)
    → child.wait() → if exited: restart (up to 3 times) → if exhausted: send error notification
```

## Integration

### Dependencies (consumed by llm module)
- `reqwest` — HTTP client for OpenAI-compatible API
- `futures_util::StreamExt` — Async stream iteration for SSE parsing
- `serde_json` — JSON serialization/deserialization
- `tokio::sync::mpsc` — Token channel for streaming
- `async_trait` — Async trait for `LlmProvider`
- `crate::config::Config` — Configuration for provider factory and process manager

### Consumers (depend on llm module)
- `src/pipeline/llm_task.rs` — Primary consumer: calls `stream()` for conversation turns
- `src/pipeline/consolidation.rs` — Calls `complete()` for summarization, `complete_short()` for profile/memory extraction
- `src/daemon.rs` — `InferenceDaemon` calls `complete_short()` for proactive inference
- `src/eyes.rs` — `EyesDaemon` calls `complete_multimodal()` for screen analysis
- `src/tools/` — Various tools call `complete()` or `complete_multimodal()` (TakeScreenshotTool, WebSearchTool synthesis, RunAgentTool synthesis)
- `src/main.rs` — Creates `LlmSession`, calls `create_provider()`, spawns `llm_manager::supervise()`
- `src/agents/` — ACP session manager may use LLM for result synthesis
- `src/dream/` — S-DREAM daemon uses secondary LLM for cold-path consolidation
- `src/mcp/` — MCP tool proxy may use LLM for result synthesis
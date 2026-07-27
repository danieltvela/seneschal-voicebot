# LLM — Task List

Module: `src/llm/`

---

## [M0.2] OpenAIClient

### Streaming (`src/llm/client.rs`)
- [x] SSE streaming via `reqwest` to `/v1/chat/completions`
- [x] `StreamToken::Content` and `StreamToken::ToolCall` variants
- [x] Extra sampling params (`repetition_penalty`, `top_k`, `min_p`) sent on every request
- [x] Think filter: strips `<antThinking>...</antThinking>` from reasoning model output
- [ ] **Connection pool**: reuse HTTP connections via `reqwest::Client` pool; verify keep-alive behavior
- [ ] **Retry logic**: on transient errors (5xx, connection reset), retry up to 2 times with exponential backoff
- [ ] **Request timeout**: add configurable timeout (`LLM_REQUEST_TIMEOUT_SECS`, default 30s) for the initial connection + first token
- [ ] **Token budget tracking**: count tokens consumed per turn; log when approaching `LLM_MAX_TOKENS`
- [ ] **Streaming error recovery**: if SSE stream drops mid-response, inject a graceful error message ("My response was interrupted...") instead of silent truncation

### Complete (non-streaming)
- [x] `complete()` for one-shot memory extraction, profile, daemon
- [x] `complete_short()` for lightweight daemon prompts
- [ ] **Short complete timeout**: `complete_short` should have a 10-second deadline; `complete` should have 60 seconds
- [ ] **Rate limit**: enforce max concurrent `complete()` calls (1 at a time) to avoid KV-cache pressure

### Provider support
- [x] OpenAI-compatible endpoints (mlx-lm, oMLX, OpenAI, Ollama)
- [x] Optional API key (`LLM_API_KEY`)
- [x] `LLM_PROVIDER` hint for provider-specific tweaks
- [ ] **Provider auto-detection**: on first request, detect provider type from response headers; log detected provider
- [ ] **Anthropic-compatible endpoint**: add support for non-OpenAI streaming formats behind a feature flag

---

## [M0.2] LlmSession (`src/llm/session.rs`)

- [x] Message history management with role tracking (user, assistant, system)
- [x] Context consolidation integration
- [ ] **Token counting accuracy**: use a proper tokenizer (not character-based estimation) for `needs_consolidation()` threshold
  - Consider: `tiktoken-rs` for OpenAI token counting, or approximate with 4 chars/token for Latin scripts
- [ ] **History compaction**: when loading history on startup with `LLM_HISTORY_LOAD_LIMIT`, summarize older messages instead of just dropping them
- [ ] **System prompt injection points**: document all injection points — base prompt, user profile, memories, conversation summary, tool definitions

### LLM Manager (`src/llm/manager.rs`)
- [ ] **Self-managed LLM process**: improve `LLM_SELF_MANAGED` mode:
  - Health check endpoint (poll `/v1/models` every 30 seconds)
  - Graceful shutdown sequence (SIGTERM, wait, SIGKILL)
  - Crash counter with backoff (3 crashes in 5 minutes → stop restarting)
- [ ] **GPU memory monitoring**: log GPU memory pressure from mlx-lm/oMLX status endpoints

---

## [M0.4] LLM Intelligence

### Intent Routing
- [ ] **Intent classifier**: classify each user utterance as `query`, `command`, `chitchat`, `emergency`
  - Use lightweight prompt or keyword matching (not a second LLM call)
- [ ] **Temperature adjustment**: low temperature for commands (0.1), normal for queries (0.7), higher for chitchat (0.9)
- [ ] **Emergency override**: specific phrases ("help", "stop", "emergencia") bypass all tool execution and trigger predefined response

### Speculative Prefill (future)
- [ ] **Prefill cache**: pre-compute LLM KV-cache for common prefixes (system prompt) to reduce time-to-first-token
- [ ] **Parallel speculation**: when user pauses, speculatively prefill LLM with possible continuations of the partial transcript

---

## [M1.1] Reliability

- [ ] **Graceful degradation**: if primary LLM is unreachable, route to `SECONDARY_LLM_URL` as fallback
- [ ] **Context window overflow**: if the estimated token count exceeds the model's context window, force consolidation before sending the request (never send a request that will be truncated)
- [ ] **Response quality checks**: detect degenerate LLM output (repetition, gibberish, empty) and log; offer retry

---

*Last updated: 2026-07-27.*

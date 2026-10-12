# Seneschal — Constitution (Standard Operating Procedure)

This document defines the **invariant rules** and **design principles** that govern
the Seneschal project. Every decision — architectural, code-level, operational — must
conform to these principles. Amendments require explicit deliberation; no agent or
contributor may override them implicitly.

---

## I. Identity

### 1. Name
The project is **Seneschal**. The name is inspired by the Steward of Gondor:
the seneschal serves the king, and **the user is the king**.

- **Never** refer to the project as "Jarvis" or "Hive." Jarvis(R) is a trademark of
  Marvel Studios/Disney. This is an independent fan project.
- The binary is `seneschal`. The repository is `seneschal-voicebot`. The config
  prefix in env vars is `SENESCHAL_`.

### 2. Brand
- Banner mark: plain white standard, swallowtail, no emblem — faithful to
  the Steward's sigil (see `BRAND.md`).
- Logo files: `assets/logo.svg` (lockup), `assets/logo-icon.svg` (icon),
  `assets/logo-wordmark.svg` (wordmark).
- Colors: `currentColor` theming; `#FFFFFF` on dark, `#0B1220` on light.

---

## II. Architecture — Non-Negotiable

### 1. Single Binary
Seneschal is a **single Rust binary**. All pipeline stages run in-process connected by
`tokio` channels. There is no inter-service communication, no microservices, no IPC.

**Rationale:** No serialization/deserialization overhead, no network round-trips, trivial
cancellation propagation, easy to reason about.

### 2. Streaming Pipeline
The pipeline is **STT → LLM → TTS**, connected by typed `mpsc` channels:

```
Microphone → AudioCapture (CPAL)
           → SttProvider (Whisper/Parakeet/SFSpeechRecognizer) + Silero VAD
           → LLM client (OpenAI-compatible /v1/chat/completions, streaming SSE)
           → SentenceSplitter (buffer until punctuation boundary)
           → TTS (macOS AVSpeechSynthesizer or Kokoro ONNX)
           → AudioOutput (CPAL)
```

### 3. Provider Traits
The pipeline has three pluggable layers, each behind an interface that makes the rest
of the pipeline backend-agnostic:

- **STT:** `SttProvider` trait with `process_audio` and `transcribe_complete`.
- **LLM:** `OpenAIClient` targets any OpenAI-compatible endpoint.
- **TTS:** `TtsEngine` enum dispatching to backend variants.

### 4. Pipeline FSM
Pipeline state is tracked by a **finite state machine** with a single
`tokio::sync::watch<PipelineState>` channel:

```rust
enum PipelineState {
    Idle,
    Listening { utterance_id: u64 },
    Thinking  { utterance_id: u64 },
    Speaking  { utterance_id: u64 },
    Paused    { reason: PauseReason },
}
```

**No central coordinator sits on the hot path.** Each actor that owns a transition
writes directly to the `watch` sender. Observers (TUI, Control API, logger) subscribe
to the receiver.

### 5. Actor Model via Tokio Tasks
Every pipeline stage is a `tokio::spawn` task with its own private state, an
`mpsc::Receiver` for input, and typed `mpsc::Sender` handles for output. Tasks
**never** share mutable state. Communication is exclusively through channels:

| Actor | Input channel | Output channel |
|-------|--------------|----------------|
| VAD/STT | `async_channel<AudioChunk>` | `mpsc<SpeechEvent>` |
| llm_task | `mpsc<PipelineFrame>` | `mpsc<LLMToken>` |
| sen_task | `mpsc<LLMToken>` | `mpsc<SentenceReady>` |
| tts_task | `mpsc<SentenceReady>` | speaker (CPAL) |

### 6. Sentence-by-Sentence TTS
The LLM response is not buffered entirely before TTS. Tokens stream in
real-time, the SentenceSplitter emits complete sentences on punctuation
boundaries, and the TTS task synthesizes and plays sentence N while the LLM
is still generating sentence N+1.

**Rationale:** First word heard by the user in under 1 second.

### 7. Barge-In
When the user speaks while the pipeline is active:
1. VAD detects `SpeechStart` → sends cancellation via `broadcast barge_in_tx`.
2. llm_task aborts the HTTP stream.
3. sen_task drains buffered tokens.
4. tts_task stops audio playback via `play_cancel: AtomicBool`.

Cancellation is **immediate and idempotent** — all actors must handle receiving
a cancel signal even after they have already completed.

### 8. No Speculative LLM on Local GPU
The local GPU (Apple Silicon M-series) is used for STT and TTS. Running a
speculative LLM on the same GPU would cause contention and jitter. The LLM
inference is delegated to an external process (mlx-lm or oMLX).

### 9. Narrow Scope
Seneschal owns the **audio pipeline and conversational experience only**.
Complex tasks (shell commands, file system, calendar, web navigation) are
**delegated to external agents** via ACP (Agent Communication Protocol).
The LLM sees agent tools as function calls; the agent returns results
asynchronously.

### 10. Apple MLX Backends Only
LLM inference uses Apple MLX-based servers (mlx-lm or oMLX) that maintain
KV-cache implicitly. This is substantially faster than llama.cpp on
Apple Silicon. Non-MLX backends may be supported, but performance is not
guaranteed.

---

## III. Code Standards — Non-Negotiable

### 1. Language
- **All code, comments, commit messages, and documentation are in English.**
- System prompt and user-facing spoken output may be in Spanish or English
  (`SENECHAL_LANGUAGE` config).

### 2. Error Handling
- `anyhow::Result` with `.context()` strings for application code.
- `thiserror` for library-level custom error types.
- Never `unwrap()` in production code paths — use `?`, `.context()`, or
  explicit `match`.

### 3. Logging
- `tracing` crate throughout. Never `println!` in library or pipeline code.
- Use structured targets: `seneschal`, `audio`, `pipeline`, `sttvad`, `llm`,
  `tts`, `db`, `daemon`, `eyes`, `control`, `performance`, `speaker`, `profile`.
- Filter with `RUST_LOG` at runtime.
- When TUI is active, all logs redirect to `seneschal.log`.

### 4. Async Runtime
- **Tokio** is the only async runtime.
- Channels: `mpsc` for 1:1 data flow, `broadcast` for 1:N cancellation,
  `watch` for global state, `oneshot` for request-response.

### 5. Serialization
- `serde` + `serde_json` for all structured data.
- Environment config loaded via `dotenvy` → `Config::from_env()`.

### 6. Testing
- Unit tests: `#[test]` / `#[tokio::test]` anywhere under `src/`.
- Wiremock-based e2e: `src/e2e_tests.rs`, marked `#[ignore]`.
- Real-hardware tests (STT/LLM): `#[ignore]` + env guard at top.
- VAD/audio tests: synthetic sine waves and silence.
- TTS tests: macOS requires voices installed; Kokoro for CI.
- Run `make qa` before any PR: `cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo test --features tui,remote,control && cargo test e2e -- --ignored && cargo build --features tui,remote,control`.

### 7. Git Discipline
- Bugs/fixes: work directly on `main`.
- Features: create branch `feature/<short-name>`.
- Commit messages: English, short, descriptive (e.g., `feat: add speaker verification module`).
- Merge: interactive rebase → squash merge → delete feature branch.
- Never force-push, never push to main without explicit user instruction.
- Never `git push` from agent workflows — only local commits.

---

## IV. Data — Non-Negotiable

### 1. Persistence
- **SQLite** is the only database (`sqlx` with migrations).
- Tables: `sessions`, `messages`, `user_profile`, `memories`.
- Schema managed through `sqlx migrate` in `src/db/migrations/`.
- Database path: configurable via `DB_PATH` (default: `data/seneschal.db`).

### 2. Config Precedence
1. Environment variables (highest priority)
2. Explicit config file path (`SENESCHAL_CONFIG_FILE`)
3. Environment-specific TOML (`seneschal.{env}.toml`)
4. Embedded default config (lowest priority)

### 3. Environment Separation
- `SENESCHAL_ENV=pro` (default) and `SENESCHAL_ENV=dev`.
- Each environment has its own config file and `data/{env}/` directory.
- Never mix PRO and DEV data.

### 4. No Secrets in Code
- API keys, tokens, and secrets are loaded from environment variables only.
- Never hardcode credentials, URLs, or tokens.
- `.env` is in `.gitignore`. `.env.example` is committed with placeholder values.

---

## V. Module Boundaries — Non-Negotiable

### 1. Legacy Modules (do not extend)
- `src/stt/whisper.rs` — DEPRECATED; replaced by whisper-cpp-plus in `src/stt/mod.rs`.
- `src/websocket_client.rs` — No longer needed.
- `provider/` — Python LFM2.5-Audio server (not used).
- `src/tts/piper.rs` — Piper subprocess wrapper (reference only, not integrated).

**Do not add features to legacy modules. Flag for removal if found.**

### 2. Tool Registration
New tools must:
1. Implement the `Tool` trait in `src/tools/<name>.rs`.
2. Register in `main.rs` startup sequence.
3. Expose OpenAI-compatible JSON Schema via `parameters()`.
4. Be documented in `doc/TOOLS.md`.

### 3. New Providers
Follow the checklists in `doc/ARCHITECTURE.md` § "Adding a New Provider".

---

## VI. Platform

### 1. macOS First
- Primary target: **macOS 12.0+ (Apple Silicon M-series)**.
- Linux/Windows support is aspirational — macOS features (AVSpeech, CoreML,
  SFSpeechRecognizer, Apple Events) take priority.

### 2. Rust Edition
- **Rust edition 2024**, stable toolchain.
- `rust-toolchain.toml` pins the version.

---

## VII. Amendments

This constitution may be amended only through explicit deliberation.
Proposed changes must:
1. State the principle to be changed.
2. Explain the rationale.
3. Identify affected modules and migration path.
4. Be reviewed against all other principles for consistency.

No agent may amend this document autonomously.

---

*Last amended: 2026-07-27. Version 1.0.*

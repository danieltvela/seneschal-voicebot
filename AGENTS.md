# Voicebot — Agent Instructions

## Legal & Naming

- **Jarvis®** is a trademark of Marvel Studios/Disney. This is an independent fan project.
- Refer to this project as **"Voicebot"**. Never "Jarvis" or "Hive".

---

## Commands

```bash
cargo build --release      # Release build
cargo run                  # Dev run (reads .env)
cargo test                 # All tests
cargo fmt                  # Format
cargo clippy               # Lint
cargo run --features tui   # With TUI
cargo run -- --list-devices
cargo run -- --list-voices
```

---

## QA Workflow (AI agents: start here)

**Before opening a PR, run the QA harness end-to-end.**

```bash
make qa                # fast suite: fmt, lint, test, test-ci, test-e2e, build
make qa-full           # adds audit + coverage
QA_SKIP=audit,coverage make qa
```

| Stage | Command | Fails if… |
|---|---|---|
| `fmt` | `cargo fmt --check` | code is unformatted |
| `lint` | `cargo clippy --all-targets --no-deps -- -D warnings` | any clippy warning |
| `test` | `cargo test` | any default-features test fails |
| `test-ci` | `cargo test --features tui,remote,control` | any feature test fails |
| `test-e2e` | `cargo test e2e -- --ignored` | wiremock e2e harness fails |
| `test-stt` | `cargo test -- --ignored stt` | real-Whisper STT fails (skipped if no model) |
| `test-llm` | `cargo test -- --ignored llm` | real-LLM test fails (skipped if no LLM server) |
| `build` | `cargo build --features tui,remote,control` | release/feature build breaks |

---

## Architecture

Mono-user voice AI chatbot in Rust. Streaming STT→LLM→TTS pipeline, single process, tokio channels.

```
Microphone → AudioCapture (CPAL) → WhisperSTTVAD (whisper-cpp-plus + Silero VAD)
→ LLM client (OpenAI-compatible /v1/chat/completions, streaming SSE)
→ SentenceSplitter (buffer until punctuation boundary)
→ TTS (macOS AVSpeechSynthesizer or Kokoro ONNX)
→ AudioOutput (CPAL speaker)
```

**Key decisions:**
- **Single binary**: all stages connected by `tokio::sync` channels
- **No speculative LLM on local GPUs**: single-GPU contention causes jitter
- **LLM→TTS streaming**: buffer until sentence boundary, synthesize immediately
- **Barge-in**: `CancellationToken` cancels active pipeline
- **Pluggable STT**: Provider trait for Whisper, Parakeet, SFSpeechRecognizer
- **Cold Path Memory (S-DREAM)**: L1/L2 consolidation, see `src/dream/`

---

## Documentation Index

Detailed reference docs live in `doc/`. Read the relevant file when needed:

| Topic | File |
|-------|------|
| Module boundaries & legacy modules | [`doc/modules.md`](doc/modules.md) |
| Environment variables | [`doc/env-vars.md`](doc/env-vars.md) |
| Build features & dependencies | [`doc/build-features.md`](doc/build-features.md) |
| Config file precedence | [`doc/config.md`](doc/config.md) |
| Code style & patterns | [`doc/code-style.md`](doc/code-style.md) |
| Testing guidelines | [`doc/testing.md`](doc/testing.md) |
| Git workflow & issue process | [`doc/git-workflow.md`](doc/git-workflow.md) |
| Common workflows | [`doc/common-workflows.md`](doc/common-workflows.md) |

---

## Repository Map

A full codemap is available at `codemap.md`. Read it before any task. For deep work on a folder, also read that folder's `codemap.md`.
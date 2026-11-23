# Seneschal — Agent Instructions

## Legal & Naming

- **Jarvis®** is a trademark of Marvel Studios/Disney. This is an independent fan project.
- Refer to this project as **"Seneschal"**. Never "Jarvis" or "Hive".

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

# Desktop spike (independent Tauri workspace)
cd desktop
npm install
npm run sidecar:build      # build + stage the daemon sidecar
npm run dev                # build + launch the Tauri shell
npm run check && npm test
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
| `test` | `cargo test --workspace` | any default-features test fails |
| `test-ci` | `cargo test --features full` | any feature test fails |
| `test-e2e` | `cargo test e2e -- --ignored` | wiremock e2e harness fails |
| `test-stt` | `cargo test -- --ignored stt` | real-Whisper STT fails (skipped if no model) |
| `test-llm` | `cargo test -- --ignored llm` | real-LLM test fails (skipped if no LLM server) |
| `build` | `cargo build --features full` | release/feature build breaks |

---

## Architecture

Mono-user voice AI chatbot in Rust. Streaming STT→LLM→TTS pipeline, single process, tokio channels.

```
Microphone → AudioCapture (CPAL) → Nemotron ASR WebSocket (`crates/asr`,
server-side VAD endpointing)
→ LLM client (OpenAI-compatible /v1/chat/completions, streaming SSE)
→ SentenceSplitter (buffer until punctuation boundary)
→ TTS (macOS AVSpeechSynthesizer or Kokoro ONNX)
→ AudioOutput (CPAL speaker)
```

**Key decisions:**
- **Single binary**: all stages connected by `tokio::sync` channels
- **No speculative LLM on local GPUs**: single-GPU contention causes jitter. On multi-GPU hosts the LLM server (out-of-process) is pinned with `CUDA_VISIBLE_DEVICES`; the Nemotron ASR/Magpie TTS services run in their own containers on the secondary GPU
- **LLM→TTS streaming**: buffer until sentence boundary, synthesize immediately
- **Barge-in**: at the first Nemotron ASR partial, `barge_in_tx` cancels the in-flight LLM request (including one just sent) and the TTS queue + playback; STT/VAD keep running, and a dispatched deep-worker task is never cancelled
- **STT**: the runtime provider is `crates/asr` (Nemotron 3.5 over NeMo-Speech.cpp realtime WebSocket). Whisper/Parakeet/SFSpeechRecognizer remain as legacy providers in `seneschal-core`
- **STT/VAD contract**: endpointing is server-side; live partials are logged while the user speaks, the LLM is gated on the final transcript, and stale results are dropped after barge-in — specified in `doc/stt-contract.md` and locked by deterministic + audit tests
- **Desktop shell**: `desktop/` is an isolated Tauri v2 app that launches the real `seneschal` binary as a sidecar, consumes Control REST/SSE, and manages tiered Settings; see `doc/design/32-desktop-spike.md` and `doc/design/33-desktop-settings.md`
- **Desktop onboarding**: the first-run wizard owns a durable completion marker, bootstraps a runnable config, and suppresses the startup greeting until the final restart; see `doc/design/36-desktop-onboarding.md`
- **Cold Path Memory (S-DREAM)**: L1/L2 consolidation, see `src/dream/`

---

## Documentation Index

Detailed reference docs live in `doc/`. Read the relevant file when needed:

| Topic | File |
|-------|------|
| STT/VAD pipeline contract (P1–P6) | [`doc/stt-contract.md`](doc/stt-contract.md) |
| End-of-speech (EOT) detection research | [`doc/end-of-turn-detection.md`](doc/end-of-turn-detection.md) |
| Module boundaries & legacy modules | [`doc/modules.md`](doc/modules.md) |
| Environment variables | [`doc/env-vars.md`](doc/env-vars.md) |
| Build features & dependencies | [`doc/build-features.md`](doc/build-features.md) |
| Config file precedence | [`doc/config.md`](doc/config.md) |
| Code style & patterns | [`doc/code-style.md`](doc/code-style.md) |
| Testing guidelines | [`doc/testing.md`](doc/testing.md) |
| Git workflow & issue process | [`doc/git-workflow.md`](doc/git-workflow.md) |
| Common workflows | [`doc/common-workflows.md`](doc/common-workflows.md) |
| iOS / watch companion (dual-channel) | [`doc/IOS_COMPANION.md`](doc/IOS_COMPANION.md) |
| Apple Watch networking constraints | [`doc/APPLE_WATCH_CLIENT.md`](doc/APPLE_WATCH_CLIENT.md) |
| Desktop spike architecture | [`doc/design/32-desktop-spike.md`](doc/design/32-desktop-spike.md) |
| Desktop Settings & tiered apply | [`doc/design/33-desktop-settings.md`](doc/design/33-desktop-settings.md) |
| Desktop audio panel & diagnostics | [`doc/design/34-desktop-audio.md`](doc/design/34-desktop-audio.md) |
| Desktop model manager | [`doc/design/35-desktop-models.md`](doc/design/35-desktop-models.md) |
| Desktop onboarding wizard | [`doc/design/36-desktop-onboarding.md`](doc/design/36-desktop-onboarding.md) |

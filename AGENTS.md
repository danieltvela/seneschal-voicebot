# Voicebot — Agent Instructions

## Legal & Naming

- **Jarvis®** is a trademark of Marvel Studios/Disney. This is an independent fan project.
- Refer to this project as **"Voicebot"**. Never "Jarvis" or "Hive".
- See `LICENSE-VOICEBOT.md` for full details.

---

## Commands

```bash
cargo build --release      # Release build
cargo run                  # Dev run (reads .env)
cargo run --release        # Production run
cargo test                 # All tests
cargo fmt                  # Format
cargo clippy               # Lint

# Feature flags
cargo run --features kokoro     # Kokoro TTS backend
cargo run --features speech     # macOS SFSpeechRecognizer STT
cargo run --features tui        # Terminal UI
cargo run --features remote     # WebSocket server
cargo run --features speaker    # Speaker verification

# List devices/voices
cargo run -- --list-devices     # Or LIST_AUDIO_DEVICES=1 cargo run
cargo run -- --list-voices      # Or LIST_VOICES=1 cargo run

# Single test
cargo test <test_name>
cargo test -p voicebot <test_name>
```

---

## QA Workflow (AI agents: start here)

**Before opening a PR, every AI agent MUST run the QA harness end-to-end.** It is the single source of truth for "does this branch meet the bar".

```bash
make qa                # fast suite: fmt, lint, test, test-ci, test-e2e, build (~5 min)
make qa-full           # adds audit + coverage; skips cleanly if tools missing
make help              # discover individual stages
bash scripts/qa.sh     # direct, no `make` required
```

The harness exits 0 only if every stage ran to completion. Skipped stages (no Whisper model, no LLM server, no `cargo-audit`) print `[SKIP] reason` and are non-fatal.

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
| `audit` | `cargo audit` | known CVE in deps (skipped if `cargo-audit` missing) |
| `coverage` | `cargo llvm-cov` | tooling broken (skipped if `cargo-llvm-cov` missing) |

**Customization:**
```bash
QA_SKIP=audit,coverage make qa          # skip stages
QA_NO_COLOR=1 make qa                   # disable colors
QA_KEEP_GOING=1 bash scripts/qa.sh full # don't stop on first failure
```

**Adding new test categories:**
- Default unit tests: `#[test]` / `#[tokio::test]` anywhere under `src/`. Picked up by `test`.
- Wiremock-based e2e: add to `src/e2e_tests.rs` and mark `#[ignore]`. Picked up by `test-e2e`.
- Real-LLM / real-audio integration: `#[ignore]` + check env at top of fn. Add to `test-llm` / `test-stt` filter patterns.
- Zero-coverage modules: **add unit tests**, do not ignore them.

---

## Architecture Overview

Mono-user voice AI chatbot in Rust. Streaming STT→LLM→TTS pipeline, single process, tokio channels.

### Data Flow

```
Microphone → AudioCapture (CPAL) → WhisperSTTVAD (whisper-cpp-plus + Silero VAD)
      partial transcripts accumulated in-memory
→ LLM client (OpenAI-compatible /v1/chat/completions, streaming SSE)
      tokens streamed as they arrive
→ SentenceSplitter (buffer until punctuation boundary)
→ TTS (macOS AVSpeechSynthesizer or Kokoro ONNX)
      synthesizes sentence by sentence
→ AudioOutput (CPAL speaker)
```

### Key Design Decisions

- **Single binary**: no inter-service communication; all stages connected by `tokio::sync` channels
- **STT→LLM latency trick**: partial Whisper transcripts are accumulated in a `String`; when VAD signals end-of-speech the full transcript is sent to the LLM server. The server maintains its own KV-cache implicitly across requests within a session.
- **No speculative LLM on local GPUs**: Do NOT implement speculative / preemptive LLM generation (starting the LLM while the user is still speaking). This requires cloud-scale infrastructure with separate GPUs for STT and LLM. On a local single-GPU setup (Apple Silicon or one NVIDIA card), the speculative LLM task would contend with STT for the same GPU compute, causing jitter, wasted cycles, and memory pressure. The latency savings are real only when STT and LLM run on physically separate hardware.
- **LLM→TTS streaming**: LLM tokens arrive via SSE and are buffered until a sentence boundary (`.`, `!`, `?`, `;`, `:`) — then that sentence is synthesized immediately. While sentence N plays, sentence N+1 is being generated and synthesized.
- **Language**: English by default (`VOICEBOT_LANGUAGE=en`), Spanish supported. Affects Whisper hint and TTS voice. Parakeet auto-detects 25 languages.
- **Barge-in**: Implemented via `CancellationToken` (tokio-util). User speech cancels the active pipeline.
- **Pluggable STT**: Provider trait abstracts Whisper (whisper-cpp-plus), Parakeet (ONNX), and macOS SFSpeechRecognizer (`speech` crate). Whisper and Parakeet share Silero VAD. SFSpeechRecognizer uses Apple's built-in speech detection. Select via `STT_PROVIDER` env var.
- **Agent delegation**: Complex tasks can be delegated to external AI agents via the ACP protocol over stdio.
- **Cold Path Memory (S-DREAM)**: Background memory consolidation with L1/L2 dual-layer architecture. L1 tracks context saturation (profile + memories section exceeding ~4000 chars) and emits `ProactiveEvent::L1Saturated` to trigger consolidation. L2 is the long-term archive of consolidated conversations stored as JSONL files with rotation (10 MB / 10000 lines per file). The `recover_historical_context` tool searches the L2 archive via FTS5 full-text search. The `[IMMUTABLE RULES]` prompt block injects non-negotiable behavioral constraints during consolidation cycles. See `src/dream/`.

---

## Module Boundaries

| Directory | Purpose | Key Files |
|-----------|---------|-----------|
| `src/audio/` | Audio pipeline: capture, VAD, resampling, playback | `audio_capture.rs`, `vad.rs`, `buffer.rs`, `audio_transform.rs`, `output.rs`, `speaker.rs`, `ambient_buffer.rs` |
| `src/stt/` | Provider trait + Whisper + Parakeet + SFSpeechRecognizer implementations. 16kHz f32 mono. | `mod.rs`, `whisper.rs` (DEPRECATED), `parakeet.rs`, `speech_recognizer.rs` |
| `src/llm/` | HTTP client to `/v1/chat/completions`, session management | `client.rs` (OpenAIClient), `session.rs` (LlmSession), `manager.rs` |
| `src/tts/` | `avspeech.rs` (macOS AVSpeech), `sentence.rs` (boundary splitting), `kokoro.rs` (ONNX) | `avspeech.rs`, `kokoro.rs`, `sentence.rs`, `piper.rs` (reference) |
| `src/pipeline/` | Pipeline orchestration with FSM | `mod.rs`, `fsm.rs` (PipelineState), `llm_task.rs`, `tts_task.rs`, `sen_task.rs`, `frames.rs`, `state.rs`, `consolidation.rs` |
| `src/daemon.rs` | InferenceDaemon — main inference lifecycle | Loops: listen VAD → STT → LLM → TTS |
| `src/eyes.rs` | EyesDaemon — visual/status monitoring | Periodically observes system state |
| `src/control/` | Control API (HTTP/WebSocket) | `api.rs`, `state.rs`, `broadcast.rs`, `client.rs`, `mod.rs` |
| `src/mcp/` | Model Context Protocol integration | `mod.rs` (McpClient, McpToolDef, call_tool) |
| `src/tools/` | Tool implementations: time, screenshot, notifications, clipboard, open_app, web_search | `clipboard.rs`, `current_time.rs`, `open_app.rs`, `web_search.rs`, etc. |
| `src/agents/` | Agent delegation for complex tasks | `mod.rs`, `config.rs`, `session_manager.rs`, `session_events.rs` |
| `src/analysis/` | Identity analysis | `mod.rs`, `identity.rs` |
| `src/db/` | SQLite persistence: sessions, messages, user_profile, memories | `database.rs`, `mod.rs` |
| `src/dream/` | S-DREAM cold-path memory consolidation daemon | `mod.rs` (SDreamDaemon, SDreamConfig) |
| `src/config.rs` | Environment-based config | `Config::from_env()` |
| `src/memory/` | Extract persistent notes from conversation, archive outdated | `mod.rs` (extract_memories, build_memory_context) |
| `src/profile/` | User profile facts extraction | `mod.rs` |
| `src/i18n.rs` | Internationalization support | Language-specific strings |
| `src/remote/` | WebSocket server for remote audio streaming | `mod.rs`, `server.rs`, `protocol.rs` |
| `src/tui/` | Terminal UI (ratatui) | `app.rs`, `events.rs`, `input.rs`, `mod.rs`, `ui.rs` |
| `src/bin/acp_agent_chat.rs` | Debug/test TUI chat with ACP agent via JSON-RPC 2.0 over stdio | Run: `cargo run --bin acp_agent_chat` |
| `src/bin/test_stt_plus.rs` | Test binary for whisper-cpp-plus streaming functionality | Run: `cargo run --bin test_stt_plus --release` |
| `src/e2e_tests.rs` | End-to-end pipeline tests | Integration tests |

### Legacy Modules (do not extend)

- `src/stt/whisper.rs` — **DEPRECATED** — legacy whisper-rs wrapper; replaced by `whisper-cpp-plus` in `src/stt/mod.rs`
- `src/websocket_client.rs` — No longer needed
- `provider/` — Python LFM2.5-Audio server (not used)
- `src/tts/piper.rs` — Piper subprocess wrapper (kept for reference, not active)

**Do not extend legacy modules.** If you find code there, flag it for removal.

---

## Config File

Default configuration values live in `voicebot.pro.toml` (PRO) or `voicebot.dev.toml` (DEV), selected by the `VOICEBOT_ENV` environment variable. The file is also embedded into the binary, so a missing local file falls back to the compiled defaults.

Precedence (highest first):
1. Environment variables (existing names unchanged)
2. Explicit config file path (`VOICEBOT_CONFIG_FILE`)
3. Environment-specific config file (`voicebot.{env}.toml` in the current directory)
4. Embedded default config

Use `VOICEBOT_CONFIG_FILE=/path/to/custom.toml` to load an alternate file. Partial files are merged with embedded defaults, so only changed values need to be specified.

### Migration from single-voicebot.toml

If you have an existing `data/voicebot.db`, manually move it to `data/pro/voicebot.db`:

```bash
mkdir -p data/pro
mv data/voicebot.db data/pro/voicebot.db
mv data/archives data/pro/archives  # if exists
mv data/speaker.emb data/pro/speaker.emb  # if exists
```

Rename your `voicebot.toml` to `voicebot.pro.toml` and update data paths to `data/pro/`.

---

## Environment Variables (critical)

Read from `.env` (dotenvy loads automatically):

| Variable | Default | Description |
|----------|---------|-------------|
| `VOICEBOT_ENV` | `pro` | Environment: pro (default) or dev. Selects voicebot.{env}.toml and data/{env}/ paths. |
| `AUDIO_SAMPLE_RATE` | `16000` | Audio sample rate |
| `AUDIO_CHANNELS` | `1` | Audio channels |
| `VOICEBOT_LANGUAGE` | `en` | Language (`en` or `es`) |
| `STT_PROVIDER` | `whisper` | `whisper` (default) or `parakeet` |
| `WHISPER_MODEL` | `models/ggml-large-v3-turbo.bin` | Whisper GGML model path |
| `WHISPER_THREADS` | `0` | CPU threads (0 = auto) |
| `PARAKEET_MODEL_DIR` | — | Required when `STT_PROVIDER=parakeet`. Download ONNX from: https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx |
| `LLM_URL` | `http://127.0.0.1:8000` | LLM server URL (mlx-lm default; oMLX is 8001) |
| `LLM_MAX_TOKENS` | `1024` | Max tokens per response |
| `LLM_CONTEXT_TOKENS` | `8192` | Context window size |
| `LLM_CONSOLIDATION_THRESHOLD_PCT` | `80` | % threshold for consolidation |
| `LLM_SUMMARY_KEEP_TURNS` | `6` | Recent turns to keep after consolidation |
| `AVSPEECH_VOICE` | `"Jorge (Enhanced)"` | macOS AVSpeech voice name |
| `AVSPEECH_RATE` | `0.55` | Speech rate (0.0–1.0) |
| `SEARXNG_URL` | — | SearXNG base URL (enables web_search) |
| `SEARXNG_SECRET` | — | SearXNG bearer token |
| `WS_PORT` | `9090` | WebSocket server port |
| `S_DREAM_INTERVAL_SECS` | `3600` | Seconds between consolidation cycles (0 = disabled) |
| `S_DREAM_ON_IDLE` | `1` | Trigger consolidation when user is idle (1 = true) |
| `S_DREAM_IDLE_THRESHOLD_SECS` | `600` | Idle seconds before consolidation triggers |
| `S_DREAM_SCHEDULED_HOUR` | `3` | Scheduled daily hour (0-23); set empty to disable |
| `S_DREAM_L2_MIN_MESSAGES` | `50` | Min L2 messages before consolidation triggers |
| `S_DREAM_JSONL_DIR` | `data/{env}/archives` | Directory for archived JSONL consolidation files (default: data/{env}/archives) |

---

## Build Features & Dependencies

| Feature | Enables | Extra deps | Requirements |
|---------|---------|------------|--------------|
| (none) | Core pipeline | whisper-cpp-plus, reqwest, sqlx | — |
| `parakeet` | NVIDIA Parakeet STT (ONNX) | parakeet-rs | ParakeetTDT model files |
| `speech` | macOS SFSpeechRecognizer STT | speech | macOS only, microphone permission |
| `kokoro` | Kokoro ONNX TTS | kokorox | `brew install espeak-ng` |
| `tui` | Terminal UI | ratatui, crossterm | — |
| `remote` | WebSocket server | axum, tower | — |
| `speaker` | Speaker verification | sherpa-rs | `models/speaker_embedding.onnx` |
| `avspeech` | macOS AVSpeechSynthesizer | objc2*, block2 | macOS only |

**On macOS**: whisper-cpp-plus uses Metal by default (faster STT via metal feature). Model files: `models/ggml-large-v3-turbo.bin` + `models/*-encoder.mlmodelc` for CoreML encoder fallback.

---

## Code Style & Patterns

- **Error handling**: `anyhow::Result` with context strings; `thiserror` for custom types.
- **Logging**: `tracing` throughout (no println!); logs → `voicebot.log` when TUI active.
- **Async**: tokio runtime + channels (`mpsc`, `broadcast`) for inter-stage comms.
- **Cancellation**: `CancellationToken` (tokio-util) for barge-in support.
- **Serialization**: serde + serde_json.
- **Tool calling**: LLM uses `<tool_name: args>` syntax; parsed by ToolRegistry.

### When Adding Tools

1. Define tool schema in `src/tools/mod.rs` or dedicated module.
2. Implement handler returning `Result<String, Error>`.
3. Register in main pipeline's tool map.
4. Add doc comment explaining use case and limitations.

### Database Migrations

Use sqlx migrations:
```bash
sqlx migrate add <migration_name>
sqlx migrate run
```

Migrations live in `src/db/migrations/`.

---

## Testing

- **VAD/audio tests**: Use synthetic sine waves / silence (see `src/audio/` tests).
- **STT tests**: Skip if model file missing (`#[ignore]`). Uses `whisper-cpp-plus`.
- **TTS tests**: macOS requires voices installed; kokoro for Linux CI.
- **Parallel tests**: Use `temp-env` crate to safely override env vars.
- **Mock LLM**: Use `wiremock` crate for HTTP client tests.

Run specific test:
```bash
cargo test <test_name> -- --nocapture
```

### Debugging Binary

The `test_stt_plus` binary provides standalone STT testing without full pipeline:
```bash
cargo run --bin test_stt_plus --release
```

---

## Git Workflow

### Branch Strategy
- **Bugs/fixes**: Work directly on `main`. Small fixes, no feature branch needed.
- **New features**: Create a feature branch (`feature/<short-name>`). One feature per branch.

### Commit Messages
- **English only**, short descriptive text identifying the change. No lengthy explanations.
- Small, focused commits are preferred.
- Example: `feat: add speaker verification module` or `fix: silence VAD false positives`

### Feature Merge Process
1. Complete the feature in the feature branch.
2. Interactive rebase to squash related commits: `git rebase -i main`
3. Merge into main: `git checkout main && git merge --squash feature/<name>`
4. Delete the feature branch.

### Code Review
- **Local**: Review all code manually before committing (you).
- **CI/remote only**: When explicitly requested, allow the agent to commit, push, check CI logs, fix, and re-commit autonomously.
- Never auto-commit to main without explicit user instruction.

### Gitea Issues

Issues live on Gitea (`tesla.local:3000`). Use the Gitea MCP CLI for all issue operations (never `gh`, `tea` or raw `curl`).

#### Documenting Work as Issue Comments

Every time an agent completes an analysis, plan, or finishes work on a Gitea issue, it **must** leave the results as a comment on that issue. This creates an auditable trail and lets the user review work without checking commits.

**When posting comments:**

| Trigger | What to include |
|---------|----------------|
| Starting analysis | Brief scope statement + what you'll investigate |
| Completing a plan | Numbered steps, affected files/modules, estimated complexity |
| Finishing implementation | Summary of changes, files touched, commands run, test results |
| Fixing a bug | Root cause, fix applied, verification steps |
| Research/spike | Findings, options considered, recommendation |

**Comment format:**

```markdown
## Analysis / Plan / Results

[Brief description of the work performed]

### Changes
- File/module affected: what changed and why

### Verification
- `cargo test` result
- `cargo clippy` clean
- Manual testing notes (if applicable)

### Related
- Commands run: `cargo run --features tui`
```

**Workflow for issue-driven work:**
1. Fetch issue details from Gitea with ots MCP.
2. Mark issue as in progress by adding label `ongoing`.
3. Post initial comment with scope/plan into the issue.
4. Execute the work.
5. Post final comment with results (mandatory) in the issue.

Labels exist but no issue templates — the agent handles formatting naturally.

### Versioning
- Semver with pre-release state: `v<major>.<minor>.<patch>-<state><number>`
- States: `alpha`, `beta`, `rc`
- Example: `v0.1.13-alpha01`, `v0.1.0-beta.1`
- Tag on main after validated merge.

### Git Worktrees (Isolated Binary Model)
To avoid context switching for the human and resource collisions, use an isolated binary model:
- **Human Zone:** `/Users/danielvela/projects/ai/voicebot` (Main stable context). Validates and merges PRs.
- **AI Zone:** `/Users/danielvela/projects/ai/voicebot-ai` (Autonomous cycle zone). 
  - Agents MUST perform all work here.
  - Each issue requires its own worktree/branch: `git worktree add -b feature/issue-N /Users/danielvela/projects/ai/voicebot-ai`.
  - When a task is completed and a PR is opened, the worktree is cleared or moved to the next task.

---

## Common Workflows

### Running Development

```bash
# 1. Ensure .env exists (cp .env.example .env if not)
# 2. Start external LLM server (mlx-lm or oMLX)
# 3. Run voicebot
cargo run --features tui --release
```

### Adding a New Feature

1. Read this file first (architecture guidance).
2. Check existing tools/agents to avoid duplication.
3. If feature affects multiple modules, create integration test in `e2e_tests.rs`.
4. Update this file if you discover new conventions.

### Debugging Pipeline Latency

Check these stages:
- VAD sensitivity: `src/audio/vad.rs` (frame thresholds).
- STT provider choice: Whisper vs Parakeet (`STT_PROVIDER` env var).
- Whisper decoding: `src/stt/whisper.rs` (thread count, model size).
- Parakeet decoding: `src/stt/parakeet.rs` (ONNX Runtime, model size).
- LLM response time: External server config, context window size.
- TTS synthesis: `say` vs Kokoro vs AVSpeech backend choice.

Log with `RUST_LOG=trace cargo run` for detailed timing.

---

## References

- `LICENSE-VOICEBOT.md`: Trademark information
- `readme.md`: User-facing documentation
- `CONTRIBUTING.md`: Contributor guidelines
- `secondary-agent.md`: Secondary LLM orchestration design (Spanish)

## Repository Map

A full codemap is available at `codemap.md` in the project root.

Before working on any task, read `codemap.md` to understand:
- Project architecture and entry points
- Directory responsibilities and design patterns
- Data flow and integration points between modules

For deep work on a specific folder, also read that folder's `codemap.md`.

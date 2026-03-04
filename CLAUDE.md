# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build --release      # Release build
cargo run                  # Development run
cargo run --release        # Production run
cargo test                 # Run all tests
cargo fmt                  # Format code
cargo clippy               # Lint code

# List available audio devices
cargo run -- --list-devices
# or
LIST_AUDIO_DEVICES=1 cargo run
```

To run a single test:
```bash
cargo test <test_name>
cargo test -p voicebot <test_name>
```

## Architecture

Voicebot is a mono-user voice AI chatbot in Rust using Speech-to-Speech (S2S) models. Data flows:

1. **Input**: `Microphone → AudioCapture (CPAL) → VAD → AudioBuffer → SessionManager → S2SAdapter → Model`
2. **Tool execution**: `S2S Model → ToolRouter → ToolRegistry | McpServer | AgentManager → result back to model`
3. **Output**: `Model → AudioTransformer (resampling) → AudioOutput → Speaker`
4. **Persistence**: `SessionManager ↔ SQLite (sqlx, async, connection pool)`

### Key Modules

**`src/audio/`** — Audio pipeline
- `audio_capture.rs`: CPAL microphone input; normalizes I16/U16/F32 to f32 (-1.0..1.0)
- `vad.rs`: Energy-threshold VAD; emits `SpeechStart/Speech/SpeechEnd/Silence` states
- `buffer.rs`: Circular VecDeque buffer accumulating samples with duration tracking
- `audio_transform.rs`: Rubato-based resampling between sample rates
- `output.rs`: CPAL speaker playback

**`src/s2s/`** — Speech-to-Speech model layer (Adapter pattern)
- `adapter.rs`: `S2SAdapter` dispatches to pluggable models via the `S2SModel` trait
- `models/`: Stubs for LLaMA-Omni, Moshi, Ultravox, LFM2.5-Audio
- `S2SRequest` carries `audio: Vec<f32>`, `context`, optional `tools`; `S2SResponse` returns audio + optional `tool_calls`

**`src/tools/`** — Tool execution (Router → Registry pattern)
- `router.rs`: `ToolRouter` dispatches calls in priority order: Built-in → MCP → Agents
- `registry.rs`: `ToolRegistry` with `Tool` trait for built-in tools
- `builtin/`: `FileOperations`, `WebSearch`, `SystemInfo`

**`src/session/`** — Conversation state
- `manager.rs`: `SessionManager` with UUID-based sessions; persists to DB
- `context.rs`: `ConversationContext` holding message history; roles: User/Assistant/System/Tool

**`src/mcp/`** — Model Context Protocol integration
- `server.rs`: `McpServer` manages MCP tools separately from built-ins

**`src/agents/`** — External agent integrations
- `manager.rs`: `AgentManager` (currently supports OpenClaw)

**`src/db/`** — SQLite persistence (sqlx, max 5 connections, auto-migration on startup)
- Tables: `sessions`, `messages`, `config`

**`src/config.rs`** — Environment-based config; key vars: `AUDIO_SAMPLE_RATE` (default 16000), `AUDIO_CHANNELS` (default 1), `AUDIO_DEVICE`, `LIST_AUDIO_DEVICES`

### Design Patterns
- **Adapter pattern** for S2S models — implement `S2SModel` trait to add a new model
- **Registry + Router** for tools — register via `ToolRegistry`, routing priority: builtin → MCP → agents
- **`anyhow::Result`** for error propagation with context; `thiserror` for custom error types
- **`tracing`** for structured logging throughout

### Testing Approach
- Use in-memory SQLite for DB tests: `"file:memdb?mode=memory&cache=shared"`
- Generate synthetic audio (sine waves / silence) for VAD and buffer tests
- Mock the `S2SModel` trait for adapter tests
- See `testing_strategy.md` for the full 5-layer integration test plan

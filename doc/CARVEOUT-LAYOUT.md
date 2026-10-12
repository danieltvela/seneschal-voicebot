# Seneschal — Workspace Layout Spec

> Cargo workspace crate layout and file migration map for the modular carve-out.  
> This document is the blueprint for Phases 4–8. All changes are `git mv` only — no logic rewrites.

## Workspace Root `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
    "crates/seneschal-core",
    "crates/seneschal-mcp",
    "crates/seneschal-agents",
    "crates/seneschal-plugins",
    "crates/seneschal-control",
    "crates/seneschal-remote",
    "crates/seneschal-search",
    "crates/seneschal-memory",
    "crates/seneschal-tools-core",
    "crates/seneschal-classifier",
    "crates/seneschal-extras",
    "crates/seneschal-tui",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
```

Each crate is opt-in from the main binary via Cargo features.

## Crate Templates

### `seneschal-core` (voice pipeline, no workspace deps)

```toml
[package]
name = "seneschal-core"
version.workspace = true
edition.workspace = true

[dependencies]
tokio = { version = "1", features = ["full"] }
async-channel = "2"
cpal = "0.15"
whisper-cpp-plus = { ... }
rubato = { ... }
reqwest = { ... }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# ... (all current core deps)
```

- **No dependency on any other workspace crate.**
- Exposes: `pub use` for `AudioChunk`, `AudioOutput`, `AudioCapture`, `AudioTransformer`, `WhisperSTTVAD`, `SttProvider`, `OpenAIClient`, `LlmProvider`, `OpenAiLlmProvider`, `TtsEngine`, `SentenceSplitter`, `PipelineState`, `PipelineEvents`, `PipelineFrame`, all pipeline actors.

### `seneschal-mcp` (standalone)

```toml
[package]
name = "seneschal-mcp"
deps: tokio, serde, serde_json, reqwest
```

### `seneschal-agents` (standalone)

```toml
[package]
name = "seneschal-agents"
deps: tokio, serde, serde_json, reqwest, dashmap, portable-pty
```

### `seneschal-plugins` (depends on seneschal-mcp + seneschal-agents)

```toml
[package]
name = "seneschal-plugins"
[dependencies]
seneschal-mcp = { path = "../seneschal-mcp" }
seneschal-agents = { path = "../seneschal-agents" }
```

### Other crates are standalone leaf nodes.

### Main binary `Cargo.toml` feature flags

```toml
[features]
default = ["tools-core", "memory"]
full = ["mcp", "agents", "plugins", "control", "remote", "tools-core", "memory", "classifier", "extras", "tui"]
mcp = ["seneschal-mcp"]
agents = ["seneschal-agents"]
plugins = ["seneschal-plugins"]
control = ["seneschal-control"]
remote = ["seneschal-remote"]
tools-core = ["seneschal-tools-core", "seneschal-search"]
memory = ["seneschal-memory"]
classifier = ["seneschal-classifier"]
extras = ["seneschal-extras"]
tui = ["seneschal-tui"]

[dependencies]
seneschal-core = { path = "crates/seneschal-core" }
seneschal-mcp = { path = "crates/seneschal-mcp", optional = true }
seneschal-agents = { path = "crates/seneschal-agents", optional = true }
seneschal-plugins = { path = "crates/seneschal-plugins", optional = true }
seneschal-control = { path = "crates/seneschal-control", optional = true }
seneschal-remote = { path = "crates/seneschal-remote", optional = true }
seneschal-search = { path = "crates/seneschal-search", optional = true }
seneschal-memory = { path = "crates/seneschal-memory", optional = true }
seneschal-tools-core = { path = "crates/seneschal-tools-core", optional = true }
seneschal-classifier = { path = "crates/seneschal-classifier", optional = true }
seneschal-extras = { path = "crates/seneschal-extras", optional = true }
seneschal-tui = { path = "crates/seneschal-tui", optional = true }
```

## Exhaustive File Migration Map

Every `.rs` file in `src/` is assigned to exactly one destination. Files marked 🔴 are deleted in Phase 6.

### Core Pipeline → `crates/seneschal-core/src/`

| Source | Destination |
|--------|-------------|
| `src/audio/mod.rs` | `crates/seneschal-core/src/audio/mod.rs` |
| `src/audio/audio_capture.rs` | `crates/seneschal-core/src/audio/audio_capture.rs` |
| `src/audio/audio_transform.rs` | `crates/seneschal-core/src/audio/audio_transform.rs` |
| `src/audio/buffer.rs` | `crates/seneschal-core/src/audio/buffer.rs` |
| `src/audio/ambient_buffer.rs` | `crates/seneschal-core/src/audio/ambient_buffer.rs` |
| `src/audio/output.rs` | `crates/seneschal-core/src/audio/output.rs` |
| `src/audio/filler.rs` | `crates/seneschal-core/src/audio/filler.rs` |
| `src/audio/speaker.rs` | `crates/seneschal-core/src/audio/speaker.rs` |
| `src/stt/mod.rs` | `crates/seneschal-core/src/stt/mod.rs` |
| `src/stt/provider.rs` | `crates/seneschal-core/src/stt/provider.rs` |
| `src/stt/whisper.rs` | `crates/seneschal-core/src/stt/whisper.rs` |
| `src/stt/no_speech_gate.rs` | `crates/seneschal-core/src/stt/no_speech_gate.rs` |
| `src/stt/parakeet.rs` | `crates/seneschal-core/src/stt/parakeet.rs` |
| `src/stt/speech_recognizer.rs` | `crates/seneschal-core/src/stt/speech_recognizer.rs` |
| `src/llm/mod.rs` | `crates/seneschal-core/src/llm/mod.rs` |
| `src/llm/client.rs` | `crates/seneschal-core/src/llm/client.rs` |
| `src/llm/session.rs` | `crates/seneschal-core/src/llm/session.rs` |
| `src/llm/provider.rs` | `crates/seneschal-core/src/llm/provider.rs` |
| `src/llm/manager.rs` | `crates/seneschal-core/src/llm/manager.rs` |
| `src/tts/mod.rs` | `crates/seneschal-core/src/tts/mod.rs` |
| `src/tts/sentence.rs` | `crates/seneschal-core/src/tts/sentence.rs` |
| `src/tts/avspeech.rs` | `crates/seneschal-core/src/tts/avspeech.rs` |
| `src/tts/kokoro.rs` | `crates/seneschal-core/src/tts/kokoro.rs` |
| `src/pipeline/mod.rs` | `crates/seneschal-core/src/pipeline/mod.rs` |
| `src/pipeline/frames.rs` | `crates/seneschal-core/src/pipeline/frames.rs` |
| `src/pipeline/fsm.rs` | `crates/seneschal-core/src/pipeline/fsm.rs` |
| `src/pipeline/state.rs` | `crates/seneschal-core/src/pipeline/state.rs` |
| `src/pipeline/llm_task.rs` | `crates/seneschal-core/src/pipeline/llm_task.rs` |
| `src/pipeline/sen_task.rs` | `crates/seneschal-core/src/pipeline/sen_task.rs` |
| `src/pipeline/tts_task.rs` | `crates/seneschal-core/src/pipeline/tts_task.rs` |
| `src/pipeline/consolidation.rs` | `crates/seneschal-core/src/pipeline/consolidation.rs` |
| `src/config.rs` (core subset) | `crates/seneschal-core/src/config.rs` (CoreConfig only) |

### Search → `crates/seneschal-search/src/`

| Source | Destination |
|--------|-------------|
| `src/search/mod.rs` | `crates/seneschal-search/src/mod.rs` |
| `src/search/brave.rs` | `crates/seneschal-search/src/brave.rs` |
| `src/search/tavily.rs` | `crates/seneschal-search/src/tavily.rs` |
| `src/search/exa.rs` | `crates/seneschal-search/src/exa.rs` |
| `src/search/searxng.rs` | `crates/seneschal-search/src/searxng.rs` |
| `src/search/tests.rs` | `crates/seneschal-search/src/tests.rs` |

### MCP → `crates/seneschal-mcp/src/`

| Source | Destination |
|--------|-------------|
| `src/mcp/mod.rs` | `crates/seneschal-mcp/src/mod.rs` |
| `src/mcp/config.rs` | `crates/seneschal-mcp/src/config.rs` |
| `src/mcp/transport.rs` | `crates/seneschal-mcp/src/transport.rs` |

### Agents → `crates/seneschal-agents/src/`

| Source | Destination |
|--------|-------------|
| `src/agents/mod.rs` | `crates/seneschal-agents/src/mod.rs` |
| `src/agents/config.rs` | `crates/seneschal-agents/src/config.rs` |
| `src/agents/session_manager.rs` | `crates/seneschal-agents/src/session_manager.rs` |
| `src/agents/session_events.rs` | `crates/seneschal-agents/src/session_events.rs` |
| `src/agents/hermes_events.rs` | `crates/seneschal-agents/src/hermes_events.rs` |
| `src/agents/opencode_events.rs` | `crates/seneschal-agents/src/opencode_events.rs` |
| `src/agents/opencode_transport.rs` | `crates/seneschal-agents/src/opencode_transport.rs` |
| `src/agent_session.rs` | `crates/seneschal-agents/src/agent_session.rs` |

### Plugins → `crates/seneschal-plugins/src/`

| Source | Destination |
|--------|-------------|
| `src/plugins/mod.rs` | `crates/seneschal-plugins/src/mod.rs` |
| `src/plugins/manager.rs` | `crates/seneschal-plugins/src/manager.rs` |
| `src/plugins/manifest.rs` | `crates/seneschal-plugins/src/manifest.rs` |
| `src/plugins/mcp_spawner.rs` | `crates/seneschal-plugins/src/mcp_spawner.rs` |
| `src/plugins/agent_bridge.rs` | `crates/seneschal-plugins/src/agent_bridge.rs` |
| `src/plugins/prompt_injection.rs` | `crates/seneschal-plugins/src/prompt_injection.rs` |
| `src/plugins/config_overrides.rs` | `crates/seneschal-plugins/src/config_overrides.rs` |

### Control → `crates/seneschal-control/src/`

| Source | Destination |
|--------|-------------|
| `src/control/mod.rs` | `crates/seneschal-control/src/mod.rs` |
| `src/control/api.rs` | `crates/seneschal-control/src/api.rs` |
| `src/control/state.rs` | `crates/seneschal-control/src/state.rs` |
| `src/control/broadcast.rs` | `crates/seneschal-control/src/broadcast.rs` |
| `src/control/client.rs` | `crates/seneschal-control/src/client.rs` |

### Remote → `crates/seneschal-remote/src/`

| Source | Destination |
|--------|-------------|
| `src/remote/mod.rs` | `crates/seneschal-remote/src/mod.rs` |
| `src/remote/server.rs` | `crates/seneschal-remote/src/server.rs` |
| `src/remote/protocol.rs` | `crates/seneschal-remote/src/protocol.rs` |
| `src/remote/tests.rs` | `crates/seneschal-remote/src/tests.rs` |

### Memory (DB + dream + memory + profile) → `crates/seneschal-memory/src/`

| Source | Destination |
|--------|-------------|
| `src/db/mod.rs` | `crates/seneschal-memory/src/db/mod.rs` |
| `src/db/database.rs` | `crates/seneschal-memory/src/db/database.rs` |
| `src/dream/mod.rs` | `crates/seneschal-memory/src/dream/mod.rs` |
| `src/memory/mod.rs` | `crates/seneschal-memory/src/memory/mod.rs` |
| `src/profile/mod.rs` | `crates/seneschal-memory/src/profile/mod.rs` |

### Tools Core → `crates/seneschal-tools-core/src/`

| Source | Destination |
|--------|-------------|
| `src/tools/mod.rs` (Tool trait + ToolRegistry) | `crates/seneschal-tools-core/src/mod.rs` |
| `src/tools/current_time.rs` | `crates/seneschal-tools-core/src/current_time.rs` |
| `src/tools/clipboard.rs` | `crates/seneschal-tools-core/src/clipboard.rs` |
| `src/tools/read_file.rs` | `crates/seneschal-tools-core/src/read_file.rs` |
| `src/tools/take_screenshot.rs` | `crates/seneschal-tools-core/src/take_screenshot.rs` |
| `src/tools/open_app.rs` | `crates/seneschal-tools-core/src/open_app.rs` |
| `src/tools/quick_search.rs` | `crates/seneschal-tools-core/src/quick_search.rs` |
| `src/tools/run_shell.rs` | `crates/seneschal-tools-core/src/run_shell.rs` |

### Classifier → `crates/seneschal-classifier/src/`

| Source | Destination |
|--------|-------------|
| `src/classifier/mod.rs` | `crates/seneschal-classifier/src/mod.rs` |
| `src/classifier/heuristic.rs` | `crates/seneschal-classifier/src/heuristic.rs` |
| `src/classifier/keyword.rs` | `crates/seneschal-classifier/src/keyword.rs` |
| `src/classifier/pipeline.rs` | `crates/seneschal-classifier/src/pipeline.rs` |
| `src/classifier/fallback.rs` | `crates/seneschal-classifier/src/fallback.rs` |

### Extras → `crates/seneschal-extras/src/`

| Source | Destination |
|--------|-------------|
| `src/daemon.rs` | `crates/seneschal-extras/src/daemon.rs` |
| `src/eyes.rs` | `crates/seneschal-extras/src/eyes.rs` |
| `src/screen_capture.rs` | `crates/seneschal-extras/src/screen_capture.rs` |
| `src/device_monitor.rs` | `crates/seneschal-extras/src/device_monitor.rs` |
| `src/i18n.rs` | `crates/seneschal-extras/src/i18n.rs` |
| `src/analysis/mod.rs` | `crates/seneschal-extras/src/analysis/mod.rs` |
| `src/analysis/identity.rs` | `crates/seneschal-extras/src/analysis/identity.rs` |
| `src/analysis/tests.rs` | `crates/seneschal-extras/src/analysis/tests.rs` |
| `src/bin/acp_agent_chat.rs` | `crates/seneschal-extras/src/bin/acp_agent_chat.rs` |
| `src/tools/noop.rs` | `crates/seneschal-extras/src/tools/noop.rs` |
| `src/tools/web_search.rs` | `crates/seneschal-extras/src/tools/web_search.rs` |
| `src/tools/open_terminal.rs` | `crates/seneschal-extras/src/tools/open_terminal.rs` |
| `src/tools/apple_events.rs` | `crates/seneschal-extras/src/tools/apple_events.rs` |
| `src/tools/deep_research.rs` | `crates/seneschal-extras/src/tools/deep_research.rs` |
| `src/tools/run_agent.rs` | `crates/seneschal-extras/src/tools/run_agent.rs` |
| `src/tools/recover_historical_context.rs` | `crates/seneschal-extras/src/tools/recover_historical_context.rs` |
| `src/tools/switch_plugin.rs` | `crates/seneschal-extras/src/tools/switch_plugin.rs` |
| `src/tools/prompt_build.rs` | `crates/seneschal-extras/src/tools/prompt_build.rs` |
| `src/tools/conversation_mode.rs` | `crates/seneschal-extras/src/tools/conversation_mode.rs` |
| `src/tools/mcp_tool.rs` | `crates/seneschal-extras/src/tools/mcp_tool.rs` |
| `src/tools/subtask.rs` | `crates/seneschal-extras/src/tools/subtask.rs` |

### TUI → `crates/seneschal-tui/src/`

| Source | Destination |
|--------|-------------|
| `src/tui/mod.rs` | `crates/seneschal-tui/src/mod.rs` |
| `src/tui/app.rs` | `crates/seneschal-tui/src/app.rs` |
| `src/tui/ui.rs` | `crates/seneschal-tui/src/ui.rs` |
| `src/tui/events.rs` | `crates/seneschal-tui/src/events.rs` |
| `src/tui/input.rs` | `crates/seneschal-tui/src/input.rs` |
| `src/tui/acp_panel.rs` | `crates/seneschal-tui/src/acp_panel.rs` |

### Files Staying in Binary Root (`src/`)

| Source | Destination | Notes |
|--------|-------------|-------|
| `src/main.rs` | `src/main.rs` (stays) | Imports from all crates via features |
| `src/lib.rs` | `src/lib.rs` (stays) | Re-exports |
| `src/config.rs` | `src/config.rs` (stays, recortado) | Composes CoreConfig + extended config |
| `src/e2e_tests.rs` | `src/e2e_tests.rs` (stays) | Integration tests |

### 🔴 Deleted (Phase 6)

| Source | Reason |
|--------|--------|
| `src/tts/piper.rs` | Dead code; not declared in `tts/mod.rs` |
| `src/classifier/embedding.rs` | Empty feature `classifier-embedding`; always panics |
| `src/classifier/logistic.rs` | Same as embedding.rs |
| `src/bin/bench_pipeline.rs.bak` | Stale backup in source tree |

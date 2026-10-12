# Module Boundaries

> **Estado:** 🟢 SALVAR = keep in core, 🟡 AISLAR = separate crate, 🔴 DESCARTAR = remove

| Directory | Purpose | Estado | Doc |
|-----------|---------|--------|-----|
| `src/audio/` | Audio pipeline: capture, VAD, resampling, playback, filler, speaker | 🟢 SALVAR | [audio-internals.md](audio-internals.md) |
| `src/stt/` | Provider trait + Whisper + Parakeet + SFSpeechRecognizer implementations. 16kHz f32 mono. | 🟢 SALVAR | [ARCHITECTURE.md](ARCHITECTURE.md) |
| `src/llm/` | HTTP client to `/v1/chat/completions`, session management | 🟢 SALVAR | [llm-provider.md](llm-provider.md) |
| `src/tts/` | `avspeech.rs` (macOS AVSpeech), `sentence.rs` (boundary splitting), `kokoro.rs` (ONNX) | 🟢 SALVAR | [ARCHITECTURE.md](ARCHITECTURE.md) |
| `src/pipeline/` | Pipeline orchestration with FSM | 🟢 SALVAR | [MAIN_PROCESS.md](MAIN_PROCESS.md), [PROCESS_ARCHITECTURE.md](PROCESS_ARCHITECTURE.md) |
| `src/config.rs` | Environment-based config | 🟢 SALVAR (dividir) | [env-vars.md](env-vars.md), [config.md](config.md) |
| `src/db/` | SQLite persistence: sessions, messages, user_profile, memories, FTS5 | 🟡 AISLAR (`seneschal-memory`) | [s-dream-format.md](s-dream-format.md) |
| `src/memory/` | Extract persistent notes from conversation, archive outdated | 🟡 AISLAR (`seneschal-memory`) | [s-dream-format.md](s-dream-format.md) |
| `src/profile/` | User profile facts extraction | 🟡 AISLAR (`seneschal-memory`) | [s-dream-format.md](s-dream-format.md) |
| `src/dream/` | S-DREAM cold-path memory consolidation daemon | 🟡 AISLAR (`seneschal-memory`) | [s-dream-format.md](s-dream-format.md) |
| `src/search/` | Pluggable web search providers (Brave, Tavily, Exa, SearXNG) | 🟡 AISLAR (`seneschal-search`) | [search-providers.md](search-providers.md) |
| `src/mcp/` | Model Context Protocol integration | 🟡 AISLAR (`seneschal-mcp`) | [ARCHITECTURE-MCP-LAYER.md](ARCHITECTURE-MCP-LAYER.md) |
| `src/tools/` | Tool implementations for LLM-callable actions | 🟡 AISLAR (dividir en `seneschal-tools-core` + `seneschal-extras`) | [TOOLS.md](TOOLS.md) |
| `src/agents/` | Agent delegation for complex tasks | 🟡 AISLAR (`seneschal-agents`) | [agents-internal.md](agents-internal.md) |
| `src/plugins/` | Plugin system (runtime-swappable feature packs) | 🟡 AISLAR (`seneschal-plugins`) | [plugins-internal.md](plugins-internal.md) |
| `src/classifier/` | Intent classifier cascade (heuristic → keyword → fallback) | 🟡 AISLAR (`seneschal-classifier`) | [classifier.md](classifier.md) |
| `src/control/` | Control API (HTTP/SSE) | 🟡 AISLAR (`seneschal-control`) | [MAIN_PROCESS.md](MAIN_PROCESS.md), [IOS_COMPANION.md](IOS_COMPANION.md) |
| `src/remote/` | WebSocket server for remote audio streaming | 🟡 AISLAR (`seneschal-remote`) | [APPLE_WATCH_CLIENT.md](APPLE_WATCH_CLIENT.md), [IOS_COMPANION.md](IOS_COMPANION.md) |
| `src/tui/` | Terminal UI (ratatui), status-only | 🟡 AISLAR (`seneschal-tui`) | readme.md §TUI Key Bindings |
| `src/daemon.rs` | InferenceDaemon — proactive reasoning loop | 🟡 AISLAR (`seneschal-extras`) | [MAIN_PROCESS.md](MAIN_PROCESS.md) |
| `src/eyes.rs` | EyesDaemon — screenshot + vision LLM | 🟡 AISLAR (`seneschal-extras`) | [MAIN_PROCESS.md](MAIN_PROCESS.md) |
| `src/screen_capture.rs` | macOS screenshot utility | 🟡 AISLAR (`seneschal-extras`) | — |
| `src/device_monitor.rs` | Audio device hotplug monitor | 🟡 AISLAR (`seneschal-extras`) | — |
| `src/agent_session.rs` | PTY-based visible agent sessions | 🟡 AISLAR (`seneschal-extras`) | [agents-internal.md](agents-internal.md) |
| `src/i18n.rs` | Multilingual notification templates | 🟡 AISLAR (`seneschal-extras`) | — |
| `src/analysis/` | Identity analysis framework | 🟡 AISLAR (pendiente decisión) | — |
| `src/bin/acp_agent_chat.rs` | Debug/test TUI chat with ACP agent | 🟡 AISLAR (`seneschal-extras`) | — |

## Legacy / Dead Code (🔴 DESCARTAR)

| File | Reason | Action |
|------|--------|--------|
| `src/tts/piper.rs` | Never declared in `tts/mod.rs`; never compiled | 🔴 ELIMINAR |
| `src/classifier/embedding.rs` | Feature `classifier-embedding` is empty in Cargo.toml; always returns error | 🔴 ELIMINAR |
| `src/classifier/logistic.rs` | Same as embedding.rs | 🔴 ELIMINAR |
| `src/bin/bench_pipeline.rs.bak` | Stale backup file in source tree | 🔴 ELIMINAR |
| `src/stt/whisper.rs` | DEPRECATED legacy whisper-rs wrapper; replaced by whisper-cpp-plus in `src/stt/mod.rs` | Descartar en carve-out |
| `provider/` | Python LFM2.5-Audio server (not used) | Eliminar |

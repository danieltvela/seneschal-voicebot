# Module Boundaries

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

## Legacy Modules (do not extend)

- `src/stt/whisper.rs` — **DEPRECATED** — legacy whisper-rs wrapper; replaced by `whisper-cpp-plus` in `src/stt/mod.rs`
- `src/websocket_client.rs` — No longer needed
- `provider/` — Python LFM2.5-Audio server (not used)
- `src/tts/piper.rs` — Piper subprocess wrapper (kept for reference, not active)

**Do not extend legacy modules.** If you find code there, flag it for removal.
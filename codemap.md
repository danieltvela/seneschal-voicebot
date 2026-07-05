# Voicebot — Root Level Codemap

## Responsibility

Entry point, system initialization, and global architecture. `main.rs` bootstraps all subsystems, spawns the four permanent pipeline actors, and runs the main audio loop. Supporting modules provide configuration loading, background daemons, visual awareness, device monitoring, screen capture, and internationalization.

## Design

### `src/main.rs` — Binary Entry Point

**Runtime model:**
- With `avspeech` feature: main thread runs `CFRunLoopRunInMode` loop to deliver AVSpeechSynthesizer buffer callbacks; `async_main()` runs on a spawned tokio thread.
- Without `avspeech`: `#[tokio::main]` runs `async_main()` directly.

**`async_main()` initialization sequence:**
1. Load `.env` via `dotenvy::dotenv()`
2. Determine `VoicebotEnv` (pro/dev) from `VOICEBOT_ENV`
3. Initialize `tracing` subscriber (file output when TUI active, stdout otherwise)
4. Load `Config::from_env()` (embedded defaults → config file → env var overrides)
5. Handle shortcuts: `--list-devices`, `--list-voices`, `--dream`
6. Create `PluginManager` from config
7. Create proactive event channel (`mpsc::channel::<ProactiveEvent>(32)`)
8. Create secondary LLM client (`OpenAiLlmProvider`) if `SECONDARY_LLM_URL` is set
9. Build `ToolRegistry` — register all tools conditionally based on config flags
10. Register agent tools (`RunAgentTool`) from `AgentRegistry::from_env()`
11. ACP pre-warm: spawn warmup tasks for each ACP agent
12. ACP keep-alive daemon: spawn `AcpKeepAliveDaemon` if enabled
13. MCP tool registration: spawn MCP servers, register prefixed tools
14. Plugin activation: activate startup plugin, spawn MCP servers, register agent tools
15. Plugin switch channel: `mpsc::channel::<PluginSwitchEvent>(8)`
16. Database initialization: `Database::new()`, load session, system prompt, profile, memories
17. Build composite system prompt via `build_system_prompt()`
18. Create `LlmSession::from_history()` with loaded context
19. Self-managed LLM: if `LLM_SELF_MANAGED`, spawn and supervise LLM process
20. Create primary LLM client via `create_provider()`
21. Spawn `InferenceDaemon` if `daemon_enabled`
22. Spawn `EyesDaemon` if `eyes_interval_secs > 0` and secondary LLM exists
23. Create STT provider via `create_provider()`
24. Initialize `ContextLens` + `IdentityAnalyzer` (speaker verification)
25. Create `AmbientBuffer` for non-user speech context
26. Create TTS engine (AvSpeech or Kokoro)
27. Create `AudioOutput` and `AudioCapture`
28. Create pipeline channels: `sentences_tx/rx`, `llm_tx/rx`, `transcript_tx/rx`
29. Spawn four pipeline tasks: `llm_task`, `sen_task`, `tts_task`, `consolidation_task`
30. Spawn TUI task (if feature enabled)
31. Spawn remote WebSocket server (if feature enabled)
32. Spawn control API server (if feature enabled)
33. Startup consolidation if context exceeds idle threshold
34. Send startup greeting via `transcript_tx`
35. Spawn device monitor if configured
36. Enter main audio loop

**Main audio loop** (`tokio::select!`):
- `rx.recv()` — Audio chunk from capture stream → resample → STT provider → handle `SpeechEvent`
- `proactive_rx.recv()` — Background events (agent results, L1 saturation, ACP permissions, inference daemon)
- `plugin_switch_rx.recv()` — Plugin activation/deactivation between turns
- `ctrl_c` — Graceful shutdown (barge-in, play_cancel, shutdown flag)

**`run_dream_mode()`** — Standalone S-DREAM consolidation: `cargo run -- --dream`. Creates minimal dependencies, runs one `SDreamDaemon::run_once()` cycle, exits.

**`map_answer_to_outcome()`** — Maps spoken yes/no transcript to ACP permission outcome string (`"allow_once"` or `"reject_once"`).

### `src/lib.rs` — Library Crate Root

Re-exports public API surface. All modules are `pub` except `control` which is internal (re-exported via `pub mod control_client`). Key re-exports:
- `AudioBuffer`, `AudioOutput`, `Config`, `Database`, `OpenAIClient`
- `LlmProvider`, `LlmSession`, `OpenAiLlmProvider`
- `SpeechEvent`, `SttProvider`, `WhisperSTTVAD`, `create_provider`
- `SentenceSplitter`

### `src/config.rs` — Configuration

**`Config` struct** — 60+ fields covering audio, VAD, STT, LLM, TTS, context consolidation, agent delegation, inference daemon, EYES, secondary LLM, shell tool, web search, speaker verification, conversation mode, MCP, remote device, self-managed LLM, persistence, S-DREAM, Apple events, plugins.

**Loading precedence** (highest first):
1. Environment variables (`apply_env_overrides()`)
2. Explicit config file path (`VOICEBOT_CONFIG_FILE`)
3. Environment-specific config file (`voicebot.{env}.toml`)
4. Embedded default config (`DEFAULT_CONFIG_TOML = include_str!("../voicebot.pro.toml")`)

**`merge_toml()`** — Recursive deep merge of toml::Value tables (user config overwrites embedded defaults).

**`VoicebotEnv`** — `Pro` (default) or `Dev`, selected by `VOICEBOT_ENV`. Determines data directory path (`data/pro/` vs `data/dev/`).

### `src/daemon.rs` — Background Daemons

**`InferenceDaemon`** — Periodic "is there anything worth saying?" loop:
- Sleeps `interval_secs`, then calls `llm_client.complete_short()` with daemon system prompt
- LLM must return `NOTHING` to suppress output; any other text is sent as `ProactiveEvent::InferenceDaemon`
- Skips tick if proactive channel is full

**`AcpKeepAliveDaemon`** — Periodic ping of idle ACP agent sessions:
- Iterates configured ACP agents, calls `manager.prewarm_agent()` on available sessions
- Keeps processes warm without interrupting active ones

**`build_daemon_system_prompt()`** — Appends strict rules to base system prompt: respond `NOTHING` if nothing important, only intervene for urgent/clearly useful messages.

### `src/eyes.rs` — Visual Awareness

**`EyesDaemon`** — Periodic screen capture + vision LLM analysis:
- Every `interval_secs`: capture screen via `screen_capture::capture_screen()`
- Encode as base64 data URL
- Call `vision_client.complete_multimodal(data_url, EYES_PROMPT)`
- Parse structured response: `warn_user: true|false` + `message: <text>`
- If `warn_user` is true: send `ProactiveEvent::AgentResult` for main LLM to reformulate

**`parse_eyes_response()`** — Line-by-line parser for `warn_user:` and `message:` fields. Returns `Some(message)` only when `warn_user: true` and message is non-empty.

### `src/device_monitor.rs` — Audio Device Monitoring

**`spawn()`** — Background task that polls `cpal` input devices every `poll_interval_secs`:
- Checks if configured device name matches any available input device
- On unavailable → available transition: sends startup greeting via `transcript_tx`
- Respects `shutdown` flag for graceful exit

**`is_device_available()`** — Iterates `cpal::default_host().input_devices()`, matches by substring (case-insensitive), verifies `default_input_config()` is Ok.

### `src/screen_capture.rs` — Screen Capture Utility

**`capture_screen()`** — Spawns `screencapture -x -t png /tmp/voicebot_screenshot.png`, reads PNG bytes. On macOS only.

**`open_screen_recording_settings()`** — Opens macOS System Settings → Privacy & Security → Screen Recording via `open -g x-apple.systempreferences:...`.

**`diagnose()`** — Error diagnostic: detects SSH sessions (`SSH_TTY`/`SSH_CONNECTION` env vars) and permission errors ("could not create image from display"), returns user-friendly fix instructions.

### `src/i18n.rs` — Internationalization

**`get_notification(key, lang)`** — Returns `&'static str` notification templates for Spanish (`"es"`) and English (`"en"`). Keys:
- `first_launch` — Extended self-introduction (capabilities, limitations, interaction model)
- `startup` — Brief greeting with `{time_str}` and `{date_str}` placeholders
- `background_task_done` — Background task completion with `{task}` and `{result}`
- `acp_permission` — ACP agent permission request with `{question}` and `{opts_str}`
- `reorganize_memory` — Memory reorganization notice
- `memory_reorganized` — Memory reorganization complete with `{now}`
- `l1_saturated` — L1 saturation alert with `{total_chars}` and `{threshold}`

## Flow

### System Startup Sequence

```
main() → async_main()
  → dotenvy::dotenv()
  → Config::from_env() (embedded defaults → file merge → env overrides)
  → ToolRegistry construction (conditional registration per config flag)
  → Database initialization (load session, history, profile, memories)
  → build_system_prompt() → LlmSession::from_history()
  → Pipeline channel creation (sentences, llm, transcript)
  → tokio::spawn: llm_task, sen_task, tts_task, consolidation_task
  → Startup consolidation (if context exceeds idle threshold)
  → transcript_tx.send(StartupGreeting)
  → Main audio loop (select: audio chunk, proactive event, plugin switch, ctrl_c)
```

### Audio Loop Per-Chunk Flow

```
AudioCapture → rx.recv() → AudioChunk { samples, sample_rate, channels }
  → Mono downmix (if multichannel)
  → resample_nearest() to config.sample_rate
  → stt_provider.process_audio(&mono, &stt_tx)
  → stt_rx.try_recv() → SpeechEvent:
    .SpeechStart → fire barge_in, clear buffer, start timing
    .Speech(partial) → push to buffer, log partial
    .SpeechEnd(final_text) → push to buffer, check duration threshold,
        → Speaker verification (async spawn)
        → Ambient mode check (wake word detection)
        → ACP permission gate (FIFO queue)
        → Ambient context injection (if referential detected)
        → transcript_tx.send(TranscriptReady { utterance_id, text })
```

### Proactive Event Flow

```
Background source (daemon, eyes, agent, consolidation) → proactive_tx
  → main loop receives ProactiveEvent:
    .AgentResult { tool_call_id: Some(id) } → inject as tool_result into LlmSession, send to llm_task
    .AgentResult { tool_call_id: None } → queue in pending_agent_results (delivered when LLM idle)
    .InferenceDaemon → no-op (handled by pipeline)
    .L1Saturated → send SystemNotification with saturation details
    .AgentQuestion → queue in pending_agent_questions, send ACP permission prompt
    .PluginSwitch → no-op (handled via dedicated plugin_switch_rx)
```

## Integration

### Module Dependencies

| Module | Depends On | Consumed By |
|--------|-----------|-------------|
| `main.rs` | All modules | Binary entry point |
| `lib.rs` | All modules | External consumers (tests, libraries) |
| `config.rs` | None (stdlib + serde + toml) | Every other module |
| `daemon.rs` | `llm`, `agents` | `main.rs` |
| `eyes.rs` | `llm`, `screen_capture`, `agents` | `main.rs` |
| `device_monitor.rs` | `pipeline`, `i18n`, `cpal` | `main.rs` |
| `screen_capture.rs` | None (stdlib + tokio) | `eyes.rs`, `tools::TakeScreenshotTool` |
| `i18n.rs` | None (stdlib) | `main.rs`, `pipeline`, `device_monitor.rs` |

### Subdirectory Codemaps

- [`src/pipeline/codemap.md`](src/pipeline/codemap.md) — Pipeline orchestration, FSM, four concurrent actors, context consolidation
- [`src/llm/codemap.md`](src/llm/codemap.md) — LLM HTTP client, session management, process supervision

### Feature Flags

| Feature | Enables | Affects Root Modules |
|---------|---------|---------------------|
| `avspeech` | macOS AVSpeechSynthesizer | `main.rs` main() runs CFRunLoop; TTS engine selection |
| `tui` | Terminal UI | `main.rs` tracing to file; TUI task spawn; TUI event channels |
| `remote` | WebSocket server | `main.rs` remote server spawn; `remote_tts_tx` channel in tts_task |
| `control` | HTTP/SSE control API | `main.rs` control server spawn; `ControlBroadcast` in pipeline tasks |
| `speaker` | Speaker verification | `main.rs` IdentityAnalyzer creation |
| `kokoro` | Kokoro ONNX TTS | `main.rs` TTS engine selection |
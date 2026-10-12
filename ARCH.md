# Seneschal — Architecture Map (ARCH.md)

This document maps every module in the Seneschal codebase, its dependencies,
and its role in the system. It is the authoritative reference for understanding
how modules relate to one another.

---

## Dependency Graph

```
                              ┌─────────────────┐
                              │   src/main.rs    │
                              │  Entry point,    │
                              │  task spawning,  │
                              │  audio loop      │
                              └────────┬────────┘
                                       │ orchestrates everything
          ┌────────────────────────────┼──────────────────────────────┐
          │                            │                              │
          ▼                            ▼                              ▼
┌─────────────────┐   ┌──────────────────────────────┐   ┌──────────────────┐
│  src/config.rs  │   │     src/pipeline/            │   │  src/lib.rs      │
│  env-based      │◄──│  ┌───────────────┐           │   │  public re-exports│
│  configuration  │   │  │ fsm.rs        │           │   └──────────────────┘
│  Config struct  │   │  │ PipelineState │           │
└────────┬────────┘   │  │ watch channel │           │
         │            │  ├───────────────┤           │
         │            │  │ frames.rs     │           │
         │            │  │ PipelineFrame │           │
         │            │  ├───────────────┤           │
         │            │  │ state.rs      │           │
         │            │  │ PipelineEvents│           │
         │            │  ├───────────────┤           │
         │            │  │ llm_task.rs   │◄────── llm, tools, agents, db
         │            │  │ sen_task.rs   │◄────── tts (SentenceSplitter)
         │            │  │ tts_task.rs   │◄────── tts (TtsEngine), audio
         │            │  │ consolidation │◄────── llm, db, memory, profile
         │            │  └───────────────┘           │
         │            └──────────────────────────────┘
         │
         │ config flows into every module
         │
    ┌────┴──────────────────────────────────────────────────────────────┐
    │                                                                    │
    ▼                                                                    ▼
┌───────────┐  ┌──────────┐  ┌──────────┐  ┌───────────┐  ┌──────────────┐
│ audio/    │  │ stt/     │  │ llm/     │  │ tts/      │  │ db/          │
│ capture   │──► provider │──► client   │──► engine    │  │ SQLite       │
│ output    │  │ VAD      │  │ session  │  │ splitter  │  │ sessions     │
│ resample  │  │ whisper  │  │ manager  │  │ avspeech  │  │ messages     │
│ buffer    │  │ parakeet │  │          │  │ kokoro    │  │ profile      │
│ speaker   │  │ speech   │  │          │  │ piper(ref)│  │ memories     │
│ ambient   │  └──────────┘  └──────────┘  └───────────┘  │ migrations   │
└───────────┘                                              └──────────────┘
      │                                                          │
      │  AudioChunk ──► stt ──► SpeechEvent ──► pipeline         │
      │                                                        │
      └──────────────────────────────────────────────────────────┤
                                                                  │
  ┌──────────────────────────────────────────────────────────────┼─────────┐
  │                     Cross-Cutting Modules                     │         │
  │                                                              │         │
  │  ┌───────────┐  ┌───────────┐  ┌────────────┐  ┌──────────┐ │         │
  │  │ tools/    │  │ agents/   │  │ memory/    │  │ profile/ │ │         │
  │  │ registry  │  │ ACP       │  │ extract    │  │ extract  │ │         │
  │  │ 20+ tools │  │ session   │  │ context    │  │ facts    │◄┼─────────┘
  │  └───────────┘  │ manager   │  └────────────┘  └──────────┘ │
  │       │         └───────────┘         │               │      │
  │       │              │                │               │      │
  │       ▼              ▼                ▼               ▼      │
  │  ┌───────────┐  ┌───────────┐  ┌──────────────────────────┐  │
  │  │ mcp/      │  │ plugins/  │  │ dream/                   │  │
  │  │ stdio     │  │ manager   │  │ S-DREAM consolidation    │  │
  │  │ HTTP      │  │ agent-bdg │  │ L1/L2 memory             │  │
  │  └───────────┘  │ manifest  │  └──────────────────────────┘  │
  │                 │ prompt-inj│                                 │
  │                 └───────────┘                                 │
  │                                                              │
  │  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌──────────┐  │
  │  │ search/   │  │ analysis/ │  │ control/  │  │ tui/     │  │
  │  │ searxng   │  │ identity  │  │ API HTTP  │  │ ratatui  │  │
  │  │ brave     │  │ Context   │  │ SSE       │  │ UI       │  │
  │  │ tavily    │  │ Lens      │  │ broadcast │  └──────────┘  │
  │  │ exa       │  └───────────┘  └───────────┘               │
  │  └───────────┘                                              │
  │                                                              │
  └──────────────────────────────────────────────────────────────┘
```

---

## Module Dependency Table

Arrows read as "depends on". External crates listed in **bold**.

### Core Pipeline (hot path)

| Module | Depends On | Role |
|--------|-----------|------|
| `audio/` | **cpal**, **rubato**, `config` | Mic capture, speaker playback, resampling, circular buffers, speaker verification (ONNX), ambient speech buffer |
| `stt/` | **whisper-cpp-plus**, **parakeet-rs**, **speech**, `audio::buffer`, `config` | `SttProvider` trait, `WhisperSTTVAD` (whisper-cpp-plus + Silero VAD), ParakeetTDT ONNX, SFSpeechRecognizer |
| `llm/` | **reqwest**, `config`, `db` | `OpenAIClient` (OpenAI-compatible SSE streaming), `LlmSession` (history), LLM manager |
| `tts/` | **objc2**, **kokorox**, `config` | `TtsEngine` enum (AvSpeech, Kokoro, Mock), `SentenceSplitter` (punctuation-boundary token buffering) |
| `pipeline/` | `stt`, `llm`, `tts`, `tools`, `agents`, `db`, `memory`, `profile`, `control`, `analysis` | FSM orchestration, per-utterance task spawning (llm_task, sen_task, tts_task, consolidation_task), `PipelineFrame` typed messages |

### Control & Monitoring

| Module | Depends On | Role |
|--------|-----------|------|
| `control/` | **axum**, **tower**, `pipeline`, `config` | HTTP + SSE server: `/control/events`, `/control/state`, `/control/history`, barge-in, mute, text input |
| `tui/` | **ratatui**, **crossterm**, `pipeline`, `config` | Terminal UI: state display, conversation scrollback, text input, TTS mute toggle |
| `remote/` | **axum**, `pipeline`, `config` | WebSocket server for remote audio streaming |

### Intelligence & Delegation

| Module | Depends On | Role |
|--------|-----------|------|
| `tools/` | `llm`, `agents`, `search`, `mcp`, `plugins`, `screen_capture`, `config` | `Tool` trait + `ToolRegistry`. 20+ tool implementations: time, clipboard, shell, web search, agent delegation, screenshots, MCP proxy |
| `agents/` | `config`, `llm` | ACP (Agent Communication Protocol): JSON-RPC 2.0 stdio session management, Hermes/OpenCode integration, session events |
| `mcp/` | `config` | MCP stdio/HTTP client: tool discovery, `McpClient`, `McpToolDef`, `call_tool` |
| `plugins/` | `config`, `agents`, `mcp`, `tools` | Plugin system: manifest loading, agent bridging, MCP spawning, config overrides, prompt injection, runtime plugin switching |
| `search/` | **reqwest**, `config` | `SearchProvider` trait: SearXNG, Brave Search, Tavily, Exa backends |

### Memory & Learning

| Module | Depends On | Role |
|--------|-----------|------|
| `db/` | **sqlx**, `config` | SQLite: sessions, messages, user_profile, memories. Migration-first schema in `db/migrations/` |
| `memory/` | `db`, `llm` | Extract persistent notes from conversation. Archive outdated memories. Build memory context for system prompt. |
| `profile/` | `db`, `llm` | Extract structured user profile facts from conversation (name, preferences, city, etc.) |
| `dream/` | `db`, `llm`, `config` | S-DREAM cold-path memory consolidation daemon. L1 → L2 archival, scheduled or idle-triggered. |
| `analysis/` | `stt`, `config` | `IdentityAnalyzer` (speaker verification via sherpa ONNX), `ContextLens` (multi-observer bus for identity/emotion/video) |

### Background Daemons

| Module | Depends On | Role |
|--------|-----------|------|
| `daemon.rs` | `llm`, `config` | `InferenceDaemon`: periodic proactive reasoning ("is there anything worth saying?") |
| `eyes.rs` | `llm`, `screen_capture`, `config` | `EyesDaemon`: periodic screenshot → vision LLM analysis → proactive user notifications |

### Utilities & Infrastructure

| Module | Depends On | Role |
|--------|-----------|------|
| `config.rs` | **dotenvy**, **toml** | `Config::from_env()` — all environment variable parsing and config file loading |
| `i18n.rs` | — | Language-specific strings (Spanish/English) |
| `screen_capture.rs` | **base64** | macOS screenshot capture utility, shared by `take_screenshot` tool and `EyesDaemon` |
| `device_monitor.rs` | `audio`, `config` | Monitors audio device connect/disconnect (Bluetooth headset reconnect detection) |
| `agent_session.rs` | — | PTY session log viewer for visible agent mode |
| `lib.rs` | all public modules | Library root, re-exports public API types |

### Binaries

| Binary | Path | Role |
|--------|------|------|
| `seneschal` | `src/main.rs` | Main binary: async_main() orchestration, task spawning, audio loop |
| `acp_agent_chat` | `src/bin/acp_agent_chat.rs` | Debug binary: TUI chat with ACP agent via JSON-RPC 2.0 stdio |
| `test_stt_plus` | `src/bin/test_stt_plus.rs` | Test binary: standalone whisper-cpp-plus streaming STT test |

---

## Data Flow (Per-Utterance)

```
                     ┌──────────┐
                     │  CPAL    │
                     │  Mic     │
                     └────┬─────┘
                          │ async_channel<AudioChunk>
                          ▼
               ┌────────────────────┐
               │  WhisperSTTVAD     │
               │  (Silero VAD +     │
               │   whisper-cpp-plus)│
               └────────┬───────────┘
                        │ mpsc<SpeechEvent>
                        ▼
               ┌────────────────────┐
               │  main loop         │
               │  dispatch          │
               └──┬────────────┬────┘
                  │            │
        SpeechStart            SpeechEnd(transcript)
        barge_in_tx            transcript_tx
        (broadcast)            (mpsc)
                  │            │
                  ▼            ▼
          ┌──────────────────────────┐
          │  llm_task                │
          │  OpenAIClient::stream()  │
          │  tool detection + exec   │
          └──────────┬───────────────┘
                     │ mpsc<LLMToken>
                     ▼
          ┌──────────────────────────┐
          │  sen_task                │
          │  SentenceSplitter        │
          │  buffer → punctuation    │
          └──────────┬───────────────┘
                     │ mpsc<SentenceReady>
                     ▼
          ┌──────────────────────────┐
          │  tts_task                │
          │  TtsEngine.synthesize()  │
          │  AudioOutput.play()      │
          └──────────┬───────────────┘
                     │
                     ▼
          ┌──────────────────────────┐
          │  Speaker                 │
          │  (CPAL output)           │
          └──────────────────────────┘
```

---

## Control Plane (Cancellation & State)

```
    ┌─────────────────┐
    │  VAD             │
    │  SpeechStart     │──► barge_in_tx (broadcast::Sender<u64>)
    └─────────────────┘         │
                                ├──► llm_task  (cancel HTTP stream)
                                ├──► sen_task  (drain buffer)
                                ├──► tts_task  (stop playback, set play_cancel)
                                └──► consolidation_task (abort if running)

    ┌─────────────────┐
    │  PipelineState   │──► watch::Sender<PipelineState>
    │  (one writer     │         │
    │   per transition)│         ├──► FSM observer (log transitions)
    └─────────────────┘         ├──► TUI (display state)
                                ├──► Control API (SSE broadcast)
                                └──► All actors (read current state)
```

---

## Feature Flags & Conditional Compilation

| Feature | Modules Activated | Extra Dependencies |
|---------|-------------------|-------------------|
| (none) | `audio`, `stt` (whisper), `llm`, `tts` (mock), `pipeline`, `tools`, `db` | whisper-cpp-plus, reqwest, sqlx |
| `avspeech` | `tts::avspeech` | objc2, block2 |
| `kokoro` | `tts::kokoro` | kokorox |
| `parakeet` | `stt::parakeet` | parakeet-rs |
| `speech` | `stt::speech_recognizer` | speech |
| `speaker` | `audio::speaker`, `analysis::identity` | sherpa-rs |
| `tui` | `tui/` | ratatui, crossterm |
| `remote` | `remote/` | axum, tower |
| `control` | `control/` | axum, tower |

---

## External Dependencies (Key Crates)

| Crate | Used By | Purpose |
|-------|---------|---------|
| `tokio` | all | Async runtime, channels (mpsc, broadcast, watch, oneshot) |
| `cpal` | `audio/` | Cross-platform audio I/O |
| `whisper-cpp-plus` | `stt/` | True streaming Whisper.cpp with Silero VAD |
| `reqwest` | `llm/`, `search/` | HTTP client for LLM SSE streaming and web search |
| `sqlx` | `db/` | SQLite async ORM with migrations |
| `serde` + `serde_json` | all | Serialization |
| `tracing` | all | Structured logging |
| `anyhow` + `thiserror` | all | Error handling |
| `ratatui` + `crossterm` | `tui/` | Terminal UI |
| `axum` + `tower` | `control/`, `remote/` | HTTP/SSE/WebSocket server |
| `kokorox` | `tts/` | Kokoro ONNX TTS |
| `parakeet-rs` | `stt/` | NVIDIA Parakeet TDT ONNX STT |
| `sherpa-rs` | `audio/`, `analysis/` | Speaker verification ONNX |
| `rubato` | `audio/` | Audio resampling |
| `objc2` | `tts/` | macOS AVSpeechSynthesizer bindings |
| `dotenvy` | `config.rs` | `.env` file loading |
| `toml` | `config.rs` | TOML config file parsing |
| `chrono` | `db/`, `tools/` | Date/time handling |
| `wiremock` | `e2e_tests.rs` [dev] | HTTP mock server for integration tests |

---

*Last updated: 2026-07-27. Version 1.0.*

# Seneschal — Task Registry (TASKS.md)

Master index of all per-module task files. Each task file tracks atomic,
verifiable work items with milestone tags linking back to `ROADMAP.md`.

## Module Task Files

| File | Module(s) | Description |
|------|-----------|-------------|
| [`tasks/pipeline.md`](tasks/pipeline.md) | `src/pipeline/`, `src/main.rs` | Core pipeline orchestration, FSM, per-utterance tasks, barge-in, SharedSession elimination |
| [`tasks/audio-stt.md`](tasks/audio-stt.md) | `src/audio/`, `src/stt/` | Audio capture/playback, VAD, STT providers (Whisper, Parakeet, SFSpeechRecognizer), speaker verification |
| [`tasks/llm.md`](tasks/llm.md) | `src/llm/` | OpenAIClient, LlmSession, LLM manager, streaming, tool detection, think filtering |
| [`tasks/tts.md`](tasks/tts.md) | `src/tts/` | TtsEngine (AvSpeech, Kokoro), SentenceSplitter, audio output |
| [`tasks/tools.md`](tasks/tools.md) | `src/tools/` | Tool trait, ToolRegistry, individual tool implementations |
| [`tasks/delegation.md`](tasks/delegation.md) | `src/agents/`, `src/mcp/`, `src/plugins/`, `src/search/` | Agent delegation (ACP), MCP integration, plugin system, web search providers |
| [`tasks/memory.md`](tasks/memory.md) | `src/db/`, `src/memory/`, `src/profile/`, `src/dream/`, `src/analysis/` | SQLite persistence, memory extraction, user profile, S-DREAM consolidation, identity analysis |
| [`tasks/control-ui.md`](tasks/control-ui.md) | `src/control/`, `src/tui/`, `src/remote/` | HTTP/SSE control API, terminal UI, WebSocket remote server |
| [`tasks/infra.md`](tasks/infra.md) | `src/config.rs`, `src/daemon.rs`, `src/eyes.rs`, `src/device_monitor.rs`, `src/i18n.rs`, build, CI | Configuration, background daemons, device monitoring, i18n, build system, CI/CD |

## Task Status Legend

| Marker | Meaning |
|--------|---------|
| `- [ ]` | Pending — not started |
| `- [~]` | In progress — partially done |
| `- [x]` | Complete — verified |
| `- [-]` | Cancelled — no longer relevant |

## Milestone Tags

Tasks are tagged with milestone IDs from `ROADMAP.md`:

| Tag | Milestone |
|-----|-----------|
| `[M0.2]` | Core Pipeline Stabilisation |
| `[M0.3]` | Tool Ecosystem |
| `[M0.4]` | Intelligence & Memory |
| `[M1.1]` | Reliability & Polish |
| `[M1.2]` | Calendar & Productivity |
| `[M1.3]` | User Experience |
| `[M2.x]` | 1.0 Release milestones |
| `[M3.x]` | Post-1.0 milestones |

## How to Work with Tasks

1. **Pick a module** → open its `tasks/<module>.md`.
2. **Find an unchecked task** with no blockers.
3. **Execute the task.** Verify with `cargo build`, `cargo test`, `cargo clippy`.
4. **Mark complete** → `[x]` + commit message referencing the task.
5. **If blocked**, note the blocker and move to another task.

---

*Last updated: 2026-07-27. Version 1.0.*

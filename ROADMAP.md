# Seneschal — Roadmap (ROADMAP.md)

This document defines the long-term strategic milestones for Seneschal. Each
milestone is a deliverable increment with clear scope and success criteria.
Milestones are ordered by dependency; earlier milestones unlock later ones.

> Current version: **v0.1.0-alpha.7** (July 2026)

---

## Phase 0: Foundation (current — v0.1.x alpha)

**Goal:** Stable, reliable voice pipeline. Solid architecture. Developer tooling.

### M0.1: Project Constitution & Structure ✅
- [x] Rename binary from `voicebot` to `seneschal`
- [x] Environment separation (PRO/DEV) via `SENESCHAL_ENV`
- [x] TOML config file + env var override (+ `SENESCHAL_CONFIG_FILE`)
- [x] QA harness: `make qa` (fmt, lint, test, e2e, build)
- [x] CI pipeline in Gitea Actions
- [x] Install script (`install.sh`) and uninstall script (`uninstall.sh`)
- [x] Code map and architecture documentation in `doc/`
- [x] `AGENTS.md` for AI agent guidance
- [x] `CONSTITUTION.md`, `ARCH.md`, `ROADMAP.md`, `TASKS.md` — governance docs

### M0.2: Core Pipeline Stabilisation 🔄
- [x] Streaming STT → LLM → TTS pipeline with tokio channels
- [x] Pipeline FSM (`PipelineState`) on `watch` channel
- [x] `PipelineFrame` typed messages for inter-actor communication
- [x] Pluggable STT (Whisper, Parakeet, SFSpeechRecognizer)
- [x] Pluggable TTS (AvSpeech, Kokoro)
- [x] Barge-in with `broadcast` cancellation
- [x] Sentence-by-sentence synthesis (sentence N plays while N+1 generates)
- [x] SQLite persistence: sessions, messages, user profile, memories
- [x] Context consolidation (summarization when context window fills)
- [x] Startup greeting and session restoration
- [ ] **Pipeline refactor: eliminate `SharedSession`** — replace all 16 shared-mutable-state fields with typed channels and `PipelineState` reads
- [ ] **Split cancellation signals** — separate `barge_in_tx` (VAD → all) from `pause_tx` (consolidation → LLM)
- [ ] Latency benchmarking suite (VAD→first-audio, end-to-end) with regression guards
- [ ] Barge-in stress tests (rapid fire interruptions, mid-sentence cancel)

### M0.3: Tool Ecosystem 🔄
- [x] Core tools: `current_time`, `read_clipboard`, `set_clipboard`, `open_app`, `read_file`, `run_shell`, `web_search`, `take_screenshot`
- [x] Agent delegation: ACP protocol (Hermes, OpenCode), CLI mode, visible agent PTY sessions
- [x] MCP integration: stdio + HTTP transport, multi-server, tool proxy
- [x] Plugin system: manifest loading, agent bridging, MCP spawning, config overrides, runtime switching
- [x] Apple Events: Calendar and Reminders via AppleScript
- [x] `quick_search` and `deep_research` multi-tier search
- [x] `recover_historical_context`: FTS5 full-text search of message archive
- [x] `noop` tool for idle pipeline handling
- [x] `prompt_build` for iterative prompt construction in TUI
- [ ] **Scheduled tool execution** — the daemon can trigger tools on a timer (e.g., "remind me in 10 minutes")
- [ ] **Tool permission system** — user-confirmable tool calls (e.g., "Should I delete this file?")
- [ ] **Tool result streaming** — long-running tools stream partial results to TTS

### M0.4: Intelligence & Memory 🔄
- [x] Speaker verification (sherpa-rs ONNX): auto-enrollment, multi-speaker profiles
- [x] Ambient context buffer: transcribe all speech, feed context even in ambient mode
- [x] Conversation modes: Active, Ambient, AmbientLocked with auto-switch
- [x] Identity analyzer: ContextLens bus for multi-observer identity tracking
- [x] S-DREAM memory consolidation: L1 → L2 archival, scheduled and idle-triggered
- [x] User profile extraction: structured facts injected into system prompt
- [x] Inference daemon: proactive "is there anything worth saying?" reasoning
- [x] EYES visual awareness: periodic screenshot → vision LLM → notifications
- [ ] **Emotion detection** — prosody-based sentiment in ContextLens (affect analyzer)
- [ ] **Intent routing** — classify utterance intent (query, command, chitchat) and adjust LLM temperature
- [ ] **Long-term learning** — S-DREAM L3: cross-session pattern extraction (habits, preferences)

---

## Phase 1: Beta Quality (v0.2 → v0.9)

**Goal:** Polished user experience. No critical bugs. Documentation complete.
Ready for daily use by non-developers.

### M1.1: Reliability & Polish
- [ ] **Zero crash target** — 48-hour soak test with real voice interaction
- [ ] **Memory leak audit** — profile and fix all leaks (audio buffers, channel backlog, DB connections)
- [ ] **Error recovery** — graceful degradation: missing LLM → TTS fallback message, no mic → TUI-only mode, STT timeout → retry
- [ ] **Startup time** — target < 5 seconds from binary launch to "ready" greeting
- [ ] **LLM self-management** — `LLM_SELF_MANAGED`: spawn, supervise, restart LLM server automatically (partially done)
- [ ] **Voice quality settings** — TTS voice, rate, pitch configurable at runtime via voice commands
- [ ] **Wake word customization** — user-trainable custom wake word (replace `WAKE_WORD` env var)

### M1.2: Calendar & Productivity
- [ ] **Calendar sync** — read/write macOS Calendar via EventKit or CalDAV
- [ ] **Reminders overhaul** — full CRUD for macOS Reminders (partially done via Apple Events)
- [ ] **Email summaries** — read recent emails, summarize for user
- [ ] **Daily briefing** — scheduled morning recap: weather, calendar, reminders, news
- [ ] **Timer/alarm system** — set named timers and alarms via voice, TTS alert on expiry

### M1.3: User Experience
- [ ] **TUI 2.0** — resizable panels, dark/light theme, conversation search, settings panel
- [ ] **Voice command reference** — built-in help system: "Seneschal, what can you do?"
- [ ] **Onboarding wizard** — first-run setup: voice selection, LLM config, permissions
- [ ] **macOS menu bar app** — system tray icon with quick actions (mute, barge-in, quit)
- [ ] **Notification Center integration** — proactive suggestions delivered as macOS notifications
- [ ] **Accessibility** — VoiceOver compatibility, high-contrast TUI mode

---

## Phase 2: 1.0 Release (v1.0.0)

**Goal:** Stable API, comprehensive documentation, ecosystem readiness.

### M2.1: API & Extensibility
- [ ] **Control API v1** — stable HTTP/SSE API with OpenAPI spec, versioned endpoints
- [ ] **Plugin marketplace** — discoverable plugin registry, install via CLI, manifest signing
- [ ] **MCP best practices guide** — documentation for MCP server authors targeting Seneschal
- [ ] **Agent SDK** — Rust/Python libraries for building ACP-compatible agents
- [ ] **Web dashboard** — browser-based control panel with conversation history, settings, plugin management

### M2.2: Multi-Platform
- [ ] **Linux support** — full pipeline on Linux: Kokoro TTS, whisper-cpp-plus STT, PulseAudio/ALSA capture
- [ ] **Windows support** — full pipeline on Windows: system TTS, whisper-cpp-plus STT, WASAPI capture
- [ ] **Cross-platform CI** — macOS, Linux, Windows in CI matrix
- [ ] **Docker deployment** — headless mode for server/embedded use (no audio capture, TUI optional)

### M2.3: Documentation
- [ ] **User manual** — comprehensive end-user documentation site
- [ ] **Developer guide** — architecture deep-dive, provider authoring, tool development
- [ ] **API reference** — full Rustdoc with examples for every public type
- [ ] **Video tutorials** — setup, configuration, plugin development
- [ ] **Translation** — documentation in Spanish and English

### M2.4: Mobile Companion App
- [x] **iOS companion dual-channel (LAN)** — WS audio (`WS_PORT`) + Control SSE/REST (`CONTROL_PORT`); pipeline status, conversation, timeline, text input, mute, barge-in, agent PermissionSheet; iPad adaptive layout + a11y (issue #190; see [`doc/IOS_COMPANION.md`](doc/IOS_COMPANION.md))
- [ ] **watchOS companion polish** — pipeline state surface + last-line preview on top of existing PTT relay (TN3135); full timeline remains phone-side
- [ ] **Android companion app** — equivalent to iOS dual-channel companion
- [ ] **WAN access** — Tailscale/tunnel + auth for away-from-home (explicitly out of #190)
- [ ] **Push notifications** — proactive suggestions delivered to phone/watch (Live Activities optional)
- [ ] **Offline / edge mode** — on-device STT/TTS or cloud fallback when host offline

---

## Phase 3: Post-1.0 (v1.1+)

**Goal:** Expand capabilities. Ecosystem growth. Community.

### M3.1: Multi-User Support
- [ ] **User profiles** — per-user conversation history, preferences, memories
- [ ] **Voice authentication** — lock/unlock sensitive actions by speaker identity
- [ ] **Multi-user conversations** — track who said what, route responses to the right person
- [ ] **Privacy mode** — suspend ambient transcription when non-enrolled speakers are present

### M3.2: Advanced Intelligence
- [ ] **Real-time translation** — STT (source) → LLM translate → TTS (target)
- [ ] **Voice cloning** — custom TTS voice from user voice sample
- [ ] **Emotion-aware responses** — adjust tone, pace, and content based on detected user emotion
- [ ] **Proactive task automation** — learn repetitive tasks (e.g., "you always ask for weather at 8 AM")
- [ ] **Knowledge graph** — structured entity extraction across sessions, semantic search

### M3.3: Smart Home & IoT
- [ ] **HomeKit integration** — control lights, thermostats, locks via voice
- [ ] **Matter protocol** — cross-platform smart home device control
- [ ] **Media control** — play/pause/skip music, podcasts, audiobooks
- [ ] **Multi-room audio** — AirPlay/Chromecast output for music and TTS

### M3.4: Ecosystem & Community
- [ ] **Plugin registry** — community-contributed plugins, ratings, automated testing
- [ ] **Voice pack marketplace** — community-created TTS voices (Kokoro styles, Piper voices)
- [ ] **Agent directory** — shareable agent configurations for specific domains (medical, legal, coding)
- [ ] **Contributor program** — RFC process, governance model, mentorship
- [ ] **Conference talks & blog** — share architecture insights, Rust async patterns, voice AI research

---

## Versioning Scheme

```
v<major>.<minor>.<patch>-<state><number>

States: alpha, beta, rc
Examples: v0.1.13-alpha01, v0.2.0-beta.1, v1.0.0-rc.1
```

| Phase | Version Range | State |
|-------|--------------|-------|
| 0 (Foundation) | v0.1.0-alpha.1 → v0.1.x-alpha.N | Alpha |
| 1 (Beta Quality) | v0.2.0-beta.1 → v0.9.x | Beta |
| 2 (1.0 Release) | v1.0.0-rc.1 → v1.0.0 | RC → Stable |
| 3 (Post-1.0) | v1.1.0+ | Stable |

---

## Milestone Progress Tracking

```
M0.1 ████████████████████ 100%  Constitution & Structure
M0.2 ████████████░░░░░░░░  60%  Pipeline Stabilisation
M0.3 ██████████████░░░░░░  70%  Tool Ecosystem
M0.4 ██████████░░░░░░░░░░  50%  Intelligence & Memory
M1.1 ░░░░░░░░░░░░░░░░░░░░   0%  Reliability & Polish
M1.2 ░░░░░░░░░░░░░░░░░░░░   0%  Calendar & Productivity
M1.3 ░░░░░░░░░░░░░░░░░░░░   0%  User Experience
M2.1 ░░░░░░░░░░░░░░░░░░░░   0%  API & Extensibility
M2.2 ░░░░░░░░░░░░░░░░░░░░   0%  Multi-Platform
M2.3 ░░░░░░░░░░░░░░░░░░░░   0%  Documentation
M2.4 ████████░░░░░░░░░░░░  40%  Mobile Companion App (iOS LAN dual-channel shipped)
M3.1 ░░░░░░░░░░░░░░░░░░░░   0%  Multi-User Support
M3.2 ░░░░░░░░░░░░░░░░░░░░   0%  Advanced Intelligence
M3.3 ░░░░░░░░░░░░░░░░░░░░   0%  Smart Home & IoT
M3.4 ░░░░░░░░░░░░░░░░░░░░   0%  Ecosystem & Community
```

---

## How to Use This Roadmap

1. **Agents** read `TASKS.md` (per-module) for atomic work items.
2. **TASKS.md** items are tagged with milestone IDs (e.g., `[M0.2]`).
3. **This roadmap** is the high-level strategic view; it is updated when milestones complete.
4. **Amendments** to this roadmap require a review against the Constitution.
5. **Completed items** are checked `[x]` in this file on delivery.

---

*Last updated: 2026-07-27. Version 1.0.*

# Control & UI — Task List

Modules: `src/control/`, `src/tui/`, `src/remote/`

---

## [M0.3] Control API (`src/control/`)

### HTTP + SSE Server (`src/control/api.rs`)
- [x] axum-based HTTP server on `CONTROL_PORT`
- [x] `GET /control/events` — SSE stream of `ControlEvent` updates
- [x] `GET /control/state` — JSON: current `PipelineState`, utterance ID, mute status
- [x] `GET /control/history` — JSON: conversation message history
- [x] `POST /control/mute` — toggle TTS mute
- [x] `POST /control/barge_in` — trigger barge-in
- [x] `POST /control/input` — inject text as user input
- [ ] **Authentication**: add optional API key auth (`CONTROL_API_KEY`) for non-localhost access
- [ ] **CORS**: add configurable CORS headers for browser-based dashboards
- [ ] **Rate limiting**: limit SSE connections to 5; limit POST endpoints to 10 req/s
- [ ] **WebSocket upgrade**: add WebSocket endpoint as alternative to SSE for bidirectional streaming
- [ ] **API versioning**: prefix endpoints with `/v1/`; maintain backward compat

### Control Events (`src/control/broadcast.rs`)
- [x] `ControlEvent` enum: `StateChanged`, `Transcript`, `LlmToken`, `LlmDone`, `TtsStart`, `ToolCall`, `MuteChanged`, `Error`
- [ ] **Event completeness**: add events for `TtsSentence`, `TtsDone`, `ConsolidationStart`, `ConsolidationDone`, `DaemonEvent`
- [ ] **Event replay**: allow SSE clients to request event history (last N events) on connect

### Control State (`src/control/state.rs`)
- [x] `ControlState` — shared mutable state for the API
- [ ] **Thread safety audit**: ensure all `ControlState` fields use appropriate synchronization (currently `Arc<Mutex<>>`)

---

## [M1.3] Terminal UI (`src/tui/`)

### Core TUI (`src/tui/app.rs`, `src/tui/ui.rs`)
- [x] ratatui-based TUI with conversation view, status bar, text input
- [x] Voice and text input work simultaneously
- [x] Scrollback (`PageUp`/`PageDown`)
- [x] TTS mute toggle (`Ctrl+T`)
- [ ] **Resizable panels**: split view with resizable conversation/status areas
- [ ] **Dark/light theme**: toggle between dark and light color schemes (`Ctrl+Shift+T`)
- [ ] **Settings panel**: dedicated settings view for voice, LLM, audio config
- [ ] **Conversation search**: `/search <query>` filters visible messages
- [ ] **Command palette**: `/help` shows available commands; `/plugin <name>`, `/mode active|ambient`
- [ ] **Tool result display**: show tool calls and results inline in the conversation view
- [ ] **Performance overlay**: `Ctrl+P` toggles latency metrics overlay (VAD→first-audio, tool durations)

### TUI Events (`src/tui/events.rs`, `src/tui/input.rs`)
- [ ] **Mouse support**: click to select messages, scroll with mouse wheel
- [ ] **Copy to clipboard**: `Ctrl+Shift+C` copies selected message text

---

## [M2.1] Remote Server (`src/remote/`)

- [x] WebSocket server for remote audio streaming (`WS_PORT`)
- [ ] **Authentication**: add API key or token authentication for WebSocket connections
- [ ] **Audio codec negotiation**: support Opus in addition to raw PCM for bandwidth-efficient streaming
- [ ] **Client identity**: track connected clients by device ID; route audio to specific speaker output per client

---

## [M2.1] Web Dashboard

- [ ] **Browser-based control panel**: SPA served by Seneschal's control API
  - Conversation history with search
  - Real-time pipeline state display
  - Settings management (LLM, STT, TTS, tools)
  - Plugin management (install, activate, deactivate)
- [ ] **Implementation**: Yew (Rust WASM), Leptos, or plain HTML+JS served from embedded assets

---

*Last updated: 2026-07-27.*

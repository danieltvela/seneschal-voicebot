# src/control/ — Control API (HTTP/REST + SSE)

## Responsibility

HTTP-based control plane for programmatic interaction with the Voicebot pipeline. Provides REST endpoints for state inspection, history retrieval, and pipeline control (mute, barge-in, text input). Also exposes an SSE (Server-Sent Events) stream for real-time event subscription. Includes a client library for automated testing and integration.

## Design

### Module Structure

| File | Role |
|------|------|
| `mod.rs` | Conditional compilation: `api` and `state` behind `control` feature flag |
| `api.rs` | Axum router + endpoint handlers (feature-gated) |
| `state.rs` | `ControlState` shared state struct (feature-gated) |
| `broadcast.rs` | `ControlEvent` enum + `ControlBroadcast` wrapper (always available) |
| `client.rs` | HTTP client library for the control API (always available) |

### Shared State (`ControlState`)

```rust
pub struct ControlState {
    pub broadcast: ControlBroadcast,
    pub pipeline_state_rx: watch::Receiver<PipelineState>,
    pub tts_muted: Arc<AtomicBool>,
    pub play_cancel: Arc<AtomicBool>,
    pub barge_in_tx: broadcast::Sender<u64>,
    pub transcript_tx: mpsc::Sender<PipelineFrame>,
    pub llm_session: Arc<Mutex<LlmSession>>,
    pub db: Database,
}
```

### Event Types (`ControlEvent`)

```rust
enum ControlEvent {
    StateChanged { state, utterance_id },
    Transcript { utterance_id, text },
    LlmToken { utterance_id, token },
    LlmDone { utterance_id, full_text },
    TtsStart { utterance_id },
    ToolCall { name, result },
    MuteChanged { muted },
    Error { message },
    SystemNotification { text },
}
```

### SSE Buffer Limit

`MAX_SSE_BUFFER_SIZE = 1024 * 1024` bytes — SSE streams terminate after 1 MB of cumulative data to prevent unbounded memory growth.

### Client Library

`ControlClient` provides:
- Builder pattern (`ControlClientBuilder`) for configuration.
- Health check, state, history endpoints.
- Control actions: mute, barge-in, text input.
- SSE subscription via `subscribe_events()` → `mpsc::Receiver<ClientControlEvent>`.
- Testing utilities: `wait_for_state`, `wait_for_event`, `poll_state`, `assert_state`, `transaction`, `send_input_and_wait`.

## Flow

### Server Startup

```
start_control_server(port, state)
  → router(state) → Axum Router
  → TcpListener::bind(0.0.0.0:port)
  → axum::serve(listener, app)
```

### Router

```
Router::new()
  .route("/control/sessions", get(get_sessions))
  .route("/control/sessions/{id}/messages", get(get_session_messages))
  .route("/control/events", get(sse_events))
  .route("/control/state", get(get_state))
  .route("/control/history", get(get_history))
  .route("/control/health", get(get_health))
  .route("/control/mute", post(post_mute))
  .route("/control/barge_in", post(post_barge_in))
  .route("/control/input", post(post_input))
  .with_state(state)
```

### Endpoint Handlers

```
GET /control/health → { "status": "healthy", "service": "jarvis-control" }

GET /control/state → {
  "state": PipelineState::Debug string,
  "utterance_id": Option<u64>,
  "tts_muted": bool
}

GET /control/history → Vec<Message> from LlmSession

GET /control/sessions → Vec<SessionListEntry> from Database

GET /control/sessions/{id}/messages → Vec<MessageListEntry> from Database

GET /control/events → SSE stream:
  → Subscribe to broadcast channel
  → Stream ControlEvent as JSON
  → Track total_bytes_sent; terminate at MAX_SSE_BUFFER_SIZE
  → Handle RecvError::Lagged → emit Error event
  → Handle RecvError::Closed → terminate stream

POST /control/mute { "muted": bool } → 204
  → Store tts_muted flag
  → Broadcast MuteChanged event

POST /control/barge_in → 204
  → Send 0 to barge_in_tx (cancels current pipeline)

POST /control/input { "text": String } → 204
  → Send PipelineFrame::TextInput to transcript_tx
  → If channel closed: return 503
```

### Client Flow

```
ControlClient::new(base_url) → ControlClientBuilder().base_url(url).build()

subscribe_events():
  → GET /control/events with Accept: text/event-stream
  → Spawn background task: read bytes → parse SSE → send to mpsc channel
  → Return mpsc::Receiver<ClientControlEvent>

wait_for_state(expected, timeout):
  → Poll GET /control/state until state matches or timeout

wait_for_event(predicate, timeout):
  → Subscribe to SSE → filter events by predicate → return first match or timeout

send_input_and_wait(text, timeout):
  → Subscribe to SSE
  → POST /control/input
  → Collect LlmToken events → accumulate text
  → On LlmDone → return full_text

transaction(from_state, action, to_state, timeout):
  → assert_state(from_state)
  → action.await
  → wait_for_state(to_state, timeout)
```

## Integration

### Dependencies
- `axum` — HTTP server framework (feature-gated).
- `futures_util` — Stream handling for SSE.
- `tokio::sync::{broadcast, mpsc, watch}` — Channel types.
- `serde` — JSON serialization.
- `reqwest` — HTTP client (for client library).
- `crate::db::Database` — Session/message queries.
- `crate::llm::LlmSession` — Conversation history.
- `crate::pipeline::frames::PipelineFrame` — Input injection.
- `crate::pipeline::fsm::PipelineState` — State reporting.

### Consumers
- `src/main.rs` — Creates `ControlState`, calls `start_control_server`.
- `src/daemon.rs` — Writes to `ControlBroadcast`, updates pipeline state.
- External automation scripts — Use `ControlClient` for testing.

### Feature Flags
- `control` feature enables `api.rs` and `state.rs` modules.
- `broadcast.rs` and `client.rs` are always available.
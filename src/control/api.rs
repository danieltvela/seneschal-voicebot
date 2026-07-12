use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::Stream;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;
use tracing::error;
use uuid::Uuid;

use super::broadcast::ControlEvent;
use super::state::ControlState;
use crate::pipeline::frames::PipelineFrame;

const MAX_SSE_BUFFER_SIZE: usize = 1024 * 1024;

pub fn router(state: Arc<ControlState>) -> Router {
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
}

pub async fn start_control_server(port: u16, state: Arc<ControlState>) -> anyhow::Result<()> {
    let app = router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(target: "control", "Control API listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn sse_events(
    State(state): State<Arc<ControlState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.broadcast.subscribe();
    let mut total_bytes_sent = 0usize;
    let stream = futures_util::stream::unfold(
        (rx, total_bytes_sent),
        |(mut rx, mut total_bytes)| async move {
            if total_bytes >= MAX_SSE_BUFFER_SIZE {
                return None;
            }
            match rx.recv().await {
                Ok(event) => {
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    total_bytes += json.len();
                    Some((Ok(Event::default().data(json)), (rx, total_bytes)))
                }
                Err(RecvError::Lagged(n)) => {
                    let err = ControlEvent::Error {
                        message: format!("Missed {n} events (subscriber lagged)"),
                    };
                    let json = serde_json::to_string(&err).unwrap_or_default();
                    total_bytes += json.len();
                    Some((Ok(Event::default().data(json)), (rx, total_bytes)))
                }
                Err(RecvError::Closed) => None,
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn get_state(State(state): State<Arc<ControlState>>) -> impl IntoResponse {
    let ps = state.pipeline_state_rx.borrow().clone();
    let muted = state.tts_muted.load(Ordering::SeqCst);
    Json(serde_json::json!({
        "state": format!("{ps:?}"),
        "utterance_id": ps.utterance_id(),
        "tts_muted": muted,
    }))
}

async fn get_history(State(state): State<Arc<ControlState>>) -> impl IntoResponse {
    let messages = state.llm_session.lock().unwrap().messages.clone();
    Json(messages)
}

async fn get_health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "seneschal-control",
    }))
}

#[derive(Deserialize)]
struct MuteBody {
    muted: bool,
}

async fn post_mute(
    State(state): State<Arc<ControlState>>,
    Json(body): Json<MuteBody>,
) -> StatusCode {
    state.tts_muted.store(body.muted, Ordering::SeqCst);
    state
        .broadcast
        .send(ControlEvent::MuteChanged { muted: body.muted });
    StatusCode::NO_CONTENT
}

async fn post_barge_in(State(state): State<Arc<ControlState>>) -> StatusCode {
    let _ = state.barge_in_tx.send(0);
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct InputBody {
    text: String,
}

async fn post_input(
    State(state): State<Arc<ControlState>>,
    Json(body): Json<InputBody>,
) -> StatusCode {
    if state
        .transcript_tx
        .send(PipelineFrame::TextInput { text: body.text })
        .await
        .is_err()
    {
        error!(target: "control", "transcript_tx closed");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    StatusCode::NO_CONTENT
}

// ── History API responses ────────────────────────────────────────────────────

#[derive(Serialize)]
struct SessionListEntry {
    id: String,
    created_at: String,
    is_active: bool,
}

#[derive(Serialize)]
struct MessageListEntry {
    id: i64,
    role: String,
    content: String,
    timestamp: String,
}

async fn get_sessions(State(state): State<Arc<ControlState>>) -> impl IntoResponse {
    match state.db.list_sessions_with_active().await {
        Ok(sessions) => {
            let entries: Vec<SessionListEntry> = sessions
                .into_iter()
                .map(|(id, created_at, is_active)| SessionListEntry {
                    id,
                    created_at,
                    is_active,
                })
                .collect();
            Json(entries)
        }
        Err(e) => {
            tracing::error!(target: "control", "Failed to list sessions: {e}");
            Json(Vec::<SessionListEntry>::new())
        }
    }
}

async fn get_session_messages(
    State(state): State<Arc<ControlState>>,
    Path(session_id_str): Path<String>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id_str) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(target: "control", "Invalid session ID '{session_id_str}': {e}");
            return Json(Vec::<MessageListEntry>::new());
        }
    };

    match state
        .db
        .get_messages_with_timestamp_after_id(session_id, 0)
        .await
    {
        Ok(messages) => {
            let entries: Vec<MessageListEntry> = messages
                .into_iter()
                .map(|(id, role, content, timestamp)| MessageListEntry {
                    id,
                    role,
                    content,
                    timestamp,
                })
                .collect();
            Json(entries)
        }
        Err(e) => {
            tracing::error!(target: "control", "Failed to get messages for session {session_id_str}: {e}");
            Json(Vec::<MessageListEntry>::new())
        }
    }
}

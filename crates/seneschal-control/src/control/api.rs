use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::Stream;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;
use tracing::error;
use uuid::Uuid;

use super::audio::{
    AudioControl, AudioTestError, AudioTestErrorCode, AudioTestKind, TestDurationBody,
    setting_string,
};
use super::broadcast::ControlEvent;
use super::state::ControlState;
use seneschal_common::settings::SettingsPatch;
use seneschal_core::pipeline::frames::PipelineFrame;

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
        .route("/control/permissions", get(get_permissions))
        .route("/control/permission", post(post_permission))
        .route("/control/config", get(get_config))
        .route("/control/config/validate", post(post_config_validate))
        .route("/control/config/apply", post(post_config_apply))
        .route("/control/audio/devices", get(get_audio_devices))
        .route("/control/audio/status", get(get_audio_status))
        .route("/control/audio/permission", post(post_audio_permission))
        .route("/control/audio/preview-tts", post(post_audio_preview_tts))
        .route("/control/audio/speaker-test", post(post_audio_speaker_test))
        .route("/control/audio/mic-test", post(post_audio_mic_test))
        .route("/control/audio/loopback", post(post_audio_loopback))
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

/// SSE event stream from a control broadcast subscription.
///
/// No cumulative byte cap: long sessions stay open. Lagging subscribers get an
/// `Error { Missed N events }` event and should resync via `GET /control/state`.
pub(crate) fn control_event_sse_stream(
    rx: tokio::sync::broadcast::Receiver<ControlEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures_util::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => {
                let json = serde_json::to_string(&event).unwrap_or_default();
                Some((Ok(Event::default().data(json)), rx))
            }
            Err(RecvError::Lagged(n)) => {
                let err = ControlEvent::Error {
                    message: format!("Missed {n} events (subscriber lagged)"),
                };
                let json = serde_json::to_string(&err).unwrap_or_default();
                Some((Ok(Event::default().data(json)), rx))
            }
            Err(RecvError::Closed) => None,
        }
    })
}

async fn sse_events(
    State(state): State<Arc<ControlState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.broadcast.subscribe();
    Sse::new(control_event_sse_stream(rx)).keep_alive(KeepAlive::default())
}

async fn get_state(State(state): State<Arc<ControlState>>) -> impl IntoResponse {
    let ps = state.pipeline_state_rx.borrow().clone();
    let muted = state.tts_muted.load(Ordering::SeqCst);
    let mut body = serde_json::json!({
        "state": ps.control_wire_state(),
        "utterance_id": ps.utterance_id(),
        "tts_muted": muted,
    });
    if let Some(reason) = ps.control_pause_reason() {
        body["pause_reason"] = serde_json::json!(reason);
    }
    Json(body)
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
    if state.permission_gate.has_pending_or_resolving() {
        state.broadcast.send(ControlEvent::Error {
            message: "permission pending — use POST /control/permission".into(),
        });
        return StatusCode::CONFLICT;
    }
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

async fn get_permissions(State(state): State<Arc<ControlState>>) -> impl IntoResponse {
    Json(state.permission_gate.list())
}

async fn get_config(State(state): State<Arc<ControlState>>) -> impl IntoResponse {
    Json(state.settings.snapshot().await)
}

async fn post_config_validate(
    State(state): State<Arc<ControlState>>,
    Json(patch): Json<SettingsPatch>,
) -> impl IntoResponse {
    Json(state.settings.validate(&patch).await)
}

async fn post_config_apply(
    State(state): State<Arc<ControlState>>,
    Json(patch): Json<SettingsPatch>,
) -> impl IntoResponse {
    match state.settings.apply(&patch).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(errors) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "ok": false, "errors": errors })),
        )
            .into_response(),
    }
}

fn audio_control(state: &ControlState) -> Result<Arc<AudioControl>, Response> {
    state.audio.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "code": "audio_unavailable",
                    "message": "Audio diagnostics are not wired in this daemon",
                }
            })),
        )
            .into_response()
    })
}

fn audio_error_response(error: AudioTestError) -> Response {
    let status = match error.code {
        AudioTestErrorCode::InvalidDuration => StatusCode::BAD_REQUEST,
        AudioTestErrorCode::AudioBusy | AudioTestErrorCode::Cancelled => StatusCode::CONFLICT,
        AudioTestErrorCode::ProviderError => StatusCode::BAD_GATEWAY,
        AudioTestErrorCode::TestTimedOut => StatusCode::GATEWAY_TIMEOUT,
        AudioTestErrorCode::TestAssetError => StatusCode::INTERNAL_SERVER_ERROR,
        AudioTestErrorCode::CaptureUnavailable
        | AudioTestErrorCode::OutputUnavailable
        | AudioTestErrorCode::ChannelClosed => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(serde_json::json!({ "error": error }))).into_response()
}

async fn get_audio_devices(State(state): State<Arc<ControlState>>) -> Response {
    match audio_control(&state).and_then(|audio| {
        audio.devices().map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "code": "device_enumeration_failed",
                        "message": format!("{error:#}"),
                    }
                })),
            )
                .into_response()
        })
    }) {
        Ok(inventory) => Json(inventory).into_response(),
        Err(response) => response,
    }
}

async fn get_audio_status(State(state): State<Arc<ControlState>>) -> Response {
    let audio = match audio_control(&state) {
        Ok(audio) => audio,
        Err(response) => return response,
    };
    let snapshot = state.settings.snapshot().await;
    let status = audio
        .status(
            setting_string(&snapshot, "audio_input_device"),
            setting_string(&snapshot, "audio_output_device"),
            setting_string(&snapshot, "stt_provider").unwrap_or_else(|| "unknown".to_string()),
            setting_string(&snapshot, "tts_provider").unwrap_or_else(|| "unknown".to_string()),
        )
        .await;
    Json(status).into_response()
}

async fn post_audio_permission(State(state): State<Arc<ControlState>>) -> Response {
    let audio = match audio_control(&state) {
        Ok(audio) => audio,
        Err(response) => return response,
    };
    Json(audio.request_microphone_permission().await).into_response()
}

async fn post_audio_preview_tts(State(state): State<Arc<ControlState>>) -> Response {
    let audio = match audio_control(&state) {
        Ok(audio) => audio,
        Err(response) => return response,
    };
    match audio.preview_tts().await {
        Ok(result) => Json(result).into_response(),
        Err(error) => audio_error_response(error),
    }
}

async fn post_audio_speaker_test(State(state): State<Arc<ControlState>>) -> Response {
    let audio = match audio_control(&state) {
        Ok(audio) => audio,
        Err(response) => return response,
    };
    match audio.speaker_test().await {
        Ok(result) => Json(result).into_response(),
        Err(error) => audio_error_response(error),
    }
}

async fn post_audio_mic_test(
    State(state): State<Arc<ControlState>>,
    Json(body): Json<TestDurationBody>,
) -> Response {
    capture_test_response(state, AudioTestKind::Mic, body.duration_secs).await
}

async fn post_audio_loopback(
    State(state): State<Arc<ControlState>>,
    Json(body): Json<TestDurationBody>,
) -> Response {
    capture_test_response(state, AudioTestKind::Loopback, body.duration_secs).await
}

async fn capture_test_response(
    state: Arc<ControlState>,
    kind: AudioTestKind,
    duration_secs: Option<u64>,
) -> Response {
    let audio = match audio_control(&state) {
        Ok(audio) => audio,
        Err(response) => return response,
    };
    match audio.capture_test(kind, duration_secs).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => audio_error_response(error),
    }
}

#[derive(Deserialize)]
struct PermissionBody {
    task_id: String,
    option_id: String,
}

async fn post_permission(
    State(state): State<Arc<ControlState>>,
    Json(body): Json<PermissionBody>,
) -> StatusCode {
    use seneschal_common::HttpPermissionResult;

    match state
        .permission_gate
        .try_resolve_http(&body.task_id, &body.option_id)
    {
        HttpPermissionResult::Ok => {
            state.broadcast.send(ControlEvent::AgentPermissionResolved {
                task_id: body.task_id,
                option_id: body.option_id,
            });
            StatusCode::NO_CONTENT
        }
        HttpPermissionResult::ClosedCancelled => {
            state.broadcast.send(ControlEvent::AgentPermissionResolved {
                task_id: body.task_id,
                option_id: "cancelled".into(),
            });
            StatusCode::CONFLICT
        }
        HttpPermissionResult::NotFound => StatusCode::NOT_FOUND,
        HttpPermissionResult::Conflict => StatusCode::CONFLICT,
        HttpPermissionResult::BadOption => StatusCode::BAD_REQUEST,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::broadcast::ControlBroadcast;
    use futures_util::StreamExt;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::{broadcast, mpsc, watch};
    use tower::ServiceExt;

    use seneschal_core::pipeline::{PauseReason, PipelineState};

    /// Build a `ControlState` with the given pipeline state. The `/control/state`
    /// handler only reads the watch channel, so the DB is never queried.
    ///
    /// The returned `TempDir` must be kept alive for the duration of the test.
    async fn build_state(state: PipelineState) -> (Arc<ControlState>, tempfile::TempDir) {
        build_state_with_audio(state, false).await
    }

    async fn build_state_with_audio(
        state: PipelineState,
        with_audio: bool,
    ) -> (Arc<ControlState>, tempfile::TempDir) {
        let db_dir = tempfile::TempDir::new().unwrap();
        let db =
            seneschal_common::db::Database::new(db_dir.path().join("control.db").to_str().unwrap())
                .await
                .unwrap();
        let (barge_in_tx, _) = broadcast::channel::<u64>(8);
        let (transcript_tx, _) = mpsc::channel::<PipelineFrame>(8);
        let (_state_tx, pipeline_state_rx) = watch::channel(state);
        let (hot_tx, _hot_rx) = mpsc::channel::<seneschal_common::settings::HotSettings>(8);
        let tts = Arc::new(tokio::sync::RwLock::new(
            seneschal_core::tts::TtsEngine::Mock(seneschal_core::tts::mock_tts::MockTts::new().0),
        ));
        let settings = crate::control::settings::SettingsService::new(
            seneschal_common::config::Config::embedded_defaults(),
            Arc::clone(&tts),
            Arc::new(seneschal_core::audio::output::AudioOutput::null()),
            Arc::new(std::sync::Mutex::new(
                seneschal_common::settings::HotSettings {
                    language: "es".to_string(),
                    wake_word: "seneschal".to_string(),
                },
            )),
            hot_tx,
        )
        .unwrap();

        let audio = if with_audio {
            let (mic_tx, _mic_rx) = mpsc::channel(1);
            Some(Arc::new(AudioControl::new(
                tts,
                Arc::new(seneschal_core::audio::output::AudioOutput::null()),
                mic_tx,
                barge_in_tx.clone(),
                pipeline_state_rx.clone(),
                Arc::new(AtomicBool::new(false)),
                Arc::new(std::sync::Mutex::new(None)),
            )))
        } else {
            None
        };

        let control = Arc::new(ControlState {
            broadcast: ControlBroadcast::new(16),
            pipeline_state_rx,
            tts_muted: Arc::new(AtomicBool::new(false)),
            play_cancel: Arc::new(AtomicBool::new(false)),
            barge_in_tx,
            transcript_tx,
            llm_session: Arc::new(std::sync::Mutex::new(seneschal_core::llm::LlmSession::new(
                "sys",
            ))),
            db,
            permission_gate: Arc::new(seneschal_common::PermissionGate::new()),
            settings: Arc::new(settings),
            audio,
        });
        (control, db_dir)
    }

    async fn get_state_json(state: Arc<ControlState>) -> (StatusCode, serde_json::Value) {
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/control/state")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// `/control/state` must expose the stable wire token plus the utterance id
    /// of the active speech state.
    #[tokio::test]
    async fn get_config_exposes_fields_origins_and_providers() {
        let (state, _db_dir) = build_state(PipelineState::Idle).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/control/config")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let fields = json["fields"].as_array().unwrap();
        let wake_word = fields
            .iter()
            .find(|field| field["key"] == "wake_word")
            .unwrap();
        assert_eq!(wake_word["tier"], "hot");
        assert!(wake_word["editable"].is_boolean());
        assert!(json["providers"]["stt"].is_array());
        assert!(json["providers"]["tts"].is_array());
        assert!(!json.to_string().contains("api_key"));
    }

    /// Invalid settings are rejected before applying, with a field name and a
    /// human-readable message.
    #[tokio::test]
    async fn apply_invalid_settings_returns_field_errors() {
        let (state, _db_dir) = build_state(PipelineState::Idle).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/control/config/apply")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({ "output_volume": 2.0 }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["errors"][0]["field"], "output_volume");
    }

    #[tokio::test]
    async fn audio_devices_endpoint_returns_inventory_shape() {
        let (state, _db_dir) = build_state_with_audio(PipelineState::Idle, true).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/control/audio/devices")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["inputs"].is_array());
        assert!(json["outputs"].is_array());
        for direction in ["inputs", "outputs"] {
            for device in json[direction].as_array().unwrap() {
                assert!(device["name"].is_string());
                assert!(device["selector"].is_string());
                assert!(device["is_default"].is_boolean());
            }
        }
    }

    #[tokio::test]
    async fn audio_status_exposes_capture_output_permission_and_providers() {
        let (state, _db_dir) = build_state_with_audio(PipelineState::Idle, true).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/control/audio/status")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["capture_active"], false);
        assert_eq!(json["output_available"], false);
        assert!(json["microphone_permission"].is_string());
        assert!(json["stt_provider"].is_string());
        assert!(json["tts_provider"].is_string());
        assert!(json["test_in_progress"].is_null());
    }

    #[tokio::test]
    async fn mic_test_requires_active_capture_and_valid_duration() {
        let (state, _db_dir) = build_state_with_audio(PipelineState::Idle, true).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/control/audio/mic-test")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"duration_secs":5}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "capture_unavailable");

        let (state, _db_dir) = build_state_with_audio(PipelineState::Idle, true).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/control/audio/mic-test")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"duration_secs":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn audio_playback_tests_reject_busy_pipeline_or_null_output() {
        let (state, _db_dir) =
            build_state_with_audio(PipelineState::Speaking { utterance_id: 1 }, true).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/control/audio/speaker-test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let (state, _db_dir) = build_state_with_audio(PipelineState::Idle, true).await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/control/audio/preview-tts")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "output_unavailable");
    }

    #[tokio::test]
    async fn get_state_reports_active_state_and_utterance_id() {
        let (state, _db_dir) = build_state(PipelineState::Thinking { utterance_id: 42 }).await;
        let (status, json) = get_state_json(state).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["state"], "thinking");
        assert_eq!(json["utterance_id"], 42);
        assert_eq!(json["tts_muted"], false);
        assert!(json.get("pause_reason").is_none());
    }

    /// A paused pipeline reports `paused` + the reason and no utterance id.
    #[tokio::test]
    async fn get_state_reports_paused_reason() {
        let (state, _db_dir) = build_state(PipelineState::Paused {
            reason: PauseReason::Consolidation,
        })
        .await;
        let (status, json) = get_state_json(state).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["state"], "paused");
        assert_eq!(json["utterance_id"], serde_json::Value::Null);
        assert_eq!(json["pause_reason"], "consolidation");
    }

    /// Regression: former `MAX_SSE_BUFFER_SIZE = 1 MiB` closed the stream mid-session.
    #[tokio::test]
    async fn sse_stream_survives_over_1_mib() {
        let broadcast = ControlBroadcast::new(4096);
        let mut stream = std::pin::pin!(control_event_sse_stream(broadcast.subscribe()));

        // ~2 KiB token × 600 events > 1 MiB of payload alone (plus JSON framing).
        let payload = "x".repeat(2048);
        const N: usize = 600;
        assert!(N * payload.len() > 1024 * 1024);

        for i in 0..N {
            broadcast.send(ControlEvent::LlmToken {
                utterance_id: i as u64,
                token: payload.clone(),
            });
        }

        let mut received = 0usize;
        while received < N {
            let _item = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .expect("SSE stream stalled")
                .expect("SSE stream closed early before delivering all events")
                .expect("infallible");
            received += 1;
        }

        assert_eq!(received, N);
    }
}

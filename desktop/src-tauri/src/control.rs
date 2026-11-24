use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::watch;

use crate::audio::SharedAudioHub;
use crate::chat::{SharedChatHub, rehydrate_history};
use crate::models::SharedModelHub;
use crate::status::{ConnectionStatus, DaemonStatus, PipelineSnapshot, SharedStatusHub};

const MIN_RECONNECT_DELAY: Duration = Duration::from_millis(100);
const MAX_RECONNECT_DELAY: Duration = Duration::from_millis(1_000);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(500);
const STATE_TIMEOUT: Duration = Duration::from_millis(2_000);

/// Events relevant to the shell. Unknown server event types are skipped, as
/// the iOS companion does.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    StateChanged {
        state: String,
        utterance_id: Option<u64>,
        pause_reason: Option<String>,
    },
    Transcript {
        utterance_id: u64,
        text: String,
        source: Option<String>,
    },
    LlmToken {
        utterance_id: u64,
        token: String,
    },
    LlmDone {
        utterance_id: u64,
        full_text: String,
    },
    TtsStart {
        utterance_id: u64,
    },
    ToolCall {
        name: String,
        result: String,
    },
    MuteChanged {
        muted: bool,
    },
    SystemNotification {
        text: String,
    },
    AudioLevel {
        rms: f32,
        peak: f32,
    },
    ModelDownloadProgress {
        model_id: String,
        download_id: String,
        phase: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        current_file: Option<String>,
        message: Option<String>,
    },
    Error(String),
}

#[derive(Debug, Deserialize)]
struct HealthWire {
    status: String,
    service: String,
}

#[derive(Debug, Deserialize)]
struct StateWire {
    state: String,
    utterance_id: Option<u64>,
    tts_muted: bool,
    #[serde(default)]
    pause_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StateChangedWire {
    state: String,
    utterance_id: Option<u64>,
    #[serde(default)]
    pause_reason: Option<String>,
}

/// Reconnect-aware client for the existing Control REST/SSE API.
pub struct ControlBridge {
    status: SharedStatusHub,
    audio: SharedAudioHub,
    models: SharedModelHub,
    chat: SharedChatHub,
    base_url: String,
    client: reqwest::Client,
    restart_rx: watch::Receiver<u64>,
}

impl ControlBridge {
    pub fn new(
        base_url: impl Into<String>,
        status: SharedStatusHub,
        audio: SharedAudioHub,
        models: SharedModelHub,
        chat: SharedChatHub,
        restart_rx: watch::Receiver<u64>,
    ) -> Self {
        Self {
            status,
            audio,
            models,
            chat,
            base_url: base_url.into(),
            client: build_client(),
            restart_rx,
        }
    }

    pub async fn run(self) {
        let Self {
            status,
            audio,
            models,
            chat,
            base_url,
            client,
            mut restart_rx,
        } = self;

        let mut backoff = MIN_RECONNECT_DELAY;
        loop {
            status.mark_connecting();
            if !Self::health_check(&client, &base_url).await {
                status.set_daemon(DaemonStatus::Starting, None);
                if Self::wait_or_restart(&mut restart_rx, backoff).await {
                    backoff = MIN_RECONNECT_DELAY;
                    continue;
                }
                backoff = MAX_RECONNECT_DELAY;
                continue;
            }

            status.set_daemon(DaemonStatus::Running, None);
            status.mark_connecting();
            let connected = tokio::select! {
                result = Self::connect_once(&status, &audio, &models, &chat, &base_url, &client) => result,
                _ = restart_rx.changed() => {
                    backoff = MIN_RECONNECT_DELAY;
                    status.mark_connecting();
                    continue;
                }
            };

            match connected {
                Ok(()) => backoff = MIN_RECONNECT_DELAY,
                Err(error) => {
                    tracing::debug!(target: "desktop_control", "control stream error: {error}");
                    backoff = MAX_RECONNECT_DELAY;
                }
            }

            chat.reset_streaming();
            status.set_connection(ConnectionStatus::Disconnected);
            if Self::wait_or_restart(&mut restart_rx, backoff).await {
                backoff = MIN_RECONNECT_DELAY;
            }
        }
    }

    async fn wait_or_restart(restart_rx: &mut watch::Receiver<u64>, delay: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(delay) => false,
            _ = restart_rx.changed() => true,
        }
    }

    async fn health_check(client: &reqwest::Client, base_url: &str) -> bool {
        let url = format!("{base_url}/control/health");
        let result = client.get(url).timeout(HEALTH_TIMEOUT).send().await;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                tracing::trace!(target: "desktop_control", "health check failed: {error}");
                return false;
            }
        };
        if !response.status().is_success() {
            return false;
        }
        match response.json::<HealthWire>().await {
            Ok(health) => health.status == "healthy" && health.service == "seneschal-control",
            Err(_) => false,
        }
    }

    /// Open SSE first, take a state snapshot and chat history second, then
    /// process queued SSE events. This prevents a transition or transcript
    /// emitted during rehydration from being lost.
    async fn connect_once(
        status: &SharedStatusHub,
        audio: &SharedAudioHub,
        models: &SharedModelHub,
        chat: &SharedChatHub,
        base_url: &str,
        client: &reqwest::Client,
    ) -> anyhow::Result<()> {
        let url = format!("{base_url}/control/events");
        let response = client
            .get(url)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await?
            .error_for_status()?;

        let snapshot = Self::get_state(client, base_url).await?;
        status.set_pipeline(snapshot);
        rehydrate_history(base_url, chat).await;
        status.mark_connected();

        let mut stream = response.bytes_stream();
        let mut buffer = SseBuffer::default();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(frame) = buffer.next_event() {
                if let Some(event) = parse_control_event(&frame)
                    && Self::apply_event(
                        status,
                        audio,
                        models,
                        chat,
                        base_url,
                        client,
                        event,
                    )
                    .await
                {
                    // Lag already lost deltas: reconnect so history is loaded
                    // before the next live event is consumed.
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn get_state(
        client: &reqwest::Client,
        base_url: &str,
    ) -> anyhow::Result<PipelineSnapshot> {
        let url = format!("{base_url}/control/state");
        let response = client
            .get(url)
            .timeout(STATE_TIMEOUT)
            .send()
            .await?
            .error_for_status()?;
        let state = response.json::<StateWire>().await?;
        Ok(PipelineSnapshot {
            state: state.state,
            utterance_id: state.utterance_id,
            pause_reason: state.pause_reason,
            tts_muted: Some(state.tts_muted),
        })
    }

    /// Apply an SSE event. Returns `true` when the caller must reconnect and
    /// rehydrate (currently subscriber lag).
    async fn apply_event(
        status: &SharedStatusHub,
        audio: &SharedAudioHub,
        models: &SharedModelHub,
        chat: &SharedChatHub,
        base_url: &str,
        client: &reqwest::Client,
        event: ControlEvent,
    ) -> bool {
        match event {
            ControlEvent::StateChanged { ref state, .. } => {
                let finalize = state == "idle" || state == "listening";
                status.apply_control_event(event);
                if finalize {
                    chat.finalize_streaming(None);
                }
            }
            ControlEvent::Transcript {
                utterance_id,
                text,
                source,
            } => {
                chat.apply_transcript(utterance_id, text, source.as_deref());
            }
            ControlEvent::LlmToken {
                utterance_id,
                token,
            } => {
                chat.apply_token(utterance_id, token);
            }
            ControlEvent::LlmDone {
                utterance_id,
                full_text,
            } => {
                chat.apply_done(utterance_id, full_text);
            }
            ControlEvent::TtsStart { .. } => {}
            ControlEvent::ToolCall { name, result } => {
                chat.apply_tool_call(name, result);
            }
            ControlEvent::SystemNotification { text } => {
                chat.apply_system_notification(text);
            }
            ControlEvent::MuteChanged { .. } => {
                status.apply_control_event(event);
            }
            ControlEvent::AudioLevel { rms, peak } => {
                audio.apply(crate::audio::AudioLevel { rms, peak });
            }
            ControlEvent::ModelDownloadProgress {
                model_id,
                download_id,
                phase,
                downloaded_bytes,
                total_bytes,
                current_file,
                message,
            } => {
                models.apply(crate::models::ModelProgress {
                    model_id,
                    download_id,
                    phase,
                    downloaded_bytes,
                    total_bytes,
                    current_file,
                    message,
                });
            }
            ControlEvent::Error(message) => {
                let missed = message.contains("Missed");
                status.apply_control_event(ControlEvent::Error(message.clone()));
                if missed {
                    if let Ok(snapshot) = Self::get_state(client, base_url).await {
                        status.set_pipeline(snapshot);
                    }
                    rehydrate_history(base_url, chat).await;
                    return true;
                }
                chat.apply_error(message);
            }
        }
        false
    }
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(500))
        .tcp_nodelay(true)
        .build()
        .expect("failed to build control HTTP client")
}

/// Incremental SSE frame splitter, independent of chunk boundaries.
#[derive(Default)]
pub struct SseBuffer {
    buffer: String,
}

impl SseBuffer {
    pub fn push_str(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    pub fn next_event(&mut self) -> Option<String> {
        if self.buffer.contains("\r\n") {
            self.buffer = self.buffer.replace("\r\n", "\n");
        }
        let end = self.buffer.find("\n\n")?;
        let frame = self.buffer[..end].to_string();
        self.buffer.drain(..end + 2);
        Some(frame)
    }
}

/// Parse one SSE frame body. Comments and unknown event types return `None`.
pub fn parse_control_event(frame: &str) -> Option<ControlEvent> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            data_lines.push(data);
        }
    }
    if data_lines.is_empty() {
        return None;
    }

    let json = data_lines.join("\n");
    let value = serde_json::from_str::<serde_json::Value>(&json).ok()?;
    match value.get("type")?.as_str()? {
        "state_changed" => {
            let event = serde_json::from_value::<StateChangedWire>(value).ok()?;
            Some(ControlEvent::StateChanged {
                state: event.state,
                utterance_id: event.utterance_id,
                pause_reason: event.pause_reason,
            })
        }
        "transcript" => {
            let event = serde_json::from_value::<TranscriptWire>(value).ok()?;
            Some(ControlEvent::Transcript {
                utterance_id: event.utterance_id,
                text: event.text,
                source: event.source,
            })
        }
        "llm_token" => {
            let event = serde_json::from_value::<LlmTokenWire>(value).ok()?;
            Some(ControlEvent::LlmToken {
                utterance_id: event.utterance_id,
                token: event.token,
            })
        }
        "llm_done" => {
            let event = serde_json::from_value::<LlmDoneWire>(value).ok()?;
            Some(ControlEvent::LlmDone {
                utterance_id: event.utterance_id,
                full_text: event.full_text,
            })
        }
        "tts_start" => {
            let event = serde_json::from_value::<UtteranceWire>(value).ok()?;
            Some(ControlEvent::TtsStart {
                utterance_id: event.utterance_id,
            })
        }
        "tool_call" => {
            let event = serde_json::from_value::<ToolCallWire>(value).ok()?;
            Some(ControlEvent::ToolCall {
                name: event.name,
                result: event.result,
            })
        }
        "mute_changed" => {
            let event = serde_json::from_value::<MuteChangedWire>(value).ok()?;
            Some(ControlEvent::MuteChanged { muted: event.muted })
        }
        "system_notification" => {
            let event = serde_json::from_value::<SystemNotificationWire>(value).ok()?;
            Some(ControlEvent::SystemNotification { text: event.text })
        }
        "audio_level" => {
            let event = serde_json::from_value::<AudioLevelWire>(value).ok()?;
            Some(ControlEvent::AudioLevel {
                rms: event.rms,
                peak: event.peak,
            })
        }
        "model_download_progress" => {
            let event = serde_json::from_value::<ModelDownloadProgressWire>(value).ok()?;
            Some(ControlEvent::ModelDownloadProgress {
                model_id: event.model_id,
                download_id: event.download_id,
                phase: event.phase,
                downloaded_bytes: event.downloaded_bytes,
                total_bytes: event.total_bytes,
                current_file: event.current_file,
                message: event.message,
            })
        }
        "error" => {
            let event = serde_json::from_value::<ErrorWire>(value).ok()?;
            Some(ControlEvent::Error(event.message))
        }
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct ErrorWire {
    message: String,
}

#[derive(Debug, Deserialize)]
struct TranscriptWire {
    utterance_id: u64,
    text: String,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmTokenWire {
    utterance_id: u64,
    token: String,
}

#[derive(Debug, Deserialize)]
struct LlmDoneWire {
    utterance_id: u64,
    full_text: String,
}

#[derive(Debug, Deserialize)]
struct UtteranceWire {
    utterance_id: u64,
}

#[derive(Debug, Deserialize)]
struct ToolCallWire {
    name: String,
    result: String,
}

#[derive(Debug, Deserialize)]
struct MuteChangedWire {
    muted: bool,
}

#[derive(Debug, Deserialize)]
struct SystemNotificationWire {
    text: String,
}

#[derive(Debug, Deserialize)]
struct AudioLevelWire {
    rms: f32,
    peak: f32,
}

#[derive(Debug, Deserialize)]
struct ModelDownloadProgressWire {
    model_id: String,
    download_id: String,
    phase: String,
    downloaded_bytes: u64,
    #[serde(default)]
    total_bytes: Option<u64>,
    #[serde(default)]
    current_file: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::StatusHub;
    use axum::http::header;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::time::Instant;

    #[test]
    fn sse_buffer_reassembles_frames_split_across_chunks() {
        let mut buffer = SseBuffer::default();
        buffer.push_str("data: {\"type\":\"state_ch");
        assert!(buffer.next_event().is_none());
        buffer.push_str("anged\",\"state\":\"speaking\"}\r\n\r\ndata: x\n\n");
        assert_eq!(
            buffer.next_event(),
            Some("data: {\"type\":\"state_changed\",\"state\":\"speaking\"}".to_string())
        );
        assert_eq!(buffer.next_event(), Some("data: x".to_string()));
        assert!(buffer.next_event().is_none());
    }

    #[test]
    fn sse_parser_skips_keepalive_and_unknown_events() {
        let frame = ": keepalive\n\ndata: {\"type\":\"mcp_notification\"}\n\n";
        let mut buffer = SseBuffer::default();
        buffer.push_str(frame);
        assert_eq!(buffer.next_event(), Some(": keepalive".to_string()));
        assert_eq!(
            buffer.next_event(),
            Some("data: {\"type\":\"mcp_notification\"}".to_string())
        );
        assert!(parse_control_event("data: {\"type\":\"mcp_notification\"}").is_none());
    }

    #[test]
    fn sse_parser_accepts_state_audio_and_error_events() {
        let state = parse_control_event(
            "data: {\"type\":\"state_changed\",\"state\":\"paused\",\"utterance_id\":null,\"pause_reason\":\"consolidation\"}",
        );
        assert_eq!(
            state,
            Some(ControlEvent::StateChanged {
                state: "paused".into(),
                utterance_id: None,
                pause_reason: Some("consolidation".into()),
            })
        );

        let level =
            parse_control_event("data: {\"type\":\"audio_level\",\"rms\":0.25,\"peak\":0.75}");
        assert_eq!(
            level,
            Some(ControlEvent::AudioLevel {
                rms: 0.25,
                peak: 0.75,
            })
        );

        let error =
            parse_control_event("data: {\"type\":\"error\",\"message\":\"Missed 3 events\"}");
        assert_eq!(error, Some(ControlEvent::Error("Missed 3 events".into())));
    }

    #[test]
    fn sse_parser_accepts_model_download_progress() {
        let event = parse_control_event(
            "data: {\"type\":\"model_download_progress\",\"model_id\":\"whisper-tiny\",\"download_id\":\"d1\",\"phase\":\"downloading\",\"downloaded_bytes\":1024,\"total_bytes\":4096,\"current_file\":\"ggml-tiny.bin\"}",
        );
        assert_eq!(
            event,
            Some(ControlEvent::ModelDownloadProgress {
                model_id: "whisper-tiny".into(),
                download_id: "d1".into(),
                phase: "downloading".into(),
                downloaded_bytes: 1024,
                total_bytes: Some(4096),
                current_file: Some("ggml-tiny.bin".into()),
                message: None,
            })
        );

        // Indeterminate NeMo progress omits total_bytes.
        let event = parse_control_event(
            "data: {\"type\":\"model_download_progress\",\"model_id\":\"magpie-tts\",\"download_id\":\"d2\",\"phase\":\"downloading\",\"downloaded_bytes\":0}",
        );
        assert_eq!(
            event,
            Some(ControlEvent::ModelDownloadProgress {
                model_id: "magpie-tts".into(),
                download_id: "d2".into(),
                phase: "downloading".into(),
                downloaded_bytes: 0,
                total_bytes: None,
                current_file: None,
                message: None,
            })
        );
    }

    #[test]
    fn sse_parser_accepts_chat_mute_and_system_events() {
        let transcript = parse_control_event(
            "data: {\"type\":\"transcript\",\"utterance_id\":4,\"text\":\"hola\",\"source\":\"voice\"}",
        );
        assert_eq!(
            transcript,
            Some(ControlEvent::Transcript {
                utterance_id: 4,
                text: "hola".into(),
                source: Some("voice".into()),
            })
        );

        let token = parse_control_event(
            "data: {\"type\":\"llm_token\",\"utterance_id\":4,\"token\":\"hola\"}",
        );
        assert_eq!(
            token,
            Some(ControlEvent::LlmToken {
                utterance_id: 4,
                token: "hola".into(),
            })
        );

        let done = parse_control_event(
            "data: {\"type\":\"llm_done\",\"utterance_id\":4,\"full_text\":\"hola\"}",
        );
        assert_eq!(
            done,
            Some(ControlEvent::LlmDone {
                utterance_id: 4,
                full_text: "hola".into(),
            })
        );

        let tool = parse_control_event(
            "data: {\"type\":\"tool_call\",\"name\":\"web_search\",\"result\":\"ok\"}",
        );
        assert_eq!(
            tool,
            Some(ControlEvent::ToolCall {
                name: "web_search".into(),
                result: "ok".into(),
            })
        );

        let mute = parse_control_event("data: {\"type\":\"mute_changed\",\"muted\":true}");
        assert_eq!(mute, Some(ControlEvent::MuteChanged { muted: true }));

        let system =
            parse_control_event("data: {\"type\":\"system_notification\",\"text\":\"hello\"}");
        assert_eq!(
            system,
            Some(ControlEvent::SystemNotification {
                text: "hello".into(),
            })
        );

        // Historical transcripts from an older daemon omit source.
        let legacy = parse_control_event(
            "data: {\"type\":\"transcript\",\"utterance_id\":5,\"text\":\"hola\"}",
        );
        assert!(matches!(
            legacy,
            Some(ControlEvent::Transcript { source: None, .. })
        ));
    }

    async fn spawn_mock_control() -> String {
        let health = || async {
            Json(serde_json::json!({
                "status": "healthy",
                "service": "seneschal-control"
            }))
        };
        let state = || async {
            Json(serde_json::json!({
                "state": "thinking",
                "utterance_id": 2,
                "tts_muted": false
            }))
        };
        let sessions = || async {
            Json(serde_json::json!([{
                "id": "11111111-1111-1111-1111-111111111111",
                "created_at": "2026-01-01T00:00:00Z",
                "is_active": true
            }]))
        };
        let messages = || async {
            Json(serde_json::json!([{
                "id": 1,
                "role": "User",
                "content": "history",
                "timestamp": "2026-01-01T00:00:01Z"
            }]))
        };
        let events = || async {
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                "data: {\"type\":\"state_changed\",\"state\":\"speaking\",\"utterance_id\":3}\n\n",
            )
        };

        let app = Router::new()
            .route("/control/health", get(health))
            .route("/control/state", get(state))
            .route("/control/sessions", get(sessions))
            .route("/control/sessions/{id}/messages", get(messages))
            .route("/control/events", get(events));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}")
    }

    async fn wait_for_connected(
        rx: &mut tokio::sync::broadcast::Receiver<crate::status::AppStatus>,
    ) -> crate::status::AppStatus {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let status = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("timed out waiting for connected status")
                .expect("status channel closed");
            eprintln!("received={status:?}");
            let matches = status.connection == ConnectionStatus::Connected
                && status.pipeline.as_ref().is_some_and(|pipeline| {
                    pipeline.state == "speaking" && pipeline.utterance_id == Some(3)
                });
            if matches {
                return status;
            }
        }
    }

    #[tokio::test]
    async fn bridge_connects_applies_snapshot_and_sse_then_reconnects() {
        let base_url = spawn_mock_control().await;
        let hub = std::sync::Arc::new(StatusHub::new());
        let chat = std::sync::Arc::new(crate::chat::ChatHub::new());
        let mut chat_rx = chat.subscribe();
        let (restart_tx, restart_rx) = watch::channel(0u64);
        let mut rx = hub.subscribe();

        tokio::spawn(
            ControlBridge::new(
                base_url,
                hub.clone(),
                std::sync::Arc::new(crate::audio::AudioHub::new()),
                std::sync::Arc::new(crate::models::ModelHub::new()),
                chat.clone(),
                restart_rx,
            )
            .run(),
        );
        let status = wait_for_connected(&mut rx).await;
        assert_eq!(status.daemon, DaemonStatus::Running);
        assert!(status.last_reconnect_ms.is_some());

        // History is hydrated before the bridge reports connected.
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), chat_rx.recv())
            .await
            .expect("chat snapshot timed out")
            .expect("chat channel closed");
        assert!(matches!(event, crate::chat::ChatEvent::Snapshot { .. }));
        assert_eq!(chat.snapshot().entries.len(), 1);
        assert_eq!(chat.snapshot().entries[0].text, "history");

        // A real restart bumps the generation; the bridge must cancel the old
        // connection and reconnect to the same mock daemon.
        restart_tx.send(2).expect("restart sender is alive");
        let second = wait_for_connected(&mut rx).await;
        assert_eq!(second.pipeline.as_ref().unwrap().state, "speaking");
    }
}

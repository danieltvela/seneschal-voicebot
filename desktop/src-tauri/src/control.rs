use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::watch;

use crate::status::{ConnectionStatus, DaemonStatus, PipelineSnapshot, SharedStatusHub};

const MIN_RECONNECT_DELAY: Duration = Duration::from_millis(100);
const MAX_RECONNECT_DELAY: Duration = Duration::from_millis(1_000);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(500);
const STATE_TIMEOUT: Duration = Duration::from_millis(2_000);

/// Events relevant to the spike. Unknown server event types are skipped, as
/// the iOS companion does.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    StateChanged {
        state: String,
        utterance_id: Option<u64>,
        pause_reason: Option<String>,
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
    base_url: String,
    client: reqwest::Client,
    restart_rx: watch::Receiver<u64>,
}

impl ControlBridge {
    pub fn new(
        base_url: impl Into<String>,
        status: SharedStatusHub,
        restart_rx: watch::Receiver<u64>,
    ) -> Self {
        Self {
            status,
            base_url: base_url.into(),
            client: build_client(),
            restart_rx,
        }
    }

    pub async fn run(self) {
        let Self {
            status,
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
                result = Self::connect_once(&status, &base_url, &client) => result,
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

    /// Open SSE first, take a state snapshot second, then process queued SSE
    /// events. This prevents a transition from being lost between the two.
    async fn connect_once(
        status: &SharedStatusHub,
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
        status.mark_connected();

        let mut stream = response.bytes_stream();
        let mut buffer = SseBuffer::default();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(frame) = buffer.next_event() {
                if let Some(event) = parse_control_event(&frame) {
                    Self::apply_event(status, base_url, client, event).await;
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

    async fn apply_event(
        status: &SharedStatusHub,
        base_url: &str,
        client: &reqwest::Client,
        event: ControlEvent,
    ) {
        match event {
            ControlEvent::StateChanged { .. } => {
                status.apply_control_event(event);
            }
            ControlEvent::Error(message) => {
                let missed = message.contains("Missed");
                status.apply_control_event(ControlEvent::Error(message));
                if missed && let Ok(snapshot) = Self::get_state(client, base_url).await {
                    status.set_pipeline(snapshot);
                }
            }
        }
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
        let frame = ": keepalive\n\ndata: {\"type\":\"tool_call\"}\n\n";
        let mut buffer = SseBuffer::default();
        buffer.push_str(frame);
        assert_eq!(buffer.next_event(), Some(": keepalive".to_string()));
        assert_eq!(
            buffer.next_event(),
            Some("data: {\"type\":\"tool_call\"}".to_string())
        );
        assert!(parse_control_event("data: {\"type\":\"tool_call\"}").is_none());
    }

    #[test]
    fn sse_parser_accepts_state_and_error_events() {
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

        let error =
            parse_control_event("data: {\"type\":\"error\",\"message\":\"Missed 3 events\"}");
        assert_eq!(error, Some(ControlEvent::Error("Missed 3 events".into())));
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
        let events = || async {
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                "data: {\"type\":\"state_changed\",\"state\":\"speaking\",\"utterance_id\":3}\n\n",
            )
        };

        let app = Router::new()
            .route("/control/health", get(health))
            .route("/control/state", get(state))
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
        let (restart_tx, restart_rx) = watch::channel(0u64);
        let mut rx = hub.subscribe();

        tokio::spawn(ControlBridge::new(base_url, hub.clone(), restart_rx).run());
        let status = wait_for_connected(&mut rx).await;
        assert_eq!(status.daemon, DaemonStatus::Running);
        assert!(status.last_reconnect_ms.is_some());

        // A real restart bumps the generation; the bridge must cancel the old
        // connection and reconnect to the same mock daemon.
        restart_tx.send(2).expect("restart sender is alive");
        let second = wait_for_connected(&mut rx).await;
        assert_eq!(second.pipeline.as_ref().unwrap().state, "speaking");
    }
}

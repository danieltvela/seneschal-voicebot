use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::control::ControlEvent;

/// State of the sidecar process itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStatus {
    Starting,
    Running,
    Restarting,
    Stopped,
    Failed,
}

/// State of the Control SSE connection in the desktop shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
}

/// Normalized `PipelineState` exposed by `/control/state` and SSE.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSnapshot {
    pub state: String,
    pub utterance_id: Option<u64>,
    pub pause_reason: Option<String>,
    pub tts_muted: Option<bool>,
}

/// One complete desktop status update sent to the frontend.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub daemon: DaemonStatus,
    pub connection: ConnectionStatus,
    pub pipeline: Option<PipelineSnapshot>,
    pub last_error: Option<String>,
    pub last_reconnect_ms: Option<u64>,
}

impl AppStatus {
    fn starting() -> Self {
        Self {
            daemon: DaemonStatus::Starting,
            connection: ConnectionStatus::Disconnected,
            pipeline: None,
            last_error: None,
            last_reconnect_ms: None,
        }
    }
}

/// In-process status bus: the current value plus a broadcast for the UI.
pub struct StatusHub {
    current: Mutex<AppStatus>,
    tx: broadcast::Sender<AppStatus>,
    reconnect_started_at: Mutex<Option<Instant>>,
}

impl StatusHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            current: Mutex::new(AppStatus::starting()),
            tx,
            reconnect_started_at: Mutex::new(None),
        }
    }

    pub fn snapshot(&self) -> AppStatus {
        self.current.lock().expect("status mutex poisoned").clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppStatus> {
        self.tx.subscribe()
    }

    fn publish(&self, update: impl FnOnce(&mut AppStatus)) {
        let status = {
            let mut current = self.current.lock().expect("status mutex poisoned");
            update(&mut current);
            current.clone()
        };
        let _ = self.tx.send(status);
    }

    pub fn set_daemon(&self, daemon: DaemonStatus, error: Option<String>) {
        self.publish(|status| {
            status.daemon = daemon;
            if error.is_some() {
                status.last_error = error;
            }
        });
    }

    /// A sidecar termination is terminal for the current generation.
    pub fn daemon_exited(&self, daemon: DaemonStatus, error: Option<String>) {
        self.publish(|status| {
            status.daemon = daemon;
            status.connection = ConnectionStatus::Disconnected;
            status.pipeline = None;
            status.last_error = error;
        });
    }

    pub fn mark_connecting(&self) {
        {
            let mut started = self
                .reconnect_started_at
                .lock()
                .expect("reconnect mutex poisoned");
            if started.is_none() {
                *started = Some(Instant::now());
            }
        }
        self.publish(|status| {
            status.connection = ConnectionStatus::Connecting;
        });
    }

    pub fn mark_connected(&self) {
        let elapsed = {
            let mut started = self
                .reconnect_started_at
                .lock()
                .expect("reconnect mutex poisoned");
            started.take().map(|at| at.elapsed().as_millis() as u64)
        };
        self.publish(|status| {
            status.connection = ConnectionStatus::Connected;
            if let Some(ms) = elapsed {
                status.last_reconnect_ms = Some(ms);
            }
        });
    }

    /// Restart timing starts exactly when the user asks for a restart, not
    /// when the old SSE stream happens to notice the disconnect.
    pub fn begin_reconnect(&self) {
        *self
            .reconnect_started_at
            .lock()
            .expect("reconnect mutex poisoned") = Some(Instant::now());
    }

    pub fn restart(&self) {
        self.publish(|status| {
            status.daemon = DaemonStatus::Restarting;
            status.connection = ConnectionStatus::Disconnected;
            status.pipeline = None;
        });
    }

    pub fn set_error(&self, error: String) {
        self.publish(|status| {
            status.last_error = Some(error);
        });
    }

    pub fn set_connection(&self, connection: ConnectionStatus) {
        self.publish(|status| {
            status.connection = connection;
        });
    }

    pub fn set_pipeline(&self, pipeline: PipelineSnapshot) {
        self.publish(|status| {
            status.pipeline = Some(pipeline);
        });
    }

    /// Apply a live mute transition without losing the current pipeline token.
    pub fn set_tts_muted(&self, muted: bool) {
        self.publish(|status| {
            if let Some(pipeline) = status.pipeline.as_mut() {
                pipeline.tts_muted = Some(muted);
            }
        });
    }

    /// Wait for the next successful Control connection.
    ///
    /// Used after a restart-tier Settings apply so the caller does not race the
    /// sidecar restart when it immediately runs an audio/provider test.
    pub async fn wait_until_connected(&self, timeout: std::time::Duration) -> Result<(), String> {
        let mut rx = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.snapshot().connection == ConnectionStatus::Connected {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err("timed out waiting for the daemon to reconnect".to_string());
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(_)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err("status hub closed while waiting for reconnect".to_string());
                }
                Err(_) => {
                    return Err(format!(
                        "timed out after {} ms waiting for the daemon to reconnect",
                        timeout.as_millis()
                    ));
                }
            }
        }
    }

    /// Apply an SSE event after it has been normalized by the control client.
    pub fn apply_control_event(&self, event: ControlEvent) {
        match event {
            ControlEvent::StateChanged {
                state,
                utterance_id,
                pause_reason,
            } => {
                let tts_muted = self
                    .snapshot()
                    .pipeline
                    .and_then(|pipeline| pipeline.tts_muted);
                self.set_pipeline(PipelineSnapshot {
                    state,
                    utterance_id,
                    pause_reason,
                    tts_muted,
                });
            }
            ControlEvent::Error(error) => self.set_error(error),
            ControlEvent::MuteChanged { muted } => self.set_tts_muted(muted),
            // Chat state is owned by `ChatHub`; audio and model progress are
            // consumed by their dedicated hubs.
            ControlEvent::Transcript { .. }
            | ControlEvent::LlmToken { .. }
            | ControlEvent::LlmDone { .. }
            | ControlEvent::TtsStart { .. }
            | ControlEvent::ToolCall { .. }
            | ControlEvent::SystemNotification { .. } => {}
            ControlEvent::AudioLevel { .. } => {}
            ControlEvent::ModelDownloadProgress { .. } => {}
        }
    }
}

impl Default for StatusHub {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedStatusHub = Arc<StatusHub>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlEvent;

    #[test]
    fn state_changed_replaces_pipeline_without_losing_mute_state() {
        let hub = StatusHub::new();
        hub.set_pipeline(PipelineSnapshot {
            state: "idle".into(),
            utterance_id: None,
            pause_reason: None,
            tts_muted: Some(true),
        });

        hub.apply_control_event(ControlEvent::StateChanged {
            state: "speaking".into(),
            utterance_id: Some(7),
            pause_reason: None,
        });

        let status = hub.snapshot();
        let pipeline = status.pipeline.expect("pipeline is present");
        assert_eq!(pipeline.state, "speaking");
        assert_eq!(pipeline.utterance_id, Some(7));
        assert_eq!(pipeline.tts_muted, Some(true));
    }

    #[test]
    fn mute_changed_updates_pipeline_without_resetting_state() {
        let hub = StatusHub::new();
        hub.set_pipeline(PipelineSnapshot {
            state: "speaking".into(),
            utterance_id: Some(9),
            pause_reason: None,
            tts_muted: Some(false),
        });

        hub.apply_control_event(ControlEvent::MuteChanged { muted: true });

        let pipeline = hub.snapshot().pipeline.expect("pipeline is present");
        assert_eq!(pipeline.state, "speaking");
        assert_eq!(pipeline.utterance_id, Some(9));
        assert_eq!(pipeline.tts_muted, Some(true));
    }

    #[test]
    fn daemon_exit_clears_connection_and_pipeline() {
        let hub = StatusHub::new();
        hub.set_pipeline(PipelineSnapshot {
            state: "thinking".into(),
            utterance_id: Some(2),
            pause_reason: None,
            tts_muted: None,
        });

        hub.daemon_exited(DaemonStatus::Stopped, Some("daemon exited".into()));

        let status = hub.snapshot();
        assert_eq!(status.daemon, DaemonStatus::Stopped);
        assert_eq!(status.connection, ConnectionStatus::Disconnected);
        assert!(status.pipeline.is_none());
        assert_eq!(status.last_error.as_deref(), Some("daemon exited"));
    }

    #[tokio::test]
    async fn wait_until_connected_resolves_on_the_next_connected_snapshot() {
        let hub = std::sync::Arc::new(StatusHub::new());
        hub.set_connection(ConnectionStatus::Disconnected);
        let waiter = std::sync::Arc::clone(&hub);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            waiter.mark_connecting();
            waiter.mark_connected();
        });

        hub.wait_until_connected(std::time::Duration::from_secs(1))
            .await
            .expect("connection event should resolve the waiter");
    }

    #[tokio::test]
    async fn wait_until_connected_times_out_when_control_stays_down() {
        let hub = StatusHub::new();
        hub.set_connection(ConnectionStatus::Disconnected);
        let result = hub
            .wait_until_connected(std::time::Duration::from_millis(20))
            .await;
        assert!(result.is_err());
    }
}

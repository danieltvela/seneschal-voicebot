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
}

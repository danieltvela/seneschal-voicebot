use crate::permission::PermissionOptionWire;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlEvent {
    /// Pipeline FSM transition. `state` is a stable token:
    /// `idle` | `listening` | `thinking` | `speaking` | `paused`.
    StateChanged {
        state: String,
        utterance_id: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pause_reason: Option<String>,
    },
    Transcript {
        utterance_id: u64,
        text: String,
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
    /// Live input level for the desktop VU meter (issue #34). Linear values in
    /// `[0, 1]`; emitted at the capture chunk cadence.
    AudioLevel {
        rms: f32,
        peak: f32,
    },
    Error {
        message: String,
    },
    /// Download lifecycle for the desktop model manager (issue #35). Emitted
    /// at most every ~200 ms plus terminal transitions; consumers resync via
    /// `GET /control/models`.
    ModelDownloadProgress {
        model_id: String,
        download_id: String,
        /// `downloading` | `verifying` | `completed` | `failed` | `cancelled`.
        phase: String,
        downloaded_bytes: u64,
        /// `None` for indeterminate downloads (NeMo weight scripts).
        #[serde(skip_serializing_if = "Option::is_none")]
        total_bytes: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_file: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    SystemNotification {
        text: String,
    },
    /// Spontaneous notification received from an MCP server (server→client).
    /// Forwarded to Control API subscribers for visibility in dashboards.
    McpNotification {
        server_name: String,
        method: String,
        params: serde_json::Value,
    },
    AgentTaskStarted {
        task_id: String,
        agent_name: String,
        objective: String,
    },
    AgentTaskRunning {
        task_id: String,
        objective: String,
    },
    AgentTaskDelegated {
        task_id: String,
        objective: String,
    },
    AgentTaskFinalizing {
        task_id: String,
        objective: String,
    },
    AgentTaskCompleted {
        task_id: String,
        objective: String,
        result: String,
    },
    AgentTaskFailed {
        task_id: String,
        message: String,
    },
    AgentPermissionRequested {
        task_id: String,
        agent_name: String,
        description: String,
        options: Vec<PermissionOptionWire>,
    },
    AgentPermissionResolved {
        task_id: String,
        option_id: String,
    },
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct ControlBroadcast {
    pub tx: broadcast::Sender<ControlEvent>,
}

#[allow(dead_code)]
impl ControlBroadcast {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn send(&self, event: ControlEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ControlEvent> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(event: &ControlEvent) -> ControlEvent {
        let json = serde_json::to_string(event).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn audio_level_uses_a_stable_wire_shape() {
        let event = ControlEvent::AudioLevel {
            rms: 0.25,
            peak: 0.75,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"audio_level","rms":0.25,"peak":0.75}"#);
        assert!(matches!(
            roundtrip(&event),
            ControlEvent::AudioLevel { rms, peak } if rms == 0.25 && peak == 0.75
        ));
    }

    #[test]
    fn state_changed_thinking_fixture() {
        let event = ControlEvent::StateChanged {
            state: "thinking".into(),
            utterance_id: Some(42),
            pause_reason: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"state_changed","state":"thinking","utterance_id":42}"#
        );
        let back = roundtrip(&event);
        match back {
            ControlEvent::StateChanged {
                state,
                utterance_id,
                pause_reason,
            } => {
                assert_eq!(state, "thinking");
                assert_eq!(utterance_id, Some(42));
                assert_eq!(pause_reason, None);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn state_changed_paused_fixture() {
        let event = ControlEvent::StateChanged {
            state: "paused".into(),
            utterance_id: None,
            pause_reason: Some("consolidation".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"state_changed","state":"paused","utterance_id":null,"pause_reason":"consolidation"}"#
        );
        let _ = roundtrip(&event);
    }

    #[test]
    fn agent_permission_requested_fixture() {
        let event = ControlEvent::AgentPermissionRequested {
            task_id: "t1".into(),
            agent_name: "hermes".into(),
            description: "bash: ls".into(),
            options: vec![
                PermissionOptionWire::with_kind("allow", "Allow once", "allow"),
                PermissionOptionWire::with_kind("deny", "Deny", "reject"),
            ],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"agent_permission_requested""#));
        assert!(json.contains(r#""id":"allow""#));
        let back = roundtrip(&event);
        match back {
            ControlEvent::AgentPermissionRequested { options, .. } => {
                assert_eq!(options.len(), 2);
                assert_eq!(options[0].id, "allow");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn agent_permission_resolved_fixture() {
        let event = ControlEvent::AgentPermissionResolved {
            task_id: "t1".into(),
            option_id: "allow".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"agent_permission_resolved","task_id":"t1","option_id":"allow"}"#
        );
        let _ = roundtrip(&event);
    }

    #[test]
    fn agent_task_completed_fixture() {
        let event = ControlEvent::AgentTaskCompleted {
            task_id: "t1".into(),
            objective: "list files".into(),
            result: "ok\n".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"agent_task_completed","task_id":"t1","objective":"list files","result":"ok\n"}"#
        );
        let _ = roundtrip(&event);
    }

    #[test]
    fn model_download_progress_uses_a_stable_wire_shape() {
        let event = ControlEvent::ModelDownloadProgress {
            model_id: "whisper-tiny".into(),
            download_id: "d-1".into(),
            phase: "downloading".into(),
            downloaded_bytes: 1024,
            total_bytes: Some(4096),
            current_file: Some("ggml-tiny.bin".into()),
            message: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"model_download_progress","model_id":"whisper-tiny","download_id":"d-1","phase":"downloading","downloaded_bytes":1024,"total_bytes":4096,"current_file":"ggml-tiny.bin"}"#
        );
        assert!(matches!(
            roundtrip(&event),
            ControlEvent::ModelDownloadProgress {
                downloaded_bytes: 1024,
                ..
            }
        ));

        let indeterminate = ControlEvent::ModelDownloadProgress {
            model_id: "magpie-tts".into(),
            download_id: "d-2".into(),
            phase: "downloading".into(),
            downloaded_bytes: 0,
            total_bytes: None,
            current_file: None,
            message: Some("nemo-speech pull magpie".into()),
        };
        let json = serde_json::to_string(&indeterminate).unwrap();
        assert!(!json.contains("total_bytes"));
        assert!(json.contains(r#""message":"nemo-speech pull magpie""#));
        let _ = roundtrip(&indeterminate);
    }

    #[test]
    fn all_new_variants_roundtrip() {
        let samples = vec![
            ControlEvent::AgentTaskStarted {
                task_id: "t".into(),
                agent_name: "a".into(),
                objective: "".into(),
            },
            ControlEvent::AgentTaskRunning {
                task_id: "t".into(),
                objective: "work".into(),
            },
            ControlEvent::AgentTaskDelegated {
                task_id: "t".into(),
                objective: "sub".into(),
            },
            ControlEvent::AgentTaskFinalizing {
                task_id: "t".into(),
                objective: "wrap".into(),
            },
            ControlEvent::AgentTaskFailed {
                task_id: "t".into(),
                message: "boom".into(),
            },
            ControlEvent::SystemNotification {
                text: "hello".into(),
            },
            ControlEvent::McpNotification {
                server_name: "s".into(),
                method: "notifications/message".into(),
                params: serde_json::json!({}),
            },
        ];
        for event in samples {
            let _ = roundtrip(&event);
        }
    }
}

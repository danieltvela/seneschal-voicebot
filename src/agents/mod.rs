pub mod config;
pub mod hermes_events;
pub mod opencode_events;
pub mod opencode_transport;
pub mod session_events;
pub mod session_manager;

#[allow(unused_imports)]
pub use config::AgentTomlConfig;
pub use config::{AgentConfig, AgentRegistry};
#[allow(unused_imports)]
pub use hermes_events::{
    HermesEvent, HermesMilestone, extract_milestone as extract_hermes_milestone, parse_hermes_event,
};
#[allow(unused_imports)]
pub use opencode_events::{
    OpenCodeEvent, OpenCodeMilestone, extract_milestone, parse_opencode_event,
};
#[allow(unused_imports)]
pub use opencode_transport::{HttpAgentTransport, OpenCodeHttpTransport};
#[allow(unused_imports)]
pub use session_events::{AcpSessionEvent, create_event_channel, parse_session_update};
#[allow(unused_imports)]
pub use session_manager::{
    AcpSessionManager, SessionEntry, SessionEvent, SessionEventRx, SessionEventTx, SessionInfo,
    SessionStatus, create_session_event_channel,
};

/// Events that trigger proactive speech from seneschal without a user utterance.
pub enum ProactiveEvent {
    /// A background agent task completed. seneschal will vocalize the result.
    ///
    /// When `tool_call_id` is `Some`, the completion came from a background tool
    /// that was invoked by the LLM itself (e.g. `web_search`). The pipeline will
    /// inject the proper OpenAI tool result message into the session and let the
    /// LLM continue naturally instead of re-prompting via a user-role notification.
    AgentResult {
        task: String,
        result: String,
        tool_call_id: Option<String>,
        correlation_id: String,
    },
    /// The inference daemon decided there is something worth saying proactively.
    /// `message` is the raw observation text; `run_proactive_pipeline` will
    /// reformulate it in seneschal's voice before speaking.
    InferenceDaemon { message: String },
    /// An ACP agent is requesting user permission for an action. seneschal speaks
    /// the question, captures the next user utterance, and routes the answer
    /// back via `response_tx`.
    AgentQuestion {
        task_id: String,
        agent_name: String,
        question: String,
        options: Vec<String>,
        /// One-shot channel: send the ACP outcome string ("allow_once" / "reject_once")
        response_tx: tokio::sync::oneshot::Sender<String>,
    },
    /// L1 memory context is saturated — total stored chars exceed the threshold.
    /// The system should prompt the user to allow memory cleanup/reorganization.
    L1Saturated {
        total_chars: usize,
        threshold: usize,
    },
    /// The LLM invoked the switch_plugin tool — the pipeline should rebuild
    /// tool registry, MCP servers, agents, and config for the new plugin.
    PluginSwitch { plugin_id: String },
    /// The configured audio device has become available (e.g. Bluetooth headset
    /// connected). The main loop should start capture, swap the output device,
    /// and send the startup greeting.
    DeviceConnected,
    /// A milestone event from a remote agent (e.g. OpenCode tool invocation).
    /// The pipeline should narrate the milestone via TTS so the user knows
    /// what the agent is doing.
    AgentMilestone {
        agent_name: String,
        milestone: String,
        correlation_id: String,
    },
    /// A spontaneous notification from an MCP server (server→client direction).
    ///
    /// Routed by the MCP reader task when an inbound JSON-RPC message has a
    /// `method` but no `id` (it is a notification, not a response). The main
    /// loop decides how to react — typically by injecting the payload as
    /// context for the next LLM turn or by narrating it via TTS.
    McpNotification {
        server_name: String,
        method: String,
        params: serde_json::Value,
    },
}

impl std::fmt::Debug for ProactiveEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentResult { task, .. } => write!(f, "AgentResult({task:?})"),
            Self::InferenceDaemon { message } => write!(f, "InferenceDaemon({message:?})"),
            Self::AgentQuestion {
                task_id,
                agent_name,
                question,
                options,
                ..
            } => {
                write!(
                    f,
                    "AgentQuestion(task={task_id}, agent={agent_name}, q={question:?}, opts={options:?})"
                )
            }
            Self::L1Saturated {
                total_chars,
                threshold,
            } => {
                write!(
                    f,
                    "L1Saturated(total_chars={total_chars}, threshold={threshold})"
                )
            }
            Self::PluginSwitch { plugin_id } => {
                write!(f, "PluginSwitch(plugin_id={plugin_id})")
            }
            Self::DeviceConnected => write!(f, "DeviceConnected"),
            Self::AgentMilestone {
                agent_name,
                milestone,
                correlation_id,
                ..
            } => {
                write!(
                    f,
                    "AgentMilestone(agent={agent_name}, milestone={milestone:?}, correlation_id={correlation_id})"
                )
            }
            Self::McpNotification {
                server_name,
                method,
                params,
            } => {
                write!(
                    f,
                    "McpNotification(server={server_name}, method={method:?}, params={params})"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_l1_saturated_event_roundtrip() {
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);

        let event = ProactiveEvent::L1Saturated {
            total_chars: 15000,
            threshold: 10000,
        };

        tx.send(event).await.unwrap();

        let received = rx.recv().await.unwrap();
        match received {
            ProactiveEvent::L1Saturated {
                total_chars,
                threshold,
            } => {
                assert_eq!(total_chars, 15000);
                assert_eq!(threshold, 10000);
            }
            other => panic!("Expected L1Saturated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_l1_saturated_debug_format() {
        let event = ProactiveEvent::L1Saturated {
            total_chars: 25000,
            threshold: 20000,
        };
        let debug_str = format!("{event:?}");
        assert!(debug_str.contains("L1Saturated"));
        assert!(debug_str.contains("25000"));
        assert!(debug_str.contains("20000"));
    }

    #[test]
    fn test_mcp_notification_debug_format() {
        let event = ProactiveEvent::McpNotification {
            server_name: "editor".to_string(),
            method: "notifications/document_changed".to_string(),
            params: serde_json::json!({"doc_id": "abc"}),
        };
        let debug_str = format!("{event:?}");
        assert!(debug_str.contains("McpNotification"));
        assert!(debug_str.contains("editor"));
        assert!(debug_str.contains("notifications/document_changed"));
    }

    #[tokio::test]
    async fn test_mcp_notification_roundtrip_channel() {
        // End-to-end: sending McpNotification through an mpsc channel should
        // preserve the payload fields that the main loop reads.
        let (tx, mut rx) = mpsc::channel::<ProactiveEvent>(8);

        let event = ProactiveEvent::McpNotification {
            server_name: "browser".to_string(),
            method: "notifications/page_loaded".to_string(),
            params: serde_json::json!({"url": "https://example.com"}),
        };

        tx.send(event).await.unwrap();

        let received = rx.recv().await.unwrap();
        match received {
            ProactiveEvent::McpNotification {
                server_name,
                method,
                params,
            } => {
                assert_eq!(server_name, "browser");
                assert_eq!(method, "notifications/page_loaded");
                assert_eq!(params["url"], "https://example.com");
            }
            other => panic!("Expected McpNotification, got {other:?}"),
        }
    }
}

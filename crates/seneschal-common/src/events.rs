// Shared event types that flow between pipeline layers.
// Extracted from src/agents/mod.rs and src/plugins/mod.rs to break circular deps.

/// Events that trigger proactive speech from seneschal without a user utterance.
pub enum ProactiveEvent {
    /// A background agent task completed. Seneschal will vocalize the result.
    AgentResult {
        task: String,
        result: String,
        tool_call_id: Option<String>,
        correlation_id: String,
    },
    /// The inference daemon decided there is something worth saying proactively.
    InferenceDaemon { message: String },
    /// An ACP agent is requesting user permission for an action.
    AgentQuestion {
        task_id: String,
        agent_name: String,
        question: String,
        options: Vec<String>,
        response_tx: tokio::sync::oneshot::Sender<String>,
    },
    /// L1 memory context is saturated.
    L1Saturated {
        total_chars: usize,
        threshold: usize,
    },
    /// The LLM invoked the switch_plugin tool.
    PluginSwitch { plugin_id: String },
    /// Audio device became available (e.g. Bluetooth headset).
    DeviceConnected,
    /// A milestone event from a remote agent.
    AgentMilestone {
        agent_name: String,
        milestone: String,
        correlation_id: String,
    },
    /// A spontaneous notification from an MCP server.
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
            } => write!(f, "L1Saturated({total_chars}/{threshold})"),
            Self::PluginSwitch { plugin_id } => write!(f, "PluginSwitch({plugin_id})"),
            Self::DeviceConnected => write!(f, "DeviceConnected"),
            Self::AgentMilestone {
                agent_name,
                milestone,
                correlation_id,
            } => {
                write!(
                    f,
                    "AgentMilestone(agent={agent_name}, milestone={milestone:?}, corr={correlation_id})"
                )
            }
            Self::McpNotification {
                server_name,
                method,
                ..
            } => write!(f, "McpNotification(server={server_name}, method={method})"),
        }
    }
}

/// Plugin lifecycle event sent across the system.
#[derive(Clone, Debug)]
pub enum PluginSwitchEvent {
    Activate { plugin_id: String },
    Deactivate,
}

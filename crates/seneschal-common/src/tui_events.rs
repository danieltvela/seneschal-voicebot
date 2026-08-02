// TUI event types shared between seneschal-core (pipeline) and seneschal-tui.
//
// These types live in seneschal-common so both crates can use them without
// creating a circular dependency.

use tokio::sync::mpsc;

use crate::classifier::{ClassifierLevel, Intent};

/// Pipeline state for the TUI status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineState {
    Idle,
    Listening,
    Transcribing,
    Thinking,
    Speaking,
}

/// Whether a user message originated from voice or keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputSource {
    Voice,
    Text,
}

/// Events sent from the pipeline to the TUI for rendering.
#[derive(Clone, Debug)]
pub enum TuiEvent {
    /// Pipeline state changed.
    StateChange(PipelineState),
    /// User message finalized (from voice STT or typed input).
    UserMessage { text: String, source: InputSource },
    /// Intent classification result for the current user turn (effective intent).
    Classification {
        intent: Intent,
        level: ClassifierLevel,
        /// True when the TUI force override replaced the cascade result.
        forced: bool,
    },
    /// A new LLM token arrived (for streaming display).
    AssistantToken(String),
    /// LLM finished streaming this turn.
    AssistantDone,
    /// A tool was called by the LLM.
    ToolCall { name: String, result: String },
    /// A system-injected notification (memory reorg, background task, etc.).
    SystemNotification { text: String },
    /// A pipeline error occurred that the user should see.
    Error(String),
    /// Show the SENECHAL splash screen on first render.
    Splash,
    /// Prompt-build mode: the prompt text was updated.
    PromptBuildUpdate { prompt: String },
    /// Prompt-build mode: activation state changed.
    PromptBuildStateChange { active: bool },
    /// Agent task lifecycle events (timeline.inline — qwen-audio-agent style).
    /// An agent task was created (LLM delegated to an agent).
    AgentTaskStarted {
        task_id: String,
        agent_name: String,
        objective: String,
    },
    /// The agent is actively processing the task.
    AgentTaskRunning { task_id: String, objective: String },
    /// The agent spawned a sub-delegation (complex multi-step project).
    AgentTaskDelegated { task_id: String, objective: String },
    /// The agent is finalizing / organizing results.
    AgentTaskFinalizing { task_id: String, objective: String },
    /// The agent completed the task successfully. `result` is the final output (Markdown/code).
    AgentTaskCompleted {
        task_id: String,
        objective: String,
        result: String,
    },
    /// The agent is requesting user permission for an action.
    AgentTaskPermissionRequested {
        task_id: String,
        agent_name: String,
        description: String,
        options: Vec<String>,
    },
    /// The agent task failed.
    AgentTaskFailed { task_id: String, message: String },
}

pub type TuiEventTx = mpsc::UnboundedSender<TuiEvent>;
pub type TuiEventRx = mpsc::UnboundedReceiver<TuiEvent>;

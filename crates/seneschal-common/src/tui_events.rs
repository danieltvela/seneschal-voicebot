// TUI event types shared between seneschal-core (pipeline) and seneschal-tui.
//
// These types live in seneschal-common so both crates can use them without
// creating a circular dependency.

use tokio::sync::mpsc;

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
    /// A prompt was sent to a subagent session (outbound user→agent message).
    AgentTaskPrompt { task_id: String, text: String },
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

/// Decide whether the TUI status bar should be force-reset to `Idle`
/// (issue #220, defense-in-depth watchdog).
///
/// A "stuck" state is one that outlived its whole processing window:
/// `Transcribing` with no LLM turn starting behind it, or `Thinking` with
/// no tokens/speech arriving. The watchdog cannot tell the difference
/// between "LLM is working" and "pipeline died" — so it only resets after
/// `timeout` with no activity, and only ONCE per stuck period:
///
/// - `state`: the state currently shown in the TUI status bar.
/// - `elapsed_secs`: seconds since the state last changed.
/// - `timeout_secs`: the patience threshold (30s for MVP).
/// - `already_reset`: this stuck period already produced a reset; while the
///   state stays the same, don't warn/reset again.
///
/// Returns `true` only when the caller must emit `StateChange(Idle)` and a
/// warning. `Listening`, `Speaking` and `Idle` are never auto-reset here:
/// listening is bounded by VAD silence timeouts, speaking by audio playback
/// finishing on its own.
pub fn should_reset_stuck_state(
    state: &PipelineState,
    elapsed_secs: u64,
    timeout_secs: u64,
    already_reset: bool,
) -> bool {
    if already_reset || elapsed_secs < timeout_secs {
        return false;
    }
    matches!(state, PipelineState::Transcribing | PipelineState::Thinking)
}

#[cfg(test)]
mod stuck_state_tests {
    use super::*;

    /// issue #220: Transcribing/Thinking past the timeout must be reset,
    /// and only once per stuck period.
    #[test]
    fn resets_stuck_transcribing_once() {
        assert!(should_reset_stuck_state(
            &PipelineState::Transcribing,
            31,
            30,
            false
        ));
        assert!(should_reset_stuck_state(
            &PipelineState::Thinking,
            30,
            30,
            false
        ));
        // Same stuck period, second tick — no repeat.
        assert!(!should_reset_stuck_state(
            &PipelineState::Transcribing,
            45,
            30,
            true
        ));
    }

    /// No false positives: under the threshold, or already-reset, or in a
    /// state that self-resolves (Listening/Speaking/Idle).
    #[test]
    fn no_false_positives() {
        assert!(!should_reset_stuck_state(
            &PipelineState::Transcribing,
            29,
            30,
            false
        ));
        assert!(!should_reset_stuck_state(
            &PipelineState::Thinking,
            5,
            30,
            false
        ));
        assert!(!should_reset_stuck_state(
            &PipelineState::Listening,
            600,
            30,
            false
        ));
        assert!(!should_reset_stuck_state(
            &PipelineState::Speaking,
            600,
            30,
            false
        ));
        assert!(!should_reset_stuck_state(
            &PipelineState::Idle,
            600,
            30,
            false
        ));
    }
}

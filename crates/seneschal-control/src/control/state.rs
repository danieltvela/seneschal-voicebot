use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, watch};

use super::broadcast::{ControlBroadcast, ControlEvent};
use seneschal_common::PermissionGate;
use seneschal_common::db::Database;
use seneschal_core::llm::LlmSession;
use seneschal_core::pipeline::frames::PipelineFrame;
use seneschal_core::pipeline::fsm::PipelineState;

/// Build the `StateChanged` Control event for a pipeline transition.
///
/// Single source of truth for the FSM → Control wire mapping, shared by the
/// supervisor observer in `main.rs`.
pub fn state_changed_event(state: &PipelineState) -> ControlEvent {
    ControlEvent::StateChanged {
        state: state.control_wire_state().to_string(),
        utterance_id: state.utterance_id(),
        pause_reason: state.control_pause_reason().map(str::to_string),
    }
}

pub struct ControlState {
    pub broadcast: ControlBroadcast,
    pub pipeline_state_rx: watch::Receiver<PipelineState>,
    pub tts_muted: Arc<AtomicBool>,
    pub play_cancel: Arc<AtomicBool>,
    pub barge_in_tx: broadcast::Sender<u64>,
    pub transcript_tx: mpsc::Sender<PipelineFrame>,
    pub llm_session: Arc<Mutex<LlmSession>>,
    pub db: Database,
    /// Shared with the main audio loop (voice permission path).
    pub permission_gate: Arc<PermissionGate>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use seneschal_core::pipeline::PauseReason;

    fn assert_event(
        state: PipelineState,
        wire: &str,
        utterance_id: Option<u64>,
        reason: Option<&str>,
    ) {
        match state_changed_event(&state) {
            ControlEvent::StateChanged {
                state: s,
                utterance_id: u,
                pause_reason: r,
            } => {
                assert_eq!(s, wire, "wire token for {state:?}");
                assert_eq!(u, utterance_id, "utterance id for {state:?}");
                assert_eq!(r.as_deref(), reason, "pause reason for {state:?}");
            }
            other => panic!("expected StateChanged, got {other:?}"),
        }
    }

    /// Every FSM state maps to a `StateChanged` Control event with the stable
    /// wire token, the utterance id of speech states, and the pause reason.
    #[test]
    fn maps_every_pipeline_state_to_a_control_event() {
        assert_event(PipelineState::Idle, "idle", None, None);
        assert_event(
            PipelineState::Listening { utterance_id: 1 },
            "listening",
            Some(1),
            None,
        );
        assert_event(
            PipelineState::Thinking { utterance_id: 2 },
            "thinking",
            Some(2),
            None,
        );
        assert_event(
            PipelineState::Speaking { utterance_id: 3 },
            "speaking",
            Some(3),
            None,
        );
        assert_event(
            PipelineState::Paused {
                reason: PauseReason::Consolidation,
            },
            "paused",
            None,
            Some("consolidation"),
        );
    }
}

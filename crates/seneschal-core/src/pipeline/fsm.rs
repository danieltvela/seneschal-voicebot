/// Global pipeline state, held in a `watch::Sender<PipelineState>`.
///
/// Each actor that owns a transition writes it directly — no central
/// coordinator sits on the hot path. Observers (TUI, logger) subscribe
/// to the `watch::Receiver` via `watch::Receiver::changed()`.
#[derive(Clone, Debug)]
pub enum PipelineState {
    /// No active utterance.
    Idle,

    /// VAD detected speech; STT is accumulating audio.
    Listening { utterance_id: u64 },

    /// Transcript ready; LLM is generating a response.
    Thinking { utterance_id: u64 },

    /// LLM done; TTS is playing the response.
    Speaking { utterance_id: u64 },

    /// Pipeline temporarily paused (e.g. consolidation running).
    Paused { reason: PauseReason },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PauseReason {
    Consolidation,
}

impl PipelineState {
    pub fn utterance_id(&self) -> Option<u64> {
        match self {
            PipelineState::Listening { utterance_id }
            | PipelineState::Thinking { utterance_id }
            | PipelineState::Speaking { utterance_id } => Some(*utterance_id),
            _ => None,
        }
    }

    /// True when the pipeline is doing active work (not Idle).
    pub fn is_busy(&self) -> bool {
        !matches!(self, PipelineState::Idle)
    }

    /// Stable Control API / SSE token for this state (`idle`, `listening`, …).
    ///
    /// Prefer this over `format!("{state:?}")` — Debug strings are not a client contract.
    pub fn control_wire_state(&self) -> &'static str {
        match self {
            PipelineState::Idle => "idle",
            PipelineState::Listening { .. } => "listening",
            PipelineState::Thinking { .. } => "thinking",
            PipelineState::Speaking { .. } => "speaking",
            PipelineState::Paused { .. } => "paused",
        }
    }

    /// Optional pause reason token for Control API (`consolidation`), when `paused`.
    pub fn control_pause_reason(&self) -> Option<&'static str> {
        match self {
            PipelineState::Paused {
                reason: PauseReason::Consolidation,
            } => Some("consolidation"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod control_wire_tests {
    use super::*;

    #[test]
    fn control_wire_state_tokens() {
        assert_eq!(PipelineState::Idle.control_wire_state(), "idle");
        assert_eq!(
            PipelineState::Listening { utterance_id: 1 }.control_wire_state(),
            "listening"
        );
        assert_eq!(
            PipelineState::Thinking { utterance_id: 2 }.control_wire_state(),
            "thinking"
        );
        assert_eq!(
            PipelineState::Speaking { utterance_id: 3 }.control_wire_state(),
            "speaking"
        );
        assert_eq!(
            PipelineState::Paused {
                reason: PauseReason::Consolidation,
            }
            .control_wire_state(),
            "paused"
        );
        assert_eq!(
            PipelineState::Paused {
                reason: PauseReason::Consolidation,
            }
            .control_pause_reason(),
            Some("consolidation")
        );
        assert_eq!(PipelineState::Idle.control_pause_reason(), None);
    }
}

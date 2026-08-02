use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, watch};

use super::broadcast::ControlBroadcast;
use seneschal_common::PermissionGate;
use seneschal_common::db::Database;
use seneschal_core::llm::LlmSession;
use seneschal_core::pipeline::frames::PipelineFrame;
use seneschal_core::pipeline::fsm::PipelineState;

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

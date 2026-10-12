// seneschal-agents — Multi-agent session management for Seneschal.
//
// Provides ACP (Agent Communication Protocol) session lifecycle, OpenCode/Hermes
// HTTP transport, event parsing for both protocols, and visible PTY agent sessions.

pub mod agent_session;
pub mod config;
pub mod hermes_events;
pub mod opencode_events;
pub mod opencode_transport;
pub mod session_events;
pub mod session_manager;

// Re-export commonly used types.
pub use agent_session::{VisibleSession, VisibleSessionManager};
pub use config::{AgentConfig, AgentRegistry, AgentTomlConfig};
pub use hermes_events::{
    HermesEvent, HermesMilestone, extract_milestone as extract_hermes_milestone, parse_hermes_event,
};
pub use opencode_events::{
    OpenCodeEvent, OpenCodeMilestone, extract_milestone, parse_opencode_event,
};
pub use opencode_transport::{HttpAgentTransport, OpenCodeHttpTransport};
pub use session_events::{AcpSessionEvent, create_event_channel, parse_session_update};
pub use session_manager::{
    AcpSessionManager, SessionEntry, SessionEvent, SessionEventRx, SessionEventTx, SessionInfo,
    SessionStatus, create_session_event_channel,
};

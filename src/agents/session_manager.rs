use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use dashmap::DashMap;

use super::config::AgentConfig;
use crate::tools::run_agent::AcpWriter;

/// Display-friendly session summary.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_name: String,
    pub created_at: Instant,
    pub last_used: Instant,
}

/// Handle to a live ACP session.
#[derive(Clone)]
pub struct SessionEntry {
    pub writer: Arc<Mutex<AcpWriter>>,
    pub session_id: String,
    pub agent_name: String,
    pub created_at: Instant,
    pub last_used: Instant,
}

impl std::fmt::Debug for SessionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEntry")
            .field("session_id", &self.session_id)
            .field("agent_name", &self.agent_name)
            .field("created_at", &self.created_at)
            .field("last_used", &self.last_used)
            .finish()
    }
}

/// Manages persistent ACP sessions keyed by agent name.
#[derive(Debug, Default)]
pub struct AcpSessionManager {
    sessions: DashMap<String, SessionEntry>,
}

impl AcpSessionManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieve an existing session for `agent_config.name`, or create one.
    pub async fn get_or_create_session(
        &self,
        agent_config: &AgentConfig,
    ) -> Result<SessionEntry> {
        if let Some(mut entry) = self.sessions.get_mut(&agent_config.name) {
            entry.last_used = Instant::now();
            return Ok((*entry.value()).clone());
        }

        let (writer, _inbound_rx) = AcpWriter::spawn(&agent_config.acp_command).await?;
        let now = Instant::now();
        let entry = SessionEntry {
            writer: Arc::new(Mutex::new(writer)),
            session_id: uuid::Uuid::new_v4().to_string(),
            agent_name: agent_config.name.clone(),
            created_at: now,
            last_used: now,
        };
        self.sessions.insert(agent_config.name.clone(), entry.clone());
        Ok(entry)
    }

    /// Close and remove the session identified by `session_id`.
    pub fn close_session(&self, session_id: &str) {
        self.sessions.retain(|_, v| v.session_id != session_id);
    }

    /// Return information about all active sessions.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|e| SessionInfo {
                session_id: e.session_id.clone(),
                agent_name: e.agent_name.clone(),
                created_at: e.created_at,
                last_used: e.last_used,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dummy_entry(session_id: &str, agent_name: &str) -> SessionEntry {
        SessionEntry {
            writer: Arc::new(Mutex::new(AcpWriter::dummy())),
            session_id: session_id.to_string(),
            agent_name: agent_name.to_string(),
            created_at: Instant::now(),
            last_used: Instant::now(),
        }
    }

    #[tokio::test]
    async fn new_manager_is_empty() {
        assert!(AcpSessionManager::new().list_sessions().is_empty());
    }

    #[tokio::test]
    async fn close_session_removes_matching_id() {
        let mgr = AcpSessionManager::new();
        mgr.sessions.insert("hermes".into(), make_dummy_entry("sid-1", "hermes"));
        assert_eq!(mgr.list_sessions().len(), 1);
        mgr.close_session("sid-1");
        assert!(mgr.list_sessions().is_empty());
    }

    #[tokio::test]
    async fn close_session_ignores_unknown_id() {
        let mgr = AcpSessionManager::new();
        mgr.sessions.insert("hermes".into(), make_dummy_entry("sid-1", "hermes"));
        mgr.close_session("nonexistent");
        assert_eq!(mgr.list_sessions().len(), 1);
    }

    #[tokio::test]
    async fn list_sessions_returns_correct_info() {
        let mgr = AcpSessionManager::new();
        mgr.sessions.insert("hermes".into(), make_dummy_entry("sid-alpha", "hermes"));
        let list = mgr.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, "sid-alpha");
        assert_eq!(list[0].agent_name, "hermes");
    }

    #[tokio::test]
    async fn list_sessions_multiple_agents() {
        let mgr = AcpSessionManager::new();
        for name in ["hermes", "oracle", "metis"] {
            mgr.sessions.insert(name.into(), make_dummy_entry(&format!("sid-{name}"), name));
        }
        assert_eq!(mgr.list_sessions().len(), 3);
    }
}

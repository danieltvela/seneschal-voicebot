use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::tools::registry::ToolResult;

/// Manages external AI agents like OpenClaw
pub struct AgentManager {
    agents: Arc<RwLock<HashMap<String, Box<dyn Agent>>>>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an external agent
    pub async fn register_agent(&self, agent: Box<dyn Agent>) -> Result<()> {
        let mut agents = self.agents.write().await;
        let name = agent.name().to_string();
        agents.insert(name, agent);
        Ok(())
    }

    /// Check if agent exists
    pub async fn has_agent(&self, name: &str) -> bool {
        let agents = self.agents.read().await;
        agents.contains_key(name)
    }

    /// Call an external agent
    pub async fn call_agent(&self, name: &str, request: &str) -> Result<ToolResult> {
        let agents = self.agents.read().await;
        let agent = agents
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Agent not found: {}", name))?;

        agent.execute(request).await
    }

    /// List all available agents
    pub async fn list_agents(&self) -> Vec<String> {
        let agents = self.agents.read().await;
        agents.keys().cloned().collect()
    }
}

impl Default for AgentManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent trait - Interface for external agents
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    /// Agent name
    fn name(&self) -> &str;

    /// Agent description
    fn description(&self) -> &str;

    /// Execute agent request
    async fn execute(&self, request: &str) -> Result<ToolResult>;
}

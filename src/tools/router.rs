use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::registry::{ToolRegistry, ToolResult};
use crate::agents::AgentManager;
use crate::mcp::McpServer;

/// Routes tool calls between S2S model, MCP server, and external agents
pub struct ToolRouter {
    registry: Arc<RwLock<ToolRegistry>>,
    mcp_server: Option<Arc<McpServer>>,
    agent_manager: Option<Arc<AgentManager>>,
}

impl ToolRouter {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(ToolRegistry::new())),
            mcp_server: None,
            agent_manager: None,
        }
    }

    /// Set MCP server for tool execution
    pub fn with_mcp_server(mut self, server: Arc<McpServer>) -> Self {
        self.mcp_server = Some(server);
        self
    }

    /// Set agent manager for external agent calls
    pub fn with_agent_manager(mut self, manager: Arc<AgentManager>) -> Self {
        self.agent_manager = Some(manager);
        self
    }

    /// Execute a tool call
    pub async fn execute_tool(&self, name: &str, arguments: &str) -> Result<ToolResult> {
        // Check if it's a built-in tool
        let registry = self.registry.read().await;
        if let Some(tool) = registry.get_tool(name) {
            return tool.execute(arguments).await;
        }
        drop(registry);

        // Check if it's an MCP tool
        if let Some(mcp) = &self.mcp_server {
            if mcp.has_tool(name).await {
                return mcp.execute_tool(name, arguments).await;
            }
        }

        // Check if it's an agent call
        if let Some(agents) = &self.agent_manager {
            if agents.has_agent(name).await {
                return agents.call_agent(name, arguments).await;
            }
        }

        anyhow::bail!("Tool not found: {}", name)
    }

    /// Get all available tools
    pub async fn list_tools(&self) -> Vec<String> {
        let mut tools = Vec::new();

        // Built-in tools
        let registry = self.registry.read().await;
        tools.extend(registry.list_tools());
        drop(registry);

        // MCP tools
        if let Some(mcp) = &self.mcp_server {
            tools.extend(mcp.list_tools().await);
        }

        // Agent tools
        if let Some(agents) = &self.agent_manager {
            tools.extend(agents.list_agents().await);
        }

        tools
    }

    /// Register a built-in tool
    pub async fn register_tool(&self, tool: Box<dyn super::registry::Tool>) -> Result<()> {
        let mut registry = self.registry.write().await;
        registry.register(tool)
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

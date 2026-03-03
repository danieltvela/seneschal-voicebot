use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tool registry for managing available tools
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a new tool
    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<()> {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Get a tool by name
    pub fn get_tool(&self, name: &str) -> Option<&Box<dyn Tool>> {
        self.tools.get(name)
    }

    /// List all registered tools
    pub fn list_tools(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Get tool definitions for model
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Tool trait - Interface for all tools
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;

    /// Tool description
    fn description(&self) -> &str;

    /// Tool definition (schema)
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with given arguments
    async fn execute(&self, arguments: &str) -> Result<ToolResult>;
}

/// Tool definition for model integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Result from tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(output: String) -> Self {
        Self {
            success: true,
            output,
            error: None,
        }
    }

    pub fn error(error: String) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error),
        }
    }
}

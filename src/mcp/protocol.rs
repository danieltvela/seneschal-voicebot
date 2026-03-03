use serde::{Deserialize, Serialize};

/// MCP protocol request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    pub tool: String,
    pub arguments: String,
}

/// MCP protocol response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResponse {
    pub success: bool,
    pub result: String,
    pub error: Option<String>,
}

/// MCP tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
}

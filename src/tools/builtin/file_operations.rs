use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::tools::registry::{Tool, ToolDefinition, ToolResult};

/// File operations tool
pub struct FileOperationsTool;

impl FileOperationsTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct FileOperationArgs {
    operation: String,
    path: String,
    content: Option<String>,
}

#[async_trait]
impl Tool for FileOperationsTool {
    fn name(&self) -> &str {
        "file_operations"
    }

    fn description(&self) -> &str {
        "Perform file operations like read, write, list, delete"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["read", "write", "list", "delete"],
                        "description": "The operation to perform"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory path"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content for write operation"
                    }
                },
                "required": ["operation", "path"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult> {
        let args: FileOperationArgs = serde_json::from_str(arguments)?;

        match args.operation.as_str() {
            "read" => {
                match tokio::fs::read_to_string(&args.path).await {
                    Ok(content) => Ok(ToolResult::success(content)),
                    Err(e) => Ok(ToolResult::error(format!("Failed to read file: {}", e))),
                }
            }
            "write" => {
                let content = args.content.unwrap_or_default();
                match tokio::fs::write(&args.path, content).await {
                    Ok(_) => Ok(ToolResult::success("File written successfully".to_string())),
                    Err(e) => Ok(ToolResult::error(format!("Failed to write file: {}", e))),
                }
            }
            "list" => {
                match tokio::fs::read_dir(&args.path).await {
                    Ok(mut entries) => {
                        let mut files = Vec::new();
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if let Ok(name) = entry.file_name().into_string() {
                                files.push(name);
                            }
                        }
                        Ok(ToolResult::success(files.join("\n")))
                    }
                    Err(e) => Ok(ToolResult::error(format!("Failed to list directory: {}", e))),
                }
            }
            "delete" => {
                match tokio::fs::remove_file(&args.path).await {
                    Ok(_) => Ok(ToolResult::success("File deleted successfully".to_string())),
                    Err(e) => Ok(ToolResult::error(format!("Failed to delete file: {}", e))),
                }
            }
            _ => Ok(ToolResult::error("Unknown operation".to_string())),
        }
    }
}

impl Default for FileOperationsTool {
    fn default() -> Self {
        Self::new()
    }
}

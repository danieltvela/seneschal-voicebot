use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use sysinfo::System;

use crate::tools::registry::{Tool, ToolDefinition, ToolResult};

/// System information tool
pub struct SystemInfoTool;

impl SystemInfoTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct SystemInfoArgs {
    info_type: String,
}

#[async_trait]
impl Tool for SystemInfoTool {
    fn name(&self) -> &str {
        "system_info"
    }

    fn description(&self) -> &str {
        "Get system information like CPU, memory, disk usage"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "info_type": {
                        "type": "string",
                        "enum": ["cpu", "memory", "disk", "all"],
                        "description": "Type of system information to retrieve"
                    }
                },
                "required": ["info_type"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult> {
        let args: SystemInfoArgs = serde_json::from_str(arguments)?;
        let mut sys = System::new_all();
        sys.refresh_all();

        let info = match args.info_type.as_str() {
            "cpu" => {
                format!("CPU Usage: {}%", sys.global_cpu_usage())
            }
            "memory" => {
                let total = sys.total_memory() / 1024 / 1024;
                let used = sys.used_memory() / 1024 / 1024;
                format!("Memory: {} MB / {} MB", used, total)
            }
            "disk" => {
                // TODO: Implement disk info
                "Disk info not implemented yet".to_string()
            }
            "all" => {
                let cpu = sys.global_cpu_usage();
                let total_mem = sys.total_memory() / 1024 / 1024;
                let used_mem = sys.used_memory() / 1024 / 1024;
                format!(
                    "CPU: {}%, Memory: {} MB / {} MB",
                    cpu, used_mem, total_mem
                )
            }
            _ => return Ok(ToolResult::error("Unknown info type".to_string())),
        };

        Ok(ToolResult::success(info))
    }
}

impl Default for SystemInfoTool {
    fn default() -> Self {
        Self::new()
    }
}

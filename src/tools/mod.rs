// Tools — all implementations moved to seneschal-tools-core and seneschal-extras.
// Re-exports for backward compatibility.

pub mod mcp_tool;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::info;

pub use mcp_tool::McpToolProxy;

// Re-export tools moved to seneschal-tools-core
pub use seneschal_tools_core::AppleEventsTool;
pub use seneschal_tools_core::{ReadClipboardTool, SetClipboardTool};
pub use seneschal_tools_core::CurrentTimeTool;
pub use seneschal_tools_core::NoopTool;
pub use seneschal_tools_core::OpenAppTool;
#[cfg(target_os = "macos")]
pub use seneschal_tools_core::OpenTerminalTool;
pub use seneschal_tools_core::QuickSearchTool;
pub use seneschal_tools_core::ReadFileTool;
pub use seneschal_tools_core::RunShellTool;
pub use seneschal_tools_core::WebSearchTool;

// Re-export tools moved to seneschal-extras
pub use seneschal_extras::conversation_mode::SetConversationModeTool;
pub use seneschal_common::tools::ConversationMode;
pub use seneschal_extras::DeepResearchTool;
pub use seneschal_extras::prompt_build::SetPromptBuildTool;
pub use seneschal_extras::RecoverHistoricalContextTool;
pub use seneschal_extras::run_agent::{
    AcpWriter, ActiveTask, PendingInteractionEntry, RunAgentTool, format_history,
};
pub use seneschal_extras::subtask::{ListTasksTool, SubtaskTracker};
pub use seneschal_extras::SwitchPluginTool;
pub use seneschal_extras::TakeScreenshotTool;

// Re-export types that moved to other crates
pub use seneschal_common::acp_writer::JsonRpcMessage;
pub use seneschal_common::tools::{Tool, ToolRegistry};

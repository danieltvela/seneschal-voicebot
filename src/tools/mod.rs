// Core tools (moved to seneschal-tools-core — re-export for compat)
// apple_events, clipboard, current_time, noop, open_app, open_terminal,
// quick_search, read_file, run_shell, web_search → seneschal_tools_core

pub mod conversation_mode;
pub mod deep_research;
pub mod mcp_tool;
pub mod prompt_build;
pub mod recover_historical_context;
pub mod run_agent;
pub mod subtask;
pub mod switch_plugin;
pub mod take_screenshot;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::info;

pub use conversation_mode::SetConversationModeTool;
pub use seneschal_common::tools::ConversationMode;
pub use deep_research::DeepResearchTool;
pub use mcp_tool::McpToolProxy;
pub use prompt_build::SetPromptBuildTool;
#[allow(unused_imports)]
pub use recover_historical_context::RecoverHistoricalContextTool;
#[allow(unused_imports)]
pub use run_agent::{
    AcpWriter, ActiveTask, JsonRpcMessage, PendingInteractionEntry, RunAgentTool, format_history,
};
pub use subtask::{ListTasksTool, SubtaskTracker};
pub use switch_plugin::SwitchPluginTool;
pub use take_screenshot::TakeScreenshotTool;

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

// Re-exported from seneschal-common to avoid type conflicts.
pub use seneschal_common::tools::{Tool, ToolRegistry};

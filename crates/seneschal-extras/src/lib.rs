// seneschal-extras — Optional/unstable features for Seneschal.
//
// Catch-all crate for daemons, visual awareness, advanced tools, and
// agent bridging that are not part of the core voice pipeline.

pub mod agent_bridge;
pub mod analysis;
pub mod conversation_mode;
pub mod daemon;
pub mod deep_research;
pub mod device_monitor;
pub mod eyes;
pub mod permission_tool;
pub mod prompt_build;
pub mod recover_historical_context;
pub mod run_agent;
pub mod screen_capture;
pub mod subtask;
pub mod switch_plugin;
pub mod take_screenshot;

// Re-export Tool trait so `super::Tool` works in submodules.
pub use seneschal_common::tools::Tool;

// Commonly used types
pub use agent_bridge::{register_plugin_agent_tools, resolve_plugin_agents};
pub use conversation_mode::SetConversationModeTool;
pub use daemon::{AcpKeepAliveDaemon, InferenceDaemon};
pub use deep_research::DeepResearchTool;
pub use eyes::EyesDaemon;
pub use permission_tool::RespondAgentPermissionTool;
pub use prompt_build::SetPromptBuildTool;
pub use recover_historical_context::RecoverHistoricalContextTool;
pub use run_agent::{
    ActiveTask, AgentTaskHandle, PendingInteractionEntry, RunAgentTool, TaskState,
};
pub use seneschal_common::tools::ConversationMode;
pub use subtask::{ListTasksTool, SubtaskTracker};
pub use switch_plugin::SwitchPluginTool;
pub use take_screenshot::TakeScreenshotTool;

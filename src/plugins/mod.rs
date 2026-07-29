pub mod agent_bridge;
pub mod config_overrides;
pub mod manager;
pub mod manifest;
pub mod mcp_spawner;
pub mod prompt_injection;

pub use config_overrides::OriginalConfigSnapshot;
pub use manager::PluginManager;
pub use manifest::{PluginPromptConfig, PromptMode};
pub use mcp_spawner::SpawnedMcpServers;
pub use prompt_injection::build_plugin_prompt_section;

pub use seneschal_common::events::{PluginPromptSections, PluginSwitchEvent};

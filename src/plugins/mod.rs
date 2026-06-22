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
pub use prompt_injection::{PluginPromptSections, build_plugin_prompt_section};

#[derive(Clone, Debug)]
pub enum PluginSwitchEvent {
    Activate { plugin_id: String },
    Deactivate,
}

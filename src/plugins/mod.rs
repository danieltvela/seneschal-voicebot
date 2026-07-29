// Plugins system — core implementation moved to seneschal-plugins crate.
// Re-exports for backward compatibility. agent_bridge remains here because
// it depends on tools/run_agent types not yet extracted.

pub mod agent_bridge;

pub use seneschal_plugins::config_overrides;
pub use seneschal_plugins::manager;
pub use seneschal_plugins::manifest;
pub use seneschal_plugins::mcp_spawner;
pub use seneschal_plugins::prompt_injection;
pub use seneschal_plugins::{
    OriginalConfigSnapshot, PluginManager, PluginPromptConfig, PromptMode, SpawnedMcpServers,
    build_plugin_prompt_section,
};
pub use seneschal_common::events::{PluginPromptSections, PluginSwitchEvent};

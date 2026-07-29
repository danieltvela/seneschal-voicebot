// Plugins system — implementation moved to seneschal-plugins and seneschal-extras.
// Re-exports for backward compatibility.

pub use seneschal_plugins::{
    OriginalConfigSnapshot, PluginManager, PluginPromptConfig, PromptMode, SpawnedMcpServers,
    build_plugin_prompt_section,
};
pub use seneschal_common::events::{PluginPromptSections, PluginSwitchEvent};
pub use seneschal_extras::agent_bridge::{register_plugin_agent_tools, resolve_plugin_agents};

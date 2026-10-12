// seneschal-plugins — Plugin system for Seneschal.
//
// Provides plugin lifecycle management, manifest parsing, config overrides,
// MCP server spawning, and prompt injection for agent delegation.

pub mod config_overrides;
pub mod manager;
pub mod manifest;
pub mod mcp_spawner;
pub mod prompt_injection;

pub use config_overrides::{OriginalConfigSnapshot, PluginConfigOverrides};
pub use manager::PluginManager;
pub use manifest::{
    McpServerConfig, PluginAgentConfig, PluginManifest, PluginPromptConfig, PromptMode,
};
pub use mcp_spawner::SpawnedMcpServers;
pub use prompt_injection::{PluginPromptSections, build_plugin_prompt_section};

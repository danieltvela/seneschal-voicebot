use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::agents::AgentConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    Replace,
    Append,
    Both,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginPromptConfig {
    pub mode: PromptMode,
    pub content: String,
    #[serde(default)]
    pub prepend: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

fn default_tool_timeout_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginAgentConfig {
    pub name: String,
    pub mode: String,
    pub command: Option<String>,
    pub acp_command: Option<String>,
    #[serde(default)]
    pub acp_warmup: bool,
    #[serde(default)]
    pub remote_url: String,
    #[serde(default)]
    pub remote_dir: String,
    #[serde(default)]
    pub remote_session_path: String,
    #[serde(default)]
    pub remote_message_path: String,
    #[serde(default)]
    pub remote_event_path: String,
    #[serde(default)]
    pub remote_api_key: String,
    pub when_to_use: String,
    pub instructions: String,
}

impl From<PluginAgentConfig> for AgentConfig {
    fn from(src: PluginAgentConfig) -> Self {
        Self {
            name: src.name,
            mode: src.mode,
            command: src.command,
            acp_command: src.acp_command.unwrap_or_default(),
            acp_warmup: src.acp_warmup,
            remote_url: src.remote_url,
            remote_dir: src.remote_dir,
            remote_session_path: src.remote_session_path,
            remote_message_path: src.remote_message_path,
            remote_event_path: src.remote_event_path,
            remote_api_key: src.remote_api_key,
            when_to_use: src.when_to_use,
            instructions: src.instructions,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub assistant_name: String,
    pub description: String,
    pub version: String,
    pub prompt: PluginPromptConfig,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub agents: Vec<PluginAgentConfig>,
    #[serde(default)]
    pub requires_permissions: HashSet<String>,
}

impl PluginManifest {
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read plugin manifest at {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse plugin manifest at {}", path.display()))
    }
}

impl std::str::FromStr for PluginManifest {
    type Err = anyhow::Error;

    fn from_str(content: &str) -> Result<Self> {
        toml::from_str(content).context("Failed to parse plugin manifest")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::str::FromStr;

    fn minimal_manifest_toml() -> String {
        r#"
name = "test-plugin"
assistant_name = "TestBot"
description = "A test plugin"
version = "0.1.0"

[prompt]
mode = "append"
content = "Test prompt content"
"#
        .trim()
        .to_string()
    }

    fn full_manifest_toml() -> String {
        r#"
name = "full-plugin"
assistant_name = "FullBot"
description = "A fully-configured test plugin"
version = "1.0.0"
requires_permissions = ["network", "filesystem"]

[prompt]
mode = "both"
content = "Full prompt"
prepend = true

[[mcp_servers]]
name = "test-mcp"
command = "npx mcp-server"
tool_timeout_secs = 60

[[agents]]
name = "test-agent"
mode = "acp"
when_to_use = "testing"
instructions = "be helpful"
"#
        .trim()
        .to_string()
    }

    // ── from_str ──────────────────────────────────────────────────────────────

    #[test]
    fn from_str_parses_minimal_manifest() {
        let manifest = PluginManifest::from_str(&minimal_manifest_toml()).unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.assistant_name, "TestBot");
        assert_eq!(manifest.description, "A test plugin");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.prompt.mode, PromptMode::Append);
        assert_eq!(manifest.prompt.content, "Test prompt content");
        assert!(!manifest.prompt.prepend);
        assert!(manifest.mcp_servers.is_empty());
        assert!(manifest.agents.is_empty());
        assert!(manifest.requires_permissions.is_empty());
    }

    #[test]
    fn from_str_parses_full_manifest() {
        let manifest = PluginManifest::from_str(&full_manifest_toml()).unwrap();
        assert_eq!(manifest.name, "full-plugin");
        assert_eq!(manifest.prompt.mode, PromptMode::Both);
        assert!(manifest.prompt.prepend);
        assert_eq!(manifest.mcp_servers.len(), 1);
        assert_eq!(manifest.mcp_servers[0].name, "test-mcp");
        assert_eq!(manifest.mcp_servers[0].command, "npx mcp-server");
        assert_eq!(manifest.mcp_servers[0].tool_timeout_secs, 60);
        assert_eq!(manifest.agents.len(), 1);
        assert_eq!(manifest.agents[0].name, "test-agent");
        assert_eq!(manifest.agents[0].mode, "acp");
        assert!(!manifest.agents[0].acp_warmup);
        assert_eq!(manifest.requires_permissions.len(), 2);
        assert!(manifest.requires_permissions.contains("network"));
        assert!(manifest.requires_permissions.contains("filesystem"));
    }

    #[test]
    fn from_str_rejects_invalid_toml() {
        let result = PluginManifest::from_str("not valid toml {{{");
        assert!(result.is_err());
    }

    #[test]
    fn from_str_rejects_missing_required_field() {
        let result = PluginManifest::from_str(r#"name = "test""#);
        assert!(result.is_err());
    }

    // ── PromptMode deserialization ────────────────────────────────────────────

    #[test]
    fn prompt_mode_deserializes_replace() {
        let manifest = PluginManifest::from_str(
            r#"
name = "p"
assistant_name = "A"
description = "D"
version = "1"
[prompt]
mode = "replace"
content = "C"
"#,
        )
        .unwrap();
        assert_eq!(manifest.prompt.mode, PromptMode::Replace);
    }

    #[test]
    fn prompt_mode_deserializes_append() {
        let manifest = PluginManifest::from_str(
            r#"
name = "p"
assistant_name = "A"
description = "D"
version = "1"
[prompt]
mode = "append"
content = "C"
"#,
        )
        .unwrap();
        assert_eq!(manifest.prompt.mode, PromptMode::Append);
    }

    #[test]
    fn prompt_mode_deserializes_both() {
        let manifest = PluginManifest::from_str(
            r#"
name = "p"
assistant_name = "A"
description = "D"
version = "1"
[prompt]
mode = "both"
content = "C"
"#,
        )
        .unwrap();
        assert_eq!(manifest.prompt.mode, PromptMode::Both);
    }

    #[test]
    fn prompt_mode_rejects_unknown_variant() {
        let result = PluginManifest::from_str(
            r#"
name = "p"
assistant_name = "A"
description = "D"
version = "1"
[prompt]
mode = "invalid"
content = "C"
"#,
        );
        assert!(result.is_err());
    }

    // ── from_file ─────────────────────────────────────────────────────────────

    #[test]
    fn from_file_reads_valid_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("plugin.toml");
        let mut file = std::fs::File::create(&manifest_path).unwrap();
        writeln!(file, "{}", minimal_manifest_toml()).unwrap();

        let manifest = PluginManifest::from_file(&manifest_path).unwrap();
        assert_eq!(manifest.name, "test-plugin");
    }

    #[test]
    fn from_file_fails_on_missing_file() {
        let result = PluginManifest::from_file(Path::new("/nonexistent/path/plugin.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn from_file_fails_on_invalid_content() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("plugin.toml");
        std::fs::write(&manifest_path, "not toml at all {{{").unwrap();

        let result = PluginManifest::from_file(&manifest_path);
        assert!(result.is_err());
    }

    // ── McpServerConfig defaults ──────────────────────────────────────────────

    #[test]
    fn mcp_server_default_timeout_is_30() {
        assert_eq!(default_tool_timeout_secs(), 30);
    }

    #[test]
    fn mcp_server_custom_timeout_is_respected() {
        let manifest = PluginManifest::from_str(&full_manifest_toml()).unwrap();
        assert_eq!(manifest.mcp_servers[0].tool_timeout_secs, 60);
    }

    // ── PluginAgentConfig to AgentConfig ──────────────────────────────────────

    #[test]
    fn plugin_agent_config_converts_to_agent_config() {
        let manifest = PluginManifest::from_str(&full_manifest_toml()).unwrap();
        let pac = &manifest.agents[0];
        let ac: AgentConfig = pac.clone().into();
        assert_eq!(ac.name, "test-agent");
        assert_eq!(ac.mode, "acp");
        assert_eq!(ac.when_to_use, "testing");
        assert_eq!(ac.instructions, "be helpful");
        assert_eq!(ac.acp_command, "");
        assert!(!ac.acp_warmup);
    }
}

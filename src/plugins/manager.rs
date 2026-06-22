use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::{info, warn};

use crate::plugins::config_overrides::{OriginalConfigSnapshot, PluginConfigOverrides};
use crate::plugins::manifest::{
    McpServerConfig, PluginAgentConfig, PluginManifest, PluginPromptConfig,
};

/// Metadata for a plugin loaded from disk.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub manifest: PluginManifest,
    pub path: PathBuf,
    pub config_overrides: PluginConfigOverrides,
}

/// Data returned by `activate()` so the caller can wire up tools, MCP servers, agents, and config.
#[derive(Debug)]
pub struct ActivatedPlugin {
    pub id: String,
    pub manifest: PluginManifest,
    pub prompt: PluginPromptConfig,
    pub mcp_servers: Vec<McpServerConfig>,
    pub agents: Vec<PluginAgentConfig>,
    pub config_overrides: PluginConfigOverrides,
    /// Tool names that were registered by the previous plugin (if any) and should be unregistered.
    pub previous_tool_names: Vec<String>,
    /// MCP server names from the previous plugin that should be torn down.
    pub previous_mcp_names: Vec<String>,
    /// Agent names from the previous plugin that should be removed.
    pub previous_agent_names: Vec<String>,
}

/// Data returned by `deactivate()` so the caller can clean up what the plugin contributed.
#[derive(Debug)]
pub struct DeactivatedPlugin {
    pub id: String,
    /// Tool names the plugin registered that should be unregistered.
    pub tool_names: Vec<String>,
    /// MCP server names that should be torn down.
    pub mcp_names: Vec<String>,
    /// Agent names that should be removed.
    pub agent_names: Vec<String>,
    pub config_overrides: PluginConfigOverrides,
    pub config_snapshot: OriginalConfigSnapshot,
}

/// Internal mutable state protected by Mutex.
#[derive(Default)]
struct PluginManagerInner {
    /// All successfully-loaded plugins keyed by id.
    available: HashMap<String, PluginInfo>,
    /// Currently active plugin id, if any.
    active_id: Option<String>,
    /// Tool names registered by the currently active plugin.
    active_tool_names: Vec<String>,
    /// MCP server names from the currently active plugin.
    active_mcp_names: Vec<String>,
    /// Agent names from the currently active plugin.
    active_agent_names: Vec<String>,
    /// Snapshot of config taken before the active plugin's overrides were applied.
    active_config_snapshot: Option<OriginalConfigSnapshot>,
}

/// Thread-safe manager for the plugin lifecycle.
///
/// Responsibilities:
/// - Load plugins from disk at construction time
/// - Track which plugin is active
/// - Return activation/deactivation data so the caller can wire up tools, MCP servers, agents, and config
///
/// This manager does NOT:
/// - Spawn MCP subprocesses (caller's job)
/// - Modify ToolRegistry directly (returns tool data)
/// - Modify Config directly (returns config overrides)
/// - Block on async operations inside the mutex
#[derive(Clone)]
pub struct PluginManager {
    inner: Arc<Mutex<PluginManagerInner>>,
}

impl PluginManager {
    /// Load all valid plugins from the given directory paths.
    ///
    /// For each path:
    /// - If it's a file ending in `.toml`, try to parse it as a manifest.
    /// - If it's a directory, look for `plugin.toml` inside it.
    /// - Look for `config.toml` alongside the manifest for config overrides.
    ///
    /// Invalid plugins are skipped with a warning log.
    pub fn new(plugin_paths: &[PathBuf]) -> Self {
        let mut inner = PluginManagerInner::default();

        for path in plugin_paths {
            let manifest_path = if path.is_file() {
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    warn!(path = %path.display(), "Skipping non-toml file in plugin paths");
                    continue;
                }
                path.clone()
            } else if path.is_dir() {
                let candidate = path.join("plugin.toml");
                if !candidate.exists() {
                    warn!(path = %path.display(), "No plugin.toml found in directory, skipping");
                    continue;
                }
                candidate
            } else {
                warn!(path = %path.display(), "Plugin path does not exist, skipping");
                continue;
            };

            let plugin = match Self::load_plugin(&manifest_path) {
                Ok(p) => p,
                Err(e) => {
                    warn!(path = %manifest_path.display(), error = %e, "Failed to load plugin, skipping");
                    continue;
                }
            };

            inner.available.insert(plugin.id.clone(), plugin);
        }

        let count = inner.available.len();
        info!(count, "PluginManager initialized");

        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Load a single plugin from its manifest path.
    fn load_plugin(manifest_path: &std::path::Path) -> Result<PluginInfo> {
        let manifest = PluginManifest::from_file(manifest_path)?;
        let id = manifest.name.clone();

        // Try to load config overrides from sibling config.toml
        let config_overrides = {
            let config_path = manifest_path
                .parent()
                .map(|p| p.join("config.toml"))
                .filter(|p| p.exists());

            if let Some(ref cp) = config_path {
                match std::fs::read_to_string(cp) {
                    Ok(contents) => match toml::from_str::<PluginConfigOverrides>(&contents) {
                        Ok(overrides) => {
                            info!(id, "Loaded plugin config overrides");
                            overrides
                        }
                        Err(e) => {
                            warn!(id, error = %e, "Failed to parse plugin config overrides, using defaults");
                            PluginConfigOverrides::default()
                        }
                    },
                    Err(e) => {
                        warn!(id, error = %e, "Failed to read plugin config.toml, using defaults");
                        PluginConfigOverrides::default()
                    }
                }
            } else {
                PluginConfigOverrides::default()
            }
        };

        Ok(PluginInfo {
            id,
            manifest,
            path: manifest_path.to_path_buf(),
            config_overrides,
        })
    }

    /// Activate a plugin by id.
    ///
    /// If another plugin is currently active, it is deactivated first and its
    /// cleanup data is included in the returned `ActivatedPlugin`.
    ///
    /// Returns `None` if the plugin id is not found.
    pub fn activate(
        &self,
        id: &str,
        current_config_snapshot: OriginalConfigSnapshot,
    ) -> Option<ActivatedPlugin> {
        let mut inner = self.inner.lock().unwrap();

        let plugin = inner.available.get(id).cloned()?;
        let previous = Self::deactivate_inner(&mut inner, current_config_snapshot);

        // Store new active state
        inner.active_id = Some(id.to_string());
        inner.active_tool_names = vec![];
        inner.active_mcp_names = plugin
            .manifest
            .mcp_servers
            .iter()
            .map(|m| m.name.clone())
            .collect();
        inner.active_agent_names = plugin
            .manifest
            .agents
            .iter()
            .map(|a| a.name.clone())
            .collect();
        inner.active_config_snapshot = None; // Caller applies overrides, we don't track snapshot here

        let previous_tool_names = previous
            .as_ref()
            .map(|p| p.tool_names.clone())
            .unwrap_or_default();
        let previous_mcp_names = previous
            .as_ref()
            .map(|p| p.mcp_names.clone())
            .unwrap_or_default();
        let previous_agent_names = previous
            .as_ref()
            .map(|p| p.agent_names.clone())
            .unwrap_or_default();

        Some(ActivatedPlugin {
            id: plugin.id,
            manifest: plugin.manifest.clone(),
            prompt: plugin.manifest.prompt.clone(),
            mcp_servers: plugin.manifest.mcp_servers.clone(),
            agents: plugin.manifest.agents.clone(),
            config_overrides: plugin.config_overrides.clone(),
            previous_tool_names,
            previous_mcp_names,
            previous_agent_names,
        })
    }

    /// Record the tool names that the currently active plugin has registered.
    /// Called by the caller after wiring up tools from the activation data.
    pub fn register_tool_names(&self, names: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.active_id.is_some() {
            inner.active_tool_names = names;
        }
    }

    /// Deactivate the currently active plugin.
    ///
    /// Returns cleanup data so the caller can unregister tools, tear down MCP servers,
    /// remove agents, and revert config overrides.
    ///
    /// Returns `None` if no plugin is currently active.
    pub fn deactivate(
        &self,
        current_config_snapshot: OriginalConfigSnapshot,
    ) -> Option<DeactivatedPlugin> {
        let mut inner = self.inner.lock().unwrap();
        Self::deactivate_inner(&mut inner, current_config_snapshot)
    }

    fn deactivate_inner(
        inner: &mut PluginManagerInner,
        config_snapshot: OriginalConfigSnapshot,
    ) -> Option<DeactivatedPlugin> {
        let active_id = inner.active_id.take()?;
        let plugin = inner.available.get(&active_id)?;

        let tool_names = std::mem::take(&mut inner.active_tool_names);
        let mcp_names = std::mem::take(&mut inner.active_mcp_names);
        let agent_names = std::mem::take(&mut inner.active_agent_names);

        info!(id = active_id, "Plugin deactivated");

        Some(DeactivatedPlugin {
            id: active_id,
            tool_names,
            mcp_names,
            agent_names,
            config_overrides: plugin.config_overrides.clone(),
            config_snapshot,
        })
    }

    /// List all available (loaded) plugin ids.
    pub fn list_available(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        inner.available.keys().cloned().collect()
    }

    /// Get the id of the currently active plugin, if any.
    pub fn get_active(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner.active_id.clone()
    }

    /// Get the manifest for a specific plugin by id.
    pub fn get_manifest(&self, id: &str) -> Option<PluginManifest> {
        let inner = self.inner.lock().unwrap();
        inner.available.get(id).map(|p| p.manifest.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::config_overrides::OriginalConfigSnapshot;
    use std::cell::RefCell;

    thread_local! {
        static TEMP_DIRS: RefCell<Vec<tempfile::TempDir>> = const { RefCell::new(Vec::new()) };
    }

    fn default_snapshot() -> OriginalConfigSnapshot {
        OriginalConfigSnapshot {
            llm_temperature: 0.3,
            llm_max_tokens: 1024,
            llm_system_prompt: "default prompt".to_string(),
            llm_context_tokens: 8192,
            language: "es".to_string(),
        }
    }

    fn create_plugin_dir(id: &str) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(id);
        std::fs::create_dir_all(&path).unwrap();

        let manifest = format!(
            r#"
name = "{id}"
assistant_name = "Assistant"
description = "Test plugin {id}"
version = "1.0.0"

[prompt]
mode = "append"
content = "Test prompt content"
"#
        );
        std::fs::write(path.join("plugin.toml"), manifest).unwrap();

        TEMP_DIRS.with(|v| v.borrow_mut().push(dir));
        path
    }

    // ── PluginManager::new ────────────────────────────────────────────────────

    #[test]
    fn new_with_empty_paths_loads_nothing() {
        let mgr = PluginManager::new(&[]);
        assert!(mgr.list_available().is_empty());
        assert!(mgr.get_active().is_none());
    }

    #[test]
    fn new_with_nonexistent_path_skips() {
        let mgr = PluginManager::new(&[PathBuf::from("/nonexistent/plugin")]);
        assert!(mgr.list_available().is_empty());
    }

    #[test]
    fn new_with_valid_directory_loads_plugin() {
        let path = create_plugin_dir("my-plugin");
        let mgr = PluginManager::new(&[path]);
        let available = mgr.list_available();
        assert!(available.contains(&"my-plugin".to_string()));
    }

    #[test]
    fn new_with_valid_toml_file_loads_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("standalone.toml");
        let manifest = r#"
name = "standalone"
assistant_name = "Assistant"
description = "Standalone plugin"
version = "0.1.0"

[prompt]
mode = "replace"
content = "Replace prompt"
"#;
        std::fs::write(&toml_path, manifest).unwrap();

        let mgr = PluginManager::new(&[toml_path]);
        let available = mgr.list_available();
        assert!(available.contains(&"standalone".to_string()));
    }

    #[test]
    fn new_with_non_toml_file_skips() {
        let dir = tempfile::tempdir().unwrap();
        let txt_path = dir.path().join("readme.txt");
        std::fs::write(&txt_path, "not a plugin").unwrap();

        let mgr = PluginManager::new(&[txt_path]);
        assert!(mgr.list_available().is_empty());
    }

    #[test]
    fn new_skips_invalid_manifest_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("broken.toml");
        std::fs::write(&toml_path, "not valid toml {{{").unwrap();

        let mgr = PluginManager::new(&[toml_path]);
        assert!(mgr.list_available().is_empty());
    }

    // ── activate ──────────────────────────────────────────────────────────────

    #[test]
    fn activate_returns_some_for_valid_plugin() {
        let path = create_plugin_dir("activate-test");
        let mgr = PluginManager::new(&[path]);
        let snapshot = default_snapshot();
        let result = mgr.activate("activate-test", snapshot);
        assert!(result.is_some());
        assert_eq!(mgr.get_active(), Some("activate-test".to_string()));
    }

    #[test]
    fn activate_returns_none_for_unknown_plugin() {
        let mgr = PluginManager::new(&[]);
        let snapshot = default_snapshot();
        let result = mgr.activate("nonexistent", snapshot);
        assert!(result.is_none());
        assert!(mgr.get_active().is_none());
    }

    #[test]
    fn activate_deactivates_previous_plugin() {
        let path1 = create_plugin_dir("first");
        let path2 = create_plugin_dir("second");

        let mgr = PluginManager::new(&[path1, path2]);
        let snapshot = default_snapshot();

        mgr.activate("first", snapshot.clone());
        mgr.register_tool_names(vec!["tool_a".to_string()]);

        let activated = mgr.activate("second", snapshot).unwrap();
        assert_eq!(activated.previous_tool_names, vec!["tool_a".to_string()]);
        assert_eq!(mgr.get_active(), Some("second".to_string()));
    }

    // ── deactivate ────────────────────────────────────────────────────────────

    #[test]
    fn deactivate_returns_cleanup_data() {
        let path = create_plugin_dir("deact-test");
        let mgr = PluginManager::new(&[path]);
        let snapshot = default_snapshot();

        mgr.activate("deact-test", snapshot.clone());
        mgr.register_tool_names(vec!["tool_x".to_string()]);

        let deactivated = mgr.deactivate(snapshot).unwrap();
        assert_eq!(deactivated.id, "deact-test");
        assert_eq!(deactivated.tool_names, vec!["tool_x".to_string()]);
        assert!(deactivated.config_overrides.llm_temperature.is_none());
        assert_eq!(deactivated.config_snapshot.llm_temperature, 0.3);
    }

    #[test]
    fn deactivate_returns_none_when_nothing_active() {
        let mgr = PluginManager::new(&[]);
        let snapshot = default_snapshot();
        let result = mgr.deactivate(snapshot);
        assert!(result.is_none());
    }

    #[test]
    fn deactivate_clears_active_state() {
        let path = create_plugin_dir("clear-test");
        let mgr = PluginManager::new(&[path]);
        let snapshot = default_snapshot();

        mgr.activate("clear-test", snapshot.clone());
        assert!(mgr.get_active().is_some());

        mgr.deactivate(snapshot);
        assert!(mgr.get_active().is_none());
    }

    // ── list_available & get_active & get_manifest ────────────────────────────

    #[test]
    fn list_available_returns_loaded_plugins() {
        let path1 = create_plugin_dir("list-a");
        let path2 = create_plugin_dir("list-b");

        let mgr = PluginManager::new(&[path1, path2]);
        let mut available = mgr.list_available();
        available.sort();
        assert_eq!(available, vec!["list-a".to_string(), "list-b".to_string()]);
    }

    #[test]
    fn get_active_is_none_initially() {
        let mgr = PluginManager::new(&[]);
        assert!(mgr.get_active().is_none());
    }

    #[test]
    fn get_manifest_returns_some_for_existing_plugin() {
        let path = create_plugin_dir("manifest-get");
        let mgr = PluginManager::new(&[path]);
        let manifest = mgr.get_manifest("manifest-get").unwrap();
        assert_eq!(manifest.name, "manifest-get");
    }

    #[test]
    fn get_manifest_returns_none_for_unknown() {
        let mgr = PluginManager::new(&[]);
        assert!(mgr.get_manifest("unknown").is_none());
    }

    // ── register_tool_names ───────────────────────────────────────────────────

    #[test]
    fn register_tool_names_no_op_when_no_active_plugin() {
        let mgr = PluginManager::new(&[]);
        mgr.register_tool_names(vec!["orphan_tool".to_string()]);
        assert!(mgr.get_active().is_none());
    }

    fn create_plugin_dir_with_config(id: &str, config_toml: Option<&str>) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(id);
        std::fs::create_dir_all(&path).unwrap();

        let manifest = format!(
            r#"
name = "{id}"
assistant_name = "Assistant-{id}"
description = "Test plugin {id}"
version = "1.0.0"

[prompt]
mode = "append"
content = "Prompt from {id}"
"#
        );
        std::fs::write(path.join("plugin.toml"), manifest).unwrap();

        if let Some(cfg) = config_toml {
            std::fs::write(path.join("config.toml"), cfg).unwrap();
        }

        TEMP_DIRS.with(|v| v.borrow_mut().push(dir));
        path
    }

    #[test]
    fn integration_full_plugin_lifecycle() {
        let path_a = create_plugin_dir_with_config(
            "lifecycle-a",
            Some(
                r#"llm_temperature = 0.8
llm_max_tokens = 2048"#,
            ),
        );
        let path_b = create_plugin_dir_with_config(
            "lifecycle-b",
            Some(
                r#"llm_temperature = 0.2
language = "en""#,
            ),
        );

        let mgr = PluginManager::new(&[path_a, path_b]);

        let available = mgr.list_available();
        assert!(available.contains(&"lifecycle-a".to_string()));
        assert!(available.contains(&"lifecycle-b".to_string()));
        assert!(mgr.get_active().is_none());

        let snapshot = default_snapshot();
        let activated = mgr
            .activate("lifecycle-a", snapshot)
            .expect("activate lifecycle-a");
        assert_eq!(activated.id, "lifecycle-a");
        assert_eq!(activated.previous_tool_names, Vec::<String>::new());
        assert_eq!(mgr.get_active().as_deref(), Some("lifecycle-a"));

        let manifest = mgr.get_manifest("lifecycle-a").unwrap();
        assert_eq!(manifest.assistant_name, "Assistant-lifecycle-a");

        let snapshot_b = default_snapshot();
        let activated_b = mgr
            .activate("lifecycle-b", snapshot_b)
            .expect("activate lifecycle-b");
        assert_eq!(activated_b.id, "lifecycle-b");
        assert_eq!(activated_b.previous_tool_names, Vec::<String>::new());
        assert_eq!(activated_b.previous_agent_names, Vec::<String>::new());
        assert_eq!(mgr.get_active().as_deref(), Some("lifecycle-b"));

        assert_ne!(mgr.get_active().as_deref(), Some("lifecycle-a"));

        let deactivate_snapshot = default_snapshot();
        let deactivated = mgr
            .deactivate(deactivate_snapshot)
            .expect("deactivate lifecycle-b");
        assert_eq!(deactivated.id, "lifecycle-b");
        assert!(mgr.get_active().is_none());

        let deactivate_snapshot2 = default_snapshot();
        assert!(mgr.deactivate(deactivate_snapshot2).is_none());
    }

    #[test]
    fn integration_switch_plugin_changes_tools_in_registry() {
        use crate::tools::{Tool, ToolRegistry};
        use async_trait::async_trait;

        struct NamedTool {
            name: String,
        }

        #[async_trait]
        impl Tool for NamedTool {
            fn name(&self) -> &str {
                &self.name
            }
            fn description(&self) -> &str {
                "test tool"
            }
            async fn run(&self, _args: &str) -> String {
                String::new()
            }
        }

        let path_a = create_plugin_dir_with_config("tools-a", None);
        let path_b = create_plugin_dir_with_config("tools-b", None);
        let mgr = PluginManager::new(&[path_a, path_b]);

        let mut registry = ToolRegistry::new();
        registry.register(NamedTool {
            name: "baseline".to_string(),
        });

        let baseline_tools = registry.list_registered();
        assert_eq!(baseline_tools, vec!["baseline"]);

        let snapshot = default_snapshot();
        let activated = mgr.activate("tools-a", snapshot).expect("activate tools-a");
        assert_eq!(activated.id, "tools-a");

        let tool_a_name = "plugin_a_tool";
        registry.register(NamedTool {
            name: tool_a_name.to_string(),
        });
        mgr.register_tool_names(vec![tool_a_name.to_string()]);

        let tools_after_a = registry.list_registered();
        assert!(tools_after_a.contains(&"baseline".to_string()));
        assert!(tools_after_a.contains(&tool_a_name.to_string()));

        let snapshot2 = default_snapshot();
        let activated_b = mgr
            .activate("tools-b", snapshot2)
            .expect("activate tools-b");
        assert_eq!(activated_b.id, "tools-b");
        assert_eq!(
            activated_b.previous_tool_names,
            vec![tool_a_name.to_string()]
        );

        for name in &activated_b.previous_tool_names {
            registry.unregister(name);
        }
        let tool_b_name = "plugin_b_tool";
        registry.register(NamedTool {
            name: tool_b_name.to_string(),
        });
        mgr.register_tool_names(vec![tool_b_name.to_string()]);

        let tools_after_b = registry.list_registered();
        assert!(tools_after_b.contains(&"baseline".to_string()));
        assert!(!tools_after_b.contains(&tool_a_name.to_string()));
        assert!(tools_after_b.contains(&tool_b_name.to_string()));

        let deactivate_snap = default_snapshot();
        let deactivated = mgr.deactivate(deactivate_snap).expect("deactivate tools-b");
        assert_eq!(deactivated.tool_names, vec![tool_b_name.to_string()]);

        for name in &deactivated.tool_names {
            registry.unregister(name);
        }

        let final_tools = registry.list_registered();
        assert_eq!(final_tools, vec!["baseline"]);
    }

    #[test]
    fn integration_config_override_apply_revert() {
        use crate::config::Config;
        use crate::plugins::config_overrides::OriginalConfigSnapshot;

        let config_toml = r#"
llm_temperature = 0.95
llm_max_tokens = 4096
llm_system_prompt = "Override system prompt"
llm_context_tokens = 16384
language = "fr"
"#;
        let path = create_plugin_dir_with_config("config-override", Some(config_toml));
        let mgr = PluginManager::new(&[path]);

        let mut config = Config::from_env().expect("Config::from_env");
        let original = OriginalConfigSnapshot::from_config(&config);

        let snapshot = OriginalConfigSnapshot::from_config(&config);
        let activated = mgr
            .activate("config-override", snapshot)
            .expect("activate config-override");

        activated.config_overrides.apply_overrides(&mut config);

        assert_eq!(config.llm_temperature, 0.95);
        assert_eq!(config.llm_max_tokens, 4096);
        assert_eq!(config.llm_system_prompt, "Override system prompt");
        assert_eq!(config.llm_context_tokens, 16384);
        assert_eq!(config.language, "fr");

        let deactivate_snap = OriginalConfigSnapshot::from_config(&config);
        let deactivated = mgr
            .deactivate(deactivate_snap)
            .expect("deactivate config-override");

        deactivated
            .config_overrides
            .revert_overrides(&mut config, &original);

        assert_eq!(config.llm_temperature, original.llm_temperature);
        assert_eq!(config.llm_max_tokens, original.llm_max_tokens);
        assert_eq!(config.llm_system_prompt, original.llm_system_prompt);
        assert_eq!(config.llm_context_tokens, original.llm_context_tokens);
        assert_eq!(config.language, original.language);
    }

    #[test]
    fn integration_prompt_inject_revert() {
        use crate::plugins::prompt_injection::{PluginPromptSections, build_plugin_prompt_section};

        let path_a = create_plugin_dir_with_config("prompt-a", None);
        let path_b = create_plugin_dir_with_config("prompt-b", None);
        let mgr = PluginManager::new(&[path_a, path_b]);

        let snapshot = default_snapshot();
        let activated_a = mgr
            .activate("prompt-a", snapshot)
            .expect("activate prompt-a");

        let sections_a = build_plugin_prompt_section(&[&activated_a.prompt]);

        assert!(!sections_a.append.is_empty());
        assert!(sections_a.append.contains("Prompt from prompt-a"));
        assert!(sections_a.replace.is_empty());
        assert!(sections_a.prepend.is_empty());

        let base_system_prompt = "You are a helpful assistant.";
        let mut current_prompt = base_system_prompt.to_string();
        if !sections_a.replace.is_empty() {
            current_prompt = sections_a.replace.clone();
        }
        if !sections_a.prepend.is_empty() {
            current_prompt = format!("{}{}", sections_a.prepend, current_prompt);
        }
        if !sections_a.append.is_empty() {
            current_prompt.push_str(&sections_a.append);
        }

        assert!(current_prompt.contains("Prompt from prompt-a"));

        let original_sections = PluginPromptSections::default();

        let reverted_prompt = current_prompt
            .replace(&sections_a.replace, &original_sections.replace)
            .replace(&sections_a.prepend, &original_sections.prepend)
            .replace(&sections_a.append, &original_sections.append);

        assert!(reverted_prompt.starts_with(base_system_prompt));
        assert!(!reverted_prompt.contains("Prompt from prompt-a"));

        let snapshot2 = default_snapshot();
        let activated_b = mgr
            .activate("prompt-b", snapshot2)
            .expect("activate prompt-b");

        let sections_b = build_plugin_prompt_section(&[&activated_b.prompt]);
        assert!(sections_b.append.contains("Prompt from prompt-b"));
        assert!(!sections_b.append.contains("Prompt from prompt-a"));
    }

    #[test]
    fn integration_concurrent_plugin_switch_safety() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let path_a = create_plugin_dir_with_config("concurrent-a", None);
        let path_b = create_plugin_dir_with_config("concurrent-b", None);
        let path_c = create_plugin_dir_with_config("concurrent-c", None);
        let mgr = PluginManager::new(&[path_a, path_b, path_c]);

        let activations = Arc::new(AtomicUsize::new(0));
        let panics_detected = Arc::new(AtomicUsize::new(0));
        let thread_count = 8;
        let iterations_per_thread = 100;
        let plugin_ids = ["concurrent-a", "concurrent-b", "concurrent-c"];

        let mut handles = Vec::new();

        for _ in 0..thread_count {
            let mgr_clone = mgr.clone();
            let activations_clone = Arc::clone(&activations);
            let panics_clone = Arc::clone(&panics_detected);

            let handle = thread::spawn(move || {
                let local_panic = std::panic::catch_unwind(|| {
                    for i in 0..iterations_per_thread {
                        let plugin_id = plugin_ids[i % plugin_ids.len()];
                        let snapshot = default_snapshot();

                        let result = mgr_clone.activate(plugin_id, snapshot);
                        assert!(
                            result.is_some(),
                            "activate should always succeed for valid plugin"
                        );
                        activations_clone.fetch_add(1, Ordering::Relaxed);

                        let active = mgr_clone.get_active();
                        assert!(
                            active.as_deref() == Some(plugin_id)
                                || plugin_ids.contains(&active.as_deref().unwrap_or("")),
                            "active should be a known plugin, got: {:?}",
                            active
                        );
                    }
                })
                .is_err();
                if local_panic {
                    panics_clone.fetch_add(1, Ordering::Relaxed);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("thread crashed unexpectedly");
        }

        assert_eq!(
            panics_detected.load(Ordering::Relaxed),
            0,
            "no thread panics expected"
        );

        let total = activations.load(Ordering::Relaxed);
        let expected = thread_count * iterations_per_thread;
        assert_eq!(
            total, expected,
            "expected {} total activations, got {}",
            expected, total
        );

        let active = mgr.get_active();
        assert!(
            active.as_deref().map(|id| plugin_ids.contains(&id)) == Some(true),
            "active plugin should be one of the known plugins, got: {:?}",
            active
        );

        let available = mgr.list_available();
        assert_eq!(available.len(), 3);
    }
}

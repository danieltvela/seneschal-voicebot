# Plugin System Internals — Architecture Reference

The plugin system (`src/plugins/`) enables runtime-swappable feature packs that modify the system prompt, register per-plugin MCP servers and agents, and override config values.

## Architecture Diagram

```
PluginManager
    │
    ├── available: HashMap<String, PluginInfo>   (loaded from disk)
    │     └── plugin.toml + optional config.toml
    │
    ├── activate("id", snapshot)
    │     ├── deactivates current plugin
    │     └── returns ActivatedPlugin { prompt, mcp_servers, agents, config_overrides, previous_* }
    │
    └── deactivate(snapshot)
          └── returns DeactivatedPlugin { tool_names, mcp_names, agent_names, config_overrides, snapshot }
```

The `PluginManager` is a data-only registry — it does **not** hold tool registries, MCP clients, or config. The caller (`main.rs`) orchestrates activation/deactivation.

## Plugin Manager

```rust
pub struct PluginManager {
    inner: Arc<Mutex<PluginManagerInner>>,
}
```

Thread-safe via `Arc<Mutex<>>`. Clone is cheap (ref-counted).

### Key Methods

```rust
pub fn new(plugin_paths: &[PathBuf]) -> Self;
```
Loads plugins from disk. For each path:
- If a `.toml` file → treat as manifest directly.
- If a directory → look for `plugin.toml` inside.
- Also loads optional sibling `config.toml` for config overrides.

```rust
pub fn activate(&self, id: &str, current_config_snapshot: OriginalConfigSnapshot)
    -> Option<ActivatedPlugin>;
```
Returns `ActivatedPlugin` with everything needed to wire up the plugin.

```rust
pub fn deactivate(&self, current_config_snapshot: OriginalConfigSnapshot)
    -> Option<DeactivatedPlugin>;
```
Returns `DeactivatedPlugin` with cleanup info for tools, MCP, agents, and config reversion.

```rust
pub fn register_tool_names(&self, names: Vec<String>);
pub fn list_available(&self) -> Vec<String>;
pub fn get_active(&self) -> Option<String>;
pub fn get_manifest(&self, id: &str) -> Option<PluginManifest>;
```

## Activation Result (ActivatedPlugin)

```rust
pub struct ActivatedPlugin {
    pub id: String,
    pub manifest: PluginManifest,
    pub prompt: PluginPromptConfig,
    pub mcp_servers: Vec<McpServerConfig>,
    pub agents: Vec<PluginAgentConfig>,
    pub config_overrides: PluginConfigOverrides,
    pub previous_tool_names: Vec<String>,      // tools to remove from prior plugin
    pub previous_mcp_names: Vec<String>,        // MCP servers to tear down
    pub previous_agent_names: Vec<String>,      // agents to remove
}
```

## Manifest Format

File: `plugin.toml` (at plugin root).

```toml
name = "my-plugin"
assistant_name = "MyAssistant"
description = "A test plugin"
version = "1.0.0"
requires_permissions = ["network"]     # optional, default []

[prompt]
mode = "append"      # "replace", "append", or "both"
content = "You have access to the weather API."
prepend = false       # for "both" mode: true = prepend, false = append

[[mcp_servers]]
name = "my-mcp"
command = "npx my-mcp-server"
tool_timeout_secs = 60

[[agents]]
name = "my-agent"
mode = "acp"
when_to_use = "For weather-related tasks"
instructions = "You are a weather agent."
```

### PromptMode

```rust
pub enum PromptMode {
    Replace,   // full system prompt replacement
    Append,    // appended to end of system prompt
    Both,      // uses prepend boolean to decide
}
```

### Prompt Injection Protocol

```rust
pub fn build_plugin_prompt_section(configs: &[&PluginPromptConfig]) -> PluginPromptSections;

pub struct PluginPromptSections {
    pub replace: String,   // for Replace mode
    pub prepend: String,   // for Both + prepend=true
    pub append: String,    // for Append mode or Both + prepend=false
}
```

The caller applies sections:
```rust
let mut system = base_prompt.clone();
if !sections.replace.is_empty() { system = sections.replace; }
if !sections.prepend.is_empty() { system = format!("{}{}", sections.prepend, system); }
if !sections.append.is_empty() { system.push_str(&sections.append); }
```

## MCP Spawner

```rust
pub async fn spawn_and_register(
    servers: &[McpServerConfig],
    tool_registry: &mut ToolRegistry,
) -> SpawnedMcpServers;
```

Spawns each MCP server, discovers tool definitions via MCP initialization, registers `McpToolProxy` instances in the `ToolRegistry`. Naming: `{server_name}_mcp__{tool_name}`.

```rust
pub fn cleanup(self, tool_registry: &mut ToolRegistry);
```
Unregisters tools and drops `Arc<McpClient>` values (terminates subprocesses).

## Agent Bridge

```rust
pub fn resolve_plugin_agents(
    plugin_agents: &[PluginAgentConfig],
    existing_names: &HashSet<String>,
) -> (Vec<AgentConfig>, Vec<String>);
```
Converts `PluginAgentConfig` → `AgentConfig`, skipping names already registered from base config.

```rust
pub fn register_plugin_agent_tools(
    agents: &[AgentConfig],
    tool_registry: &mut ToolRegistry,
    shared_history: Arc<RwLock<String>>,
    proactive_tx: mpsc::Sender<ProactiveEvent>,
    session_manager: Option<Arc<AcpSessionManager>>,
    hermes_viewer_mode: HermesSessionViewerMode,
) -> Vec<String>;
```
Creates `RunAgentTool` for each agent with appropriate transport (OpenCode HTTP, ACP session, or visible PTY), registers in tool registry.

```rust
pub fn unregister_plugin_agent_tools(tool_registry: &mut ToolRegistry, tool_names: &[String]) -> Vec<String>;
```

## Config Overrides

```rust
pub struct PluginConfigOverrides {
    pub llm_temperature: Option<f32>,
    pub llm_max_tokens: Option<u32>,
    pub llm_system_prompt: Option<String>,
    pub llm_context_tokens: Option<usize>,
    pub language: Option<String>,
}
```

Loaded from sibling `config.toml` alongside `plugin.toml`.

```rust
pub fn apply_overrides(&self, config: &mut Config);
pub fn revert_overrides(&self, config: &mut Config, baseline: &OriginalConfigSnapshot);
```

### Reversion Protocol

```rust
pub struct OriginalConfigSnapshot { /* clone of config fields */ }
pub fn from_config(config: &Config) -> Self;
```

```
1. snapshot ← OriginalConfigSnapshot::from_config(&config)
2. activate("id", snapshot)
3. activated.config_overrides.apply_overrides(&mut config)
   ... plugin runs ...
4. deactivate(snapshot)
5. deactivated.config_overrides.revert_overrides(&mut config, &snapshot)
```

## Full Activation/Deactivation Protocol

```
── ACTIVATE ──────────────────────────────────────
1. snapshot ← OriginalConfigSnapshot::from_config(&config)
2. activated ← manager.activate("plugin-id", snapshot)
3. activated.config_overrides.apply_overrides(&mut config)
4. spawned ← SpawnedMcpServers::spawn_and_register(&activated.mcp_servers, &mut tool_registry)
5. agent_configs ← resolve_plugin_agents(&activated.agents, &existing_names)
6. tool_names ← register_plugin_agent_tools(&agent_configs, &mut tool_registry, ...)
7. manager.register_tool_names(mcp_names + agent_names)
8. sections ← build_plugin_prompt_section(&[&activated.prompt])
9. Inject prompt sections into LLM system prompt

── DEACTIVATE ────────────────────────────────────
1. current_snapshot ← OriginalConfigSnapshot::from_config(&config)
2. deactivated ← manager.deactivate(current_snapshot)
3. spawned_mcp.cleanup(&mut tool_registry)       // kills subprocesses
4. unregister_plugin_agent_tools(&mut tool_registry, &deactivated.tool_names)
5. deactivated.config_overrides.revert_overrides(&mut config, &deactivated.config_snapshot)
6. Remove plugin prompt sections from system prompt

── SWITCH (activate while active) ────────────────
- previous_tool_names/mcp_names/agent_names tell what to tear down first
```

## PluginSwitchEvent

```rust
pub enum PluginSwitchEvent {
    Activate { plugin_id: String },
    Deactivate,
}
```
Channel message sent across the system to trigger plugin operations. The LLM can invoke the `switch_plugin` tool which emits this event onto the pipeline event channel.

# src/plugins/ — Plugin System

## Responsibility

Provides a **hot-swappable plugin system** that allows runtime activation/deactivation of assistant personas. Each plugin defines its own system prompt, MCP servers, agents, and config overrides. The `PluginManager` tracks which plugin is active and coordinates the lifecycle of all plugin resources (tools, MCP subprocesses, agents, config).

## Design

### `PluginManifest` (`manifest.rs`)

TOML-parsed plugin definition:

```rust
pub struct PluginManifest {
    pub name: String,              // unique plugin id
    pub assistant_name: String,    // display name for the assistant
    pub description: String,
    pub version: String,
    pub prompt: PluginPromptConfig,
    pub mcp_servers: Vec<McpServerConfig>,
    pub agents: Vec<PluginAgentConfig>,
    pub requires_permissions: HashSet<String>,
}
```

**`PluginPromptConfig`**: Defines how the plugin modifies the system prompt.
```rust
pub struct PluginPromptConfig {
    pub mode: PromptMode,  // Replace | Append | Both
    pub content: String,
    pub prepend: bool,     // when mode=Both: prepend vs append
}
```

**`McpServerConfig`**: MCP server to spawn when plugin is active.
```rust
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub tool_timeout_secs: u64,  // default 30
}
```

**`PluginAgentConfig`**: Agent to register when plugin is active. Converts to `AgentConfig` via `From` trait.

### `PluginManager` (`manager.rs`)

Thread-safe manager backed by `Arc<Mutex<PluginManagerInner>>`:

```rust
struct PluginManagerInner {
    available: HashMap<String, PluginInfo>,
    active_id: Option<String>,
    active_tool_names: Vec<String>,
    active_mcp_names: Vec<String>,
    active_agent_names: Vec<String>,
    active_config_snapshot: Option<OriginalConfigSnapshot>,
}
```

Key operations:
- **`new(plugin_paths)`** — Loads plugins from disk. Accepts `.toml` files or directories containing `plugin.toml`. Also loads sibling `config.toml` for config overrides.
- **`activate(id, snapshot)`** — Deactivates current plugin (if any), activates the target. Returns `ActivatedPlugin` with all data needed to wire up resources, plus `previous_*` fields for cleanup.
- **`deactivate(snapshot)`** — Deactivates current plugin. Returns `DeactivatedPlugin` with cleanup data (tool names, MCP names, agent names, config overrides, config snapshot).
- **`register_tool_names()`** — Records tool names registered by the active plugin (called after wiring up).
- **`list_available()` / `get_active()` / `get_manifest()`** — Query methods.

### `PluginConfigOverrides` (`config_overrides.rs`)

Optional per-plugin config overrides loaded from `config.toml`:

```rust
pub struct PluginConfigOverrides {
    pub llm_temperature: Option<f32>,
    pub llm_max_tokens: Option<u32>,
    pub llm_system_prompt: Option<String>,
    pub llm_context_tokens: Option<usize>,
    pub language: Option<String>,
}
```

- **`apply_overrides(&mut Config)`** — Mutates the running config with plugin values.
- **`revert_overrides(&mut Config, &OriginalConfigSnapshot)`** — Restores original values. Only reverts fields that were overridden.

**`OriginalConfigSnapshot`** — Captures config values before plugin activation for clean revert.

### `PluginPromptSections` (`prompt_injection.rs`)

Struct that accumulates prompt modifications from multiple plugins:

```rust
pub struct PluginPromptSections {
    pub replace: String,  // full system prompt replacement
    pub prepend: String,  // prepended to existing prompt
    pub append: String,   // appended to existing prompt
}
```

`build_plugin_prompt_section(&[&PluginPromptConfig])` iterates configs and routes content to the appropriate section based on `PromptMode`.

### `SpawnedMcpServers` (`mcp_spawner.rs`)

Tracks spawned MCP client processes and registered tool names for cleanup:

```rust
pub struct SpawnedMcpServers {
    clients: Vec<Arc<McpClient>>,
    tool_names: Vec<String>,
}
```

- **`spawn_and_register(servers, tool_registry)`** — Spawns each MCP server, creates `McpToolProxy` instances with `{server_name}_mcp__{tool_name}` naming, registers in the tool registry.
- **`cleanup(tool_registry)`** — Unregisters tools, drops client Arcs (terminating subprocesses).
- **`from_parts(clients, proxies, names, tool_registry)`** — Constructs from pre-spawned clients (used by `main.rs` at runtime).

### `Agent Bridge` (`agent_bridge.rs`)

Helper functions for wiring plugin agents into the tool registry:

- **`resolve_plugin_agents(agents, existing_names)`** — Converts `PluginAgentConfig` to `AgentConfig`, skipping duplicates.
- **`register_plugin_agent_tools(agents, registry, ...)`** — Creates `RunAgentTool` instances for each agent, registers them. Returns tool names for cleanup.
- **`unregister_plugin_agent_tools(registry, names)`** — Removes agent tools.
- **`build_plugin_agent_prompt_section()`** / **`merge_agent_prompt_sections()`** — Generates/merges system prompt sections for plugin agents.

### `PluginSwitchEvent` (`mod.rs`)

```rust
pub enum PluginSwitchEvent {
    Activate { plugin_id: String },
    Deactivate,
}
```

Emitted by `SwitchPluginTool` and consumed by the pipeline to trigger full rebuild.

## Flow

### Plugin Loading (Startup)

```
PluginManager::new(&[path1, path2, ...])
    → For each path:
        → If file: parse as manifest (must be .toml)
        → If directory: look for plugin.toml
        → Load sibling config.toml → PluginConfigOverrides
        → PluginInfo { id, manifest, path, config_overrides }
    → available: HashMap<id, PluginInfo>
```

### Plugin Activation

```
SwitchPluginTool.run("plugin_name")
    → event_tx.try_send(PluginSwitchEvent::Activate { plugin_id })
    → Pipeline receives event
    → PluginManager.activate(plugin_id, OriginalConfigSnapshot)
        → deactivate_inner(current) → DeactivatedPlugin (cleanup data)
        → Store new active state → ActivatedPlugin (wire-up data)
    → Caller:
        → Unregister previous tools (activated.previous_tool_names)
        → Cleanup previous MCP servers
        → Unregister previous agents
        → Apply config overrides (activated.config_overrides.apply_overrides)
        → Build prompt sections (build_plugin_prompt_section)
        → Spawn MCP servers (SpawnedMcpServers::spawn_and_register)
        → Register agent tools (register_plugin_agent_tools)
        → Rebuild system prompt
        → Reload Metro / restart pipeline
```

### Plugin Deactivation

```
PluginManager.deactivate(OriginalConfigSnapshot)
    → DeactivatedPlugin { tool_names, mcp_names, agent_names, config_overrides, config_snapshot }
    → Caller:
        → Unregister tools
        → Cleanup MCP servers
        → Unregister agents
        → Revert config overrides (config_overrides.revert_overrides)
        → Restore system prompt
```

## Integration

**Consumers**:
- `src/tools/switch_plugin.rs` — `SwitchPluginTool` triggers activation via `PluginSwitchEvent`.
- `src/pipeline/` — Handles `ProactiveEvent::PluginSwitch` to rebuild tool registry, MCP servers, agents, and config.
- `src/agents/config.rs` — `AgentConfig` used by `PluginAgentConfig` conversion.
- `src/mcp/McpClient` — Used by `SpawnedMcpServers`.
- `src/tools/run_agent.rs` — `RunAgentTool` instances created for plugin agents.

**Dependencies**:
- `src/config/Config` — Config overrides applied/reverted.
- `src/agents::{AgentConfig, AcpSessionManager, ProactiveEvent}`.
- `src/mcp::{McpClient, McpToolDef}`.
- `src/tools::{ToolRegistry, McpToolProxy, RunAgentTool, ActiveTask}`.
- `toml`, `serde`, `anyhow`, `dashmap`, `tracing`.
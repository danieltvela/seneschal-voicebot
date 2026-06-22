use std::sync::Arc;

use tracing::{info, warn};

use crate::mcp::McpClient;
use crate::plugins::manifest::McpServerConfig;
use crate::tools::ToolRegistry;
use crate::tools::mcp_tool::McpToolProxy;

/// Tracks spawned MCP clients and their registered tool names for cleanup.
pub struct SpawnedMcpServers {
    /// Arc-wrapped MCP clients — dropping these terminates the subprocesses.
    clients: Vec<Arc<McpClient>>,
    /// Tool names registered in the ToolRegistry for unregistration.
    tool_names: Vec<String>,
}

impl SpawnedMcpServers {
    /// Spawn MCP servers from a plugin manifest's `mcp_servers` config and
    /// register discovered tools in the ToolRegistry.
    ///
    /// Tool names use the `{server_name}_mcp__{tool_name}` convention.
    /// Spawn failures are logged and skipped — the function continues with
    /// the remaining servers.
    pub async fn spawn_and_register(
        servers: &[McpServerConfig],
        tool_registry: &mut ToolRegistry,
    ) -> Self {
        let (clients, tool_proxies, tool_names) = Self::spawn_clients(servers).await;

        for (name, proxy) in tool_proxies {
            info!(
                target: "mcp",
                tool_name = %name,
                "Registering plugin MCP tool"
            );
            tool_registry.register(proxy);
        }

        info!(
            target: "mcp",
            tool_count = tool_names.len(),
            client_count = clients.len(),
            "Registered plugin MCP servers"
        );

        Self {
            clients,
            tool_names,
        }
    }

    /// Spawn MCP client processes and prepare tool proxies without registering.
    /// Returns clients, tool proxies with names, and tool names for cleanup tracking.
    pub async fn spawn_clients(
        servers: &[McpServerConfig],
    ) -> (
        Vec<Arc<McpClient>>,
        Vec<(String, McpToolProxy)>,
        Vec<String>,
    ) {
        let mut clients: Vec<Arc<McpClient>> = Vec::new();
        let mut tool_proxies: Vec<(String, McpToolProxy)> = Vec::new();
        let mut tool_names: Vec<String> = Vec::new();

        for server in servers {
            info!(
                target: "mcp",
                server_name = %server.name,
                command = %server.command,
                "Spawning plugin MCP server"
            );

            let result = McpClient::spawn_and_init(&server.command, server.tool_timeout_secs).await;

            match result {
                Ok((client, tool_defs)) => {
                    let client = Arc::new(client);

                    for def in tool_defs {
                        let prefixed_name = format!("{}_mcp__{}", server.name, def.name);
                        let prefixed_desc =
                            format!("[MCP server: {}] {}", server.name, def.description);

                        let proxy = McpToolProxy::new(
                            prefixed_name.clone(),
                            def.name,
                            prefixed_desc,
                            def.input_schema,
                            Arc::clone(&client),
                        );
                        tool_proxies.push((prefixed_name.clone(), proxy));
                        tool_names.push(prefixed_name);
                    }

                    clients.push(client);
                }
                Err(e) => {
                    warn!(
                        target: "mcp",
                        server_name = %server.name,
                        error = %e,
                        "Plugin MCP server failed to start — skipping"
                    );
                }
            }
        }

        (clients, tool_proxies, tool_names)
    }

    /// Construct from already-spawned clients and proxies, registering the proxies
    /// in the provided tool registry. Used by main.rs when activating a plugin at runtime.
    pub fn from_parts(
        clients: Vec<Arc<McpClient>>,
        proxies: Vec<(String, McpToolProxy)>,
        names: Vec<String>,
        tool_registry: &mut ToolRegistry,
    ) -> Self {
        for (name, proxy) in proxies {
            info!(
                target: "mcp",
                tool_name = %name,
                "Registering plugin MCP tool"
            );
            tool_registry.register(proxy);
        }

        Self {
            clients,
            tool_names: names,
        }
    }

    /// Shutdown MCP clients and unregister tools from the registry.
    ///
    /// Dropping the `Arc<McpClient>` instances terminates the child processes.
    /// Tool unregistration clears the registry's cached tool definitions.
    pub fn cleanup(self, tool_registry: &mut ToolRegistry) {
        for name in &self.tool_names {
            tool_registry.unregister(name);
        }

        info!(
            target: "mcp",
            tool_count = self.tool_names.len(),
            client_count = self.clients.len(),
            "Cleaned up plugin MCP servers"
        );
    }

    /// Return the tool names that were registered.
    pub fn tool_names(&self) -> &[String] {
        &self.tool_names
    }
}

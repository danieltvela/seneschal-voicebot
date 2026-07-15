//! MCP server configuration loaded from environment variables.
//!
//! Supports both the new multi-MCP format (`MCPS`) and the legacy
//! single-MCP format (`MCP_COMMAND`).

use std::env;

// ── MCP Transport Kind ────────────────────────────────────────────────────────

/// Describes the transport mechanism for an MCP server.
#[derive(Debug, Clone)]
pub enum McpTransportKind {
    /// Stdio transport: spawn a subprocess and communicate over stdin/stdout.
    Stdio {
        /// Command to spawn the subprocess (e.g. `"bunx apple-mcp@latest"`).
        command: String,
    },
    /// HTTP transport: communicate via HTTP SSE (not yet implemented).
    Http {
        /// Base URL of the MCP HTTP server.
        url: String,
    },
}

// ── MCP Server Configuration ─────────────────────────────────────────────────

/// Single MCP server configuration loaded from environment variables.
///
/// Each server gets its own subprocess and its own set of tools.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Unique name used for tool prefixing: `{name}_mcp__{tool_name}`.
    pub name: String,
    /// Command to spawn the MCP server subprocess.
    ///
    /// Kept for backward compatibility — new code should use
    /// [`McpConfig::transport`] to determine the transport type.
    pub command: String,
    /// Transport configuration.
    pub transport: McpTransportKind,
    /// Hard timeout in seconds for each tool call (default 30).
    pub tool_timeout_secs: u64,
}

// ── TOML Config Support ───────────────────────────────────────────────────────

/// Single MCP server definition loaded from the `[[mcp_servers]]` TOML array.
///
/// Both `command` (stdio) and `url` (HTTP) are optional so the config file
/// can declare a server by either transport. When both are set, URL is preferred
/// (matching the env-var behaviour).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct McpServerTomlConfig {
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

fn default_tool_timeout_secs() -> u64 {
    30
}

impl From<McpServerTomlConfig> for McpConfig {
    fn from(toml: McpServerTomlConfig) -> Self {
        let (transport, command_display) = if let Some(url) = toml.url {
            (McpTransportKind::Http { url: url.clone() }, url)
        } else if let Some(cmd) = toml.command {
            (
                McpTransportKind::Stdio {
                    command: cmd.clone(),
                },
                cmd,
            )
        } else {
            // No transport configured — provide a no-op default so the caller
            // can still inspect the name; from_config_and_env filters these out.
            (
                McpTransportKind::Stdio {
                    command: String::new(),
                },
                String::new(),
            )
        };

        McpConfig {
            name: toml.name,
            command: command_display,
            transport,
            tool_timeout_secs: toml.tool_timeout_secs,
        }
    }
}

// ── MCP Server Registry ──────────────────────────────────────────────────────

/// Registry of all configured MCP servers.
///
/// Created once at startup from environment variables and/or TOML config.
/// Supports the new multi-MCP format (`MCPS=apple,filesystem`), the legacy
/// single-MCP format (`MCP_COMMAND`), and the `[[mcp_servers]]` TOML array.
#[derive(Debug, Clone)]
pub struct McpRegistry {
    pub servers: Vec<McpConfig>,
}

impl McpRegistry {
    /// Load MCP servers from environment variables only (backward-compatible).
    pub fn from_env() -> Self {
        Self::from_config_and_env(Vec::new())
    }

    /// Load MCP servers from environment variables and/or TOML config.
    ///
    /// Priority (first match wins):
    /// 1. `MCPS` env var → comma-separated names, each resolved via `MCP_<NAME>_*`.
    /// 2. `MCP_COMMAND` env var → single `"default"` server (legacy).
    /// 3. `toml_servers` → from the `[[mcp_servers]]` TOML array in config.
    /// 4. Empty registry (no MCP tools).
    pub fn from_config_and_env(toml_servers: Vec<McpServerTomlConfig>) -> Self {
        // ── Priority 1: Multi-MCP env ──────────────────────────────
        if let Ok(raw) = env::var("MCPS") {
            let names: Vec<&str> = raw
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            if !names.is_empty() {
                let servers = names.into_iter().filter_map(load_mcp_from_env).collect();
                return Self { servers };
            }
        }

        // ── Priority 2: Legacy MCP_COMMAND env ─────────────────────
        if let Ok(command) = env::var("MCP_COMMAND") {
            let timeout: u64 = env::var("MCP_TOOL_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30);

            return Self {
                servers: vec![McpConfig {
                    name: "default".to_string(),
                    command: command.clone(),
                    transport: McpTransportKind::Stdio { command },
                    tool_timeout_secs: timeout,
                }],
            };
        }

        // ── Priority 3: TOML servers ───────────────────────────────
        if !toml_servers.is_empty() {
            let servers: Vec<McpConfig> = toml_servers
                .into_iter()
                .filter(|s| s.command.is_some() || s.url.is_some())
                .map(McpConfig::from)
                .collect();
            return Self { servers };
        }

        // ── Priority 4: Empty ──────────────────────────────────────
        Self {
            servers: Vec::new(),
        }
    }
}

/// Load a single MCP server config from env vars using the `MCP_<NAME>_*` convention.
///
/// Priority:
/// 1. `MCP_<NAME>_URL` → HTTP transport
/// 2. `MCP_<NAME>_COMMAND` → Stdio transport
/// 3. Neither → return `None` (server is skipped)
///
/// If both URL and COMMAND are set, URL is preferred and a warning is logged.
fn load_mcp_from_env(name: &str) -> Option<McpConfig> {
    let upper = name.to_uppercase().replace('-', "_");

    let url = env::var(format!("MCP_{}_URL", upper)).ok();
    let command = env::var(format!("MCP_{}_COMMAND", upper)).ok();

    if url.is_some() && command.is_some() {
        tracing::warn!(
            target: "mcp",
            "Both MCP_{upper}_URL and MCP_{upper}_COMMAND are set — preferring URL",
            upper = upper,
        );
    }

    let tool_timeout_secs: u64 = env::var(format!("MCP_{}_TIMEOUT_SECS", upper))
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    if let Some(url_val) = url {
        return Some(McpConfig {
            name: name.to_string(),
            command: url_val.clone(),
            transport: McpTransportKind::Http { url: url_val },
            tool_timeout_secs,
        });
    }

    let command_val = command?;

    Some(McpConfig {
        name: name.to_string(),
        command: command_val.clone(),
        transport: McpTransportKind::Stdio {
            command: command_val,
        },
        tool_timeout_secs,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_url_produces_http_transport() {
        temp_env::with_var("MCP_COMMAND", None::<&str>, || {
            temp_env::with_var("MCPS", Some("test"), || {
                temp_env::with_var("MCP_TEST_URL", Some("http://localhost:9999"), || {
                    temp_env::with_var("MCP_TEST_TIMEOUT_SECS", Some("45"), || {
                        let registry = McpRegistry::from_env();
                        assert_eq!(registry.servers.len(), 1);
                        let config = &registry.servers[0];
                        assert_eq!(config.name, "test");
                        assert!(
                            matches!(config.transport, McpTransportKind::Http { .. }),
                            "expected Http transport, got {:?}",
                            config.transport,
                        );
                        assert_eq!(config.tool_timeout_secs, 45);
                        if let McpTransportKind::Http { ref url } = config.transport {
                            assert_eq!(url, "http://localhost:9999");
                        }
                    });
                });
            });
        });
    }

    #[test]
    fn mcp_mixed_stdio_and_http() {
        temp_env::with_var("MCP_COMMAND", None::<&str>, || {
            temp_env::with_var("MCPS", Some("local,remote"), || {
                temp_env::with_var("MCP_LOCAL_COMMAND", Some("bunx local-server"), || {
                    temp_env::with_var("MCP_REMOTE_URL", Some("http://remote:8080/mcp"), || {
                        temp_env::with_var("MCP_REMOTE_TIMEOUT_SECS", Some("15"), || {
                            let registry = McpRegistry::from_env();
                            assert_eq!(registry.servers.len(), 2);

                            assert_eq!(registry.servers[0].name, "local");
                            assert!(
                                matches!(
                                    registry.servers[0].transport,
                                    McpTransportKind::Stdio { .. }
                                ),
                                "expected Stdio for 'local', got {:?}",
                                registry.servers[0].transport,
                            );

                            assert_eq!(registry.servers[1].name, "remote");
                            assert!(
                                matches!(
                                    registry.servers[1].transport,
                                    McpTransportKind::Http { .. }
                                ),
                                "expected Http for 'remote', got {:?}",
                                registry.servers[1].transport,
                            );
                            assert_eq!(registry.servers[1].tool_timeout_secs, 15);
                            if let McpTransportKind::Http { ref url } =
                                registry.servers[1].transport
                            {
                                assert_eq!(url, "http://remote:8080/mcp");
                            }
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn mcp_url_precedence_over_command() {
        temp_env::with_var("MCP_COMMAND", None::<&str>, || {
            temp_env::with_var("MCPS", Some("both"), || {
                temp_env::with_var("MCP_BOTH_URL", Some("http://localhost:8080"), || {
                    temp_env::with_var("MCP_BOTH_COMMAND", Some("bunx something"), || {
                        temp_env::with_var("MCP_BOTH_TIMEOUT_SECS", Some("60"), || {
                            let registry = McpRegistry::from_env();
                            assert_eq!(registry.servers.len(), 1);
                            let config = &registry.servers[0];
                            // URL takes precedence over command.
                            assert!(
                                matches!(config.transport, McpTransportKind::Http { .. }),
                                "expected Http transport (URL precedence), got {:?}",
                                config.transport,
                            );
                            assert_eq!(config.tool_timeout_secs, 60);
                            if let McpTransportKind::Http { ref url } = config.transport {
                                assert_eq!(url, "http://localhost:8080");
                            }
                        });
                    });
                });
            });
        });
    }

    // ── from_config_and_env ────────────────────────────────────────────

    #[test]
    fn toml_servers_loaded_when_no_env() {
        let toml_servers = vec![McpServerTomlConfig {
            name: "apple".to_string(),
            command: Some("bunx apple-mcp@latest".to_string()),
            url: None,
            tool_timeout_secs: 30,
        }];

        temp_env::with_var("MCPS", None::<&str>, || {
            temp_env::with_var("MCP_COMMAND", None::<&str>, || {
                let registry = McpRegistry::from_config_and_env(toml_servers);
                assert_eq!(registry.servers.len(), 1);
                assert_eq!(registry.servers[0].name, "apple");
                assert!(matches!(
                    registry.servers[0].transport,
                    McpTransportKind::Stdio { .. }
                ));
            });
        });
    }

    #[test]
    fn env_wins_over_toml() {
        let toml_servers = vec![McpServerTomlConfig {
            name: "from_toml".to_string(),
            command: Some("echo toml".to_string()),
            url: None,
            tool_timeout_secs: 30,
        }];

        temp_env::with_var("MCP_COMMAND", None::<&str>, || {
            temp_env::with_var("MCPS", Some("from_env"), || {
                temp_env::with_var("MCP_FROM_ENV_COMMAND", Some("bunx env-server"), || {
                    let registry = McpRegistry::from_config_and_env(toml_servers);
                    assert_eq!(registry.servers.len(), 1);
                    assert_eq!(registry.servers[0].name, "from_env");
                });
            });
        });
    }

    #[test]
    fn legacy_env_wins_over_toml() {
        let toml_servers = vec![McpServerTomlConfig {
            name: "from_toml".to_string(),
            command: Some("echo toml".to_string()),
            url: None,
            tool_timeout_secs: 30,
        }];

        temp_env::with_var("MCPS", None::<&str>, || {
            temp_env::with_var("MCP_COMMAND", Some("legacy-command"), || {
                let registry = McpRegistry::from_config_and_env(toml_servers);
                assert_eq!(registry.servers.len(), 1);
                assert_eq!(registry.servers[0].name, "default");
                assert_eq!(registry.servers[0].command, "legacy-command");
            });
        });
    }

    #[test]
    fn toml_url_produces_http() {
        let toml_servers = vec![McpServerTomlConfig {
            name: "remote".to_string(),
            command: None,
            url: Some("http://remote:8080/mcp".to_string()),
            tool_timeout_secs: 45,
        }];

        temp_env::with_var("MCPS", None::<&str>, || {
            temp_env::with_var("MCP_COMMAND", None::<&str>, || {
                let registry = McpRegistry::from_config_and_env(toml_servers);
                assert_eq!(registry.servers.len(), 1);
                assert!(matches!(
                    registry.servers[0].transport,
                    McpTransportKind::Http { .. }
                ));
                assert_eq!(registry.servers[0].tool_timeout_secs, 45);
                if let McpTransportKind::Http { ref url } = registry.servers[0].transport {
                    assert_eq!(url, "http://remote:8080/mcp");
                }
            });
        });
    }

    #[test]
    fn toml_command_produces_stdio() {
        let toml_servers = vec![McpServerTomlConfig {
            name: "local".to_string(),
            command: Some("bunx local-mcp".to_string()),
            url: None,
            tool_timeout_secs: 60,
        }];

        temp_env::with_var("MCPS", None::<&str>, || {
            temp_env::with_var("MCP_COMMAND", None::<&str>, || {
                let registry = McpRegistry::from_config_and_env(toml_servers);
                assert_eq!(registry.servers.len(), 1);
                assert!(matches!(
                    registry.servers[0].transport,
                    McpTransportKind::Stdio { .. }
                ));
                assert_eq!(registry.servers[0].tool_timeout_secs, 60);
                if let McpTransportKind::Stdio { ref command } = registry.servers[0].transport {
                    assert_eq!(command, "bunx local-mcp");
                }
            });
        });
    }
}

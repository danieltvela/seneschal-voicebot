# Delegation & Integration — Task List

Modules: `src/agents/`, `src/mcp/`, `src/plugins/`, `src/search/`

---

## [M0.3] Agent Delegation (`src/agents/`)

### ACP Protocol (`src/agents/mod.rs`)
- [x] JSON-RPC 2.0 over stdio (Hermes, OpenCode)
- [x] ACP warmup at startup (`AGENT_ACP_WARMUP`)
- [x] `AcpSessionManager` for session lifecycle
- [ ] **Session reconnect**: if ACP agent process dies, attempt automatic reconnect up to 3 times
- [ ] **Heartbeats**: send periodic ping to detect dead agent sessions before user waits
- [ ] **Session multiplexing**: support multiple concurrent ACP agents (each with its own stdio process)
- [ ] **Graceful shutdown**: send `session/delete` on Seneschal shutdown to clean up agent sessions

### Agent Config (`src/agents/config.rs`)
- [x] `AgentConfig` struct with mode (cli/acp/remote), command, timeout
- [ ] **Agent health check**: on startup, spawn each agent and run a quick health check; log if agent is not responsive
- [ ] **Agent concurrency limit**: cap at 3 concurrent agent tasks; queue new ones

### Visible Agent Mode (`src/agent_session.rs`)
- [x] PTY-based visible agent with Terminal.app viewer
- [ ] **Session log rotation**: cap log files at 10 MB; archive old logs
- [ ] **Multi-agent viewer**: support viewing multiple agent PTY sessions in separate Terminal tabs

---

## [M0.3] MCP Integration (`src/mcp/`)

### MCP Client (`src/mcp/mod.rs`)
- [x] Stdio transport (subprocess)
- [x] HTTP transport (Streamable HTTP)
- [x] Multi-server support (`MCPS=apple,filesystem`)
- [x] Tool discovery via `tools/list`
- [x] Per-tool timeout (`MCP_<NAME>_TIMEOUT_SECS`)
- [ ] **Server health monitoring**: periodic `ping` to each MCP server; reconnect on failure
- [ ] **Tool result caching**: cache MCP tool results with TTL (e.g., filesystem listings valid for 5 seconds)
- [ ] **MCP server auto-restart**: if an MCP server process exits unexpectedly, restart it automatically
- [ ] **Resource support**: implement MCP `resources/list` and `resources/read` for servers that expose resources

---

## [M0.3] Plugin System (`src/plugins/`)

### Plugin Manager (`src/plugins/manager.rs`)
- [x] Manifest loading (`manifest.toml`)
- [x] MCP server spawning per plugin
- [x] Agent bridging per plugin
- [x] Config overrides with clean revert
- [x] Prompt injection (replace/append/both)
- [x] `switch_plugin` tool for runtime switching
- [ ] **Plugin validation**: validate `manifest.toml` against a schema before loading; reject invalid plugins
- [ ] **Plugin hot-reload**: detect changes to plugin directory and reload without restart
- [ ] **Plugin dependencies**: allow plugins to declare dependencies on other plugins
- [ ] **Plugin marketplace**: CLI command `seneschal plugin install <name>` to fetch from registry

### Plugin Prompt Injection (`src/plugins/prompt_injection.rs`)
- [ ] **Merge strategy**: when switching plugins, smooth the prompt transition (don't truncate mid-conversation)
- [ ] **Token budget**: track how many tokens each plugin's prompt adds; warn if near context limit

---

## [M0.3] Search Providers (`src/search/`)

- [x] `SearchProvider` trait with SearXNG, Brave, Tavily, Exa backends
- [ ] **Provider ranking**: test all providers for latency and result quality; rank and auto-select best
- [ ] **Rate limiting**: enforce per-provider rate limits to avoid API throttling
- [ ] **Result deduplication**: if multiple providers return the same URL, merge results
- [ ] **Search result caching**: cache recent searches (TTL: 60 seconds) to avoid repeated API calls

---

*Last updated: 2026-07-27.*

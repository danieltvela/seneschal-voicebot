# src/mcp/ — Model Context Protocol Integration

## Responsibility

Provides a JSON-RPC 2.0 client over stdio transport for communicating with MCP (Model Context Protocol) server subprocesses. Handles the full MCP lifecycle: spawn subprocess → initialize handshake → discover tools via `tools/list` → call tools via `tools/call` → disconnect. Each MCP server runs as an independent subprocess with its own tool set.

## Design

### `McpClient`

Persistent client that owns a single MCP server subprocess. Core fields:

```rust
pub struct McpClient {
    writer: Mutex<McpWriter>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<RpcResponse>>>>,
    tool_timeout_secs: u64,
}
```

- **`McpWriter`**: Internal struct holding `ChildStdin`, `Child`, and auto-incrementing `next_id: u64`. Provides `send_request()` (with id), `send_notification()` (no id), and `send_raw()` (newline-delimited JSON).
- **`pending`**: Map of in-flight request IDs to oneshot response channels. The background reader task routes responses to the correct waiter by matching the JSON-RPC `id` field.
- **Concurrent safety**: Multiple `call_tool()` calls can be in-flight simultaneously. Each registers its own oneshot channel keyed on the request ID.

### `McpConfig` / `McpRegistry`

```rust
pub struct McpConfig {
    pub name: String,           // unique name, used for tool prefixing
    pub command: String,        // subprocess command
    pub tool_timeout_secs: u64, // default 30
}

pub struct McpRegistry {
    pub servers: Vec<McpConfig>,
}
```

Loading priority:
1. `MCPS=apple,filesystem` → load each via `MCP_<NAME>_COMMAND` / `MCP_<NAME>_TIMEOUT_SECS`.
2. Legacy: `MCP_COMMAND` → single `"default"` server.
3. Empty registry if neither set.

### `McpToolDef`

Parsed from `tools/list` response:

```rust
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,  // JSON Schema (inputSchema field)
}
```

### Protocol Methods

- **`initialize()`**: Sends `initialize` request with `protocolVersion: "2024-11-05"`, waits for response, then sends `notifications/initialized`.
- **`list_tools()`**: Calls `tools/list`, parses the `tools` array into `Vec<McpToolDef>`.
- **`call_tool(name, arguments)`**: Calls `tools/call` with a hard timeout. Returns extracted text content.
- **`disconnect()`**: Sends `exit` notification, drops pending, waits for child to exit.

### `extract_text_content()`

Extracts text from MCP result format: `{"content": [{"type": "text", "text": "..."}], "isError": false}`. Filters for `type: "text"` items, joins with newlines. Falls back to JSON serialization of the whole result if no text parts.

## Flow

```
McpRegistry::from_env() → Vec<McpConfig>
    → for each config:
        McpClient::spawn_and_init(command, timeout)
            → Command::new(program).args(args).stdin(piped).stdout(piped).stderr(log)
            → spawn reader task (BufReader::lines → parse JSON → parse_response → route to pending)
            → initialize() → notifications/initialized
            → list_tools() → Vec<McpToolDef>
            → return (McpClient, tool_defs)
        → for each tool_def:
            McpToolProxy::new(prefixed_name, original_name, prefixed_desc, input_schema, Arc<McpClient>)
            → tool_registry.register(proxy)
```

**Tool call** (runtime):
```
LLM calls {server}_mcp__{tool_name}
    → McpToolProxy.run(args)
    → McpClient.call_tool(original_name, arguments)
    → writer.send_request("tools/call", {name, arguments})
    → wait_for_response(id) with timeout
    → reader task receives response → sends on oneshot channel
    → extract_text_content(result) → String
```

## Integration

**Consumers**:
- `src/tools/mcp_tool.rs` — `McpToolProxy` wraps each `McpToolDef` as a `dyn Tool`. Tool names use `{server_name}_mcp__{tool_name}` convention.
- `src/plugins/mcp_spawner.rs` — `SpawnedMcpServers::spawn_and_register()` spawns plugin MCP servers and registers tools.

**Dependencies**:
- `src/config/Config` — `log_file_path()` for stderr redirection.
- `tokio::process::Command`, `tokio::io::AsyncBufReadExt`, `tokio::sync::oneshot`, `serde_json`, `anyhow`, `tracing`.

**External**: Compatible with any stdio-transport MCP server (e.g., `bunx apple-mcp@latest`, `npx @mcp/server-filesystem`).
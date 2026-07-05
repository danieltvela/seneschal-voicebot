# src/tools/ — Tool Implementations

## Responsibility

Provides the `Tool` trait and concrete implementations that the LLM can invoke to perform actions on the user's system. Each tool is a self-contained async handler that receives JSON arguments from the LLM and returns a `String` result. The module also defines `ToolRegistry`, the central registration and dispatch mechanism.

## Design

### `Tool` Trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON Schema (OpenAI function-calling format)
    fn is_background(&self) -> bool;            // default: false
    fn is_silent(&self) -> bool;                // default: false (NoopTool only)
    fn should_force_for(&self, query: &str) -> bool;  // default: false
    async fn run(&self, args: &str) -> String;
}
```

Key design decisions:
- **Background vs synchronous**: `is_background()` determines whether the pipeline runs the tool in a `tokio::spawn` task (delivering results via `ProactiveEvent`) or blocks the LLM turn. `WebSearchTool`, `RunShellTool`, `McpToolProxy`, and `DeepResearchTool` are background.
- **Silent tools**: `is_silent()` suppresses LLM response after execution. Only `NoopTool` returns `true`.
- **Forced tool calls**: `should_force_for()` allows the pipeline to set `tool_choice` to guarantee the tool is called for explicit user requests (e.g., `CurrentTimeTool` for "¿Qué hora es?", `WebSearchTool` for "Busca...").
- **Tool parsing**: `ToolRegistry.parse_tool_call()` extracts tool calls from LLM output using `<tool_call>tool_name: args</tool_call>` delimiter syntax.

### `ToolRegistry`

Thread-safe registry backed by `HashMap<String, Arc<dyn Tool>>`. Provides:
- `register()` / `unregister()` — add/remove tools, invalidating cached tool definitions.
- `tool_definitions()` — returns OpenAI `tools` array with lazy caching (`Mutex<Option<Vec<serde_json::Value>>>`).
- `system_prompt_section()` — generates system prompt instructions for tool usage (Spanish-language rules).
- `forced_tool_for_query()` — finds a tool whose `should_force_for()` matches the query.
- `execute()` — dispatches a tool call by name.

### Tool Implementations

| File | Struct | Name | Background | Dependencies |
|------|--------|------|-----------|--------------|
| `current_time.rs` | `CurrentTimeTool` | `current_time` | No | `chrono::Local` |
| `clipboard.rs` | `ReadClipboardTool` | `read_clipboard` | No | `pbpaste` (macOS) |
| `clipboard.rs` | `SetClipboardTool` | `set_clipboard` | No | `pbcopy` (macOS) |
| `open_app.rs` | `OpenAppTool` | `open_app` | No | `open -a` (macOS) |
| `web_search.rs` | `WebSearchTool` | `web_search` | Yes | `reqwest`, optional `LlmProvider` (synthesis) |
| `take_screenshot.rs` | `TakeScreenshotTool` | `take_screenshot` | No | `screen_capture`, `LlmProvider` (vision) |
| `noop.rs` | `NoopTool` | `noop` | No | — |
| `mcp_tool.rs` | `McpToolProxy` | `{name}_mcp__{tool}` | Yes | `McpClient` |
| `run_shell.rs` | `RunShellTool` | `run_shell` | Yes | `tokio::process` |
| `read_file.rs` | `ReadFileTool` | `read_file` | No | `tokio::fs` |
| `deep_research.rs` | `DeepResearchTool` | `deep_research` | Yes | `AgentConfig`, `run_agent::call_agent` |
| `apple_events.rs` | `AppleEventsTool` | `apple_events` | No | `osascript` (Calendar/Reminders) |
| `recover_historical_context.rs` | `RecoverHistoricalContextTool` | `recover_historical_context` | No | `Database` (FTS5) |
| `quick_search.rs` | `QuickSearchTool` | `quick_search` | No | `SearchProvider` |
| `conversation_mode.rs` | `SetConversationModeTool` | `set_conversation_mode` | No | `Arc<Mutex<ConversationMode>>` |
| `run_agent.rs` | `RunAgentTool` | `run_{name}` | No (spawns background) | `AcpWriter`, `AcpSessionManager`, `DashMap` |
| `switch_plugin.rs` | `SwitchPluginTool` | `switch_plugin` | No | `PluginManager`, `PluginSwitchEvent` |

### `run_agent.rs` — Agent Delegation Subsystem

The largest file in the module. Contains:

- **`RunAgentTool`**: Unified agent delegation tool with two modes:
  - `"cli"`: one-shot subprocess via `call_agent()` → `strip_hermes_cli_noise()` → `synthesize_agent_result()` → `ProactiveEvent::AgentResult`
  - `"acp"`: persistent JSON-RPC 2.0 stdio via `AcpWriter` → `collect_acp_response()` → `ProactiveEvent::AgentResult`
  - Inline commands: `cancel` (sends oneshot signal), `status` (reports active tasks)

- **`AcpWriter`**: Write-side of a persistent ACP subprocess. Manages `ChildStdin`, JSON-RPC request IDs, and optional log file. Methods: `spawn()`, `initialize()` (initialize + session/new handshake), `send_prompt()`, `send_cancel()`, `warm_up()`, `kill()`.

- **`ActiveTask`**: Tracks in-flight ACP tasks with `task_id`, `session_id`, `prompt_request_id`, `TaskState`, `cancel_handle`.

- **`collect_acp_response()`**: Drives the inbound message loop. Handles `session/update` notifications (agent_message_chunk, agent_thought_chunk, tool_call), `session/request_permission` requests (auto-allow or user prompt via `ProactiveEvent::AgentQuestion`), and cancellation.

- **`JsonRpcMessage`**: Parsed JSON-RPC 2.0 messages: `Response`, `Request`, `Notification`.

## Flow

```
User utterance → STT → LLM → LLM emits "<tool_call>tool_name: {args}</tool_call>"
    → ToolRegistry.parse_tool_call() → (name, args)
    → ToolRegistry.forced_tool_for_query() (if should_force_for matches)
    → Pipeline checks is_background():
        - false: ToolRegistry.execute() → String result → LLM continues with result
        - true: tokio::spawn(Tool::run()) → returns immediately
              → result delivered via ProactiveEvent::AgentResult → pipeline injects
```

**Tool construction** (startup):
```
Config → ToolRegistry::new()
    → register(CurrentTimeTool)
    → register(ReadClipboardTool), register(SetClipboardTool)
    → register(OpenAppTool)
    → if SEARXNG_URL: register(WebSearchTool)
    → if SECONDARY_LLM_URL: register(TakeScreenshotTool)
    → if SHELL_ENABLED: register(RunShellTool)
    → if NOOP_TOOL_INSTRUCTIONS: register(NoopTool)
    → if SearchProvider configured: register(QuickSearchTool)
    → if Database available: register(RecoverHistoricalContextTool)
    → for each McpConfig: McpClient::spawn_and_init() → for each tool: register(McpToolProxy)
    → for each AgentConfig: register(RunAgentTool)
    → register(SetConversationModeTool)
    → register(SwitchPluginTool)
```

## Integration

**Consumers**:
- `src/pipeline/` — calls `ToolRegistry.parse_tool_call()`, `forced_tool_for_query()`, `execute()`, `is_background()`, `is_silent()`.
- `src/daemon.rs` — receives `ProactiveEvent::AgentResult` from background tools.

**Dependencies**:
- `src/mcp/McpClient` — used by `McpToolProxy` for `call_tool()`.
- `src/agents/` — `AgentConfig`, `AcpSessionManager`, `ProactiveEvent` used by `RunAgentTool`.
- `src/search/SearchProvider` — used by `QuickSearchTool`.
- `src/db/Database` — used by `RecoverHistoricalContextTool`.
- `src/llm/LlmProvider` — used by `TakeScreenshotTool` (vision) and `WebSearchTool` (synthesis).
- `src/plugins/` — `SwitchPluginTool` triggers `ProactiveEvent::PluginSwitch`.
- `crate::screen_capture` — used by `TakeScreenshotTool`.
- External: `chrono`, `reqwest`, `tokio::process`, `serde_json`, `async_trait`, `tracing`, `dashmap`, `uuid`.
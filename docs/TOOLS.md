# Tools — Seneschal Tool System

> Every capability the LLM can invoke is modelled as a `Tool`. This document lists every tool, explains how they are registered, and describes how the pipeline dispatches them.

---

## Table of Contents

1. [Tool Trait](#tool-trait)
2. [ToolRegistry](#toolregistry)
3. [Pipeline Integration](#pipeline-integration)
4. [Tool Categories](#tool-categories)
   - [System Tools](#system-tools)
   - [macOS & Clipboard Tools](#macos--clipboard-tools)
   - [Search Tools](#search-tools)
   - [File & Shell Tools](#file--shell-tools)
   - [Agent & Delegation Tools](#agent--delegation-tools)
   - [State Management Tools](#state-management-tools)
   - [Special Tools](#special-tools)
   - [Dynamic Tools](#dynamic-tools)
5. [Registration Flow](#registration-flow)
6. [Background vs Synchronous](#background-vs-synchronous)

---

## Tool Trait

Every tool lives in `src/tools/` and implements the `Tool` trait:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON Schema
    fn is_background(&self) -> bool;            // default: false
    fn preamble(&self) -> Option<&'static str>; // spoken preamble for background tools
    fn is_silent(&self) -> bool;                // default: false (NoopTool only)
    fn should_force_for(&self, query: &str) -> bool; // default: false
    async fn run(&self, args: &str) -> String;
}
```

**Key fields:**

| Method | Purpose |
|---|---|
| `name()` | Unique identifier for the tool (e.g. `"web_search"`) |
| `description()` | Natural-language description shown to the LLM |
| `parameters()` | JSON Schema defining expected arguments |
| `is_background()` | If `true`, tool runs asynchronously and result arrives later |
| `preamble()` | Text spoken aloud while a background tool runs |
| `is_silent()` | If `true`, pipeline stops entirely and produces no audio |
| `should_force_for()` | If `true` for a query, the tool is force-invoked bypassing LLM choice |
| `run()` | Core execution — receives JSON args as a string, returns result text |

---

## ToolRegistry

Central registry in `src/tools/mod.rs`, backed by `HashMap<String, Arc<dyn Tool>>`.

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    tool_defs_cache: RwLock<Option<Vec<serde_json::Value>>>,
    // ...
}
```

**Key methods:**

| Method | Purpose |
|---|---|
| `register()` / `unregister()` | Add or remove a tool at runtime |
| `tool_definitions()` | Returns the OpenAI-compatible `tools` array (lazily cached, invalidated on register/unregister) |
| `system_prompt_section()` | Generates Spanish-language tool usage rules embedded in the system prompt |
| `forced_tool_for_query()` | Scans all tools for `should_force_for()` match; returns the matching tool if found |
| `parse_tool_call()` | Extracts `(tool_name, args)` from `"tool_name: args"` delimiter syntax |
| `execute()` | Dispatches a tool call by name, returning the result string |

---

## Pipeline Integration

### Flow diagram

```
User utterance
     │
     ▼
STT (speech-to-text)
     │
     ▼
LLM generates response ──► emits "tool_name: {args}" in text stream
     │                           │
     │                           ▼
     │              ToolRegistry.forced_tool_for_query(query)
     │              (if match → sets tool_choice, forces tool call)
     │                           │
     │                           ▼
     │              ToolRegistry.parse_tool_call(response)
     │              extracts (tool_name, args) from delimiter syntax
     │                           │
     │                           ▼
     │              ┌── is_background()? ──┐
     │              │                      │
     │             YES                    NO
     │              │                      │
     │              ▼                      ▼
     │     tokio::spawn(tool.run())    tool.run() (sync)
     │     speak preamble              block LLM turn
     │     return immediately          inject result into LLM context
     │              │                      │
     │              ▼                      ▼
     │     ProactiveEvent::AgentResult   LLM continues with result
     │     → later injected into conv.
     │
     ▼
Audio output (TTS)
```

### Background vs Synchronous dispatch logic

| Aspect | Synchronous Tool | Background Tool |
|---|---|---|
| Execution | Blocks the LLM turn | `tokio::spawn` — non-blocking |
| Preamble | None | Spoken immediately (e.g. "Buscando en internet.") |
| Result delivery | Injected directly into LLM context | Delivered later via `ProactiveEvent::AgentResult` |
| User experience | LLM waits, then continues speaking | LLM returns early, result arrives asynchronously |
| Error handling | Returned inline | `SubtaskTracker` records status; `list_tasks` queries it |

### Silent dispatch

If `is_silent()` returns `true` (only `NoopTool`), the pipeline halts entirely — no LLM continuation, no audio.

---

## Tool Categories

### System Tools

#### `current_time` — `src/tools/current_time.rs`

| Property | Value |
|---|---|
| **File** | `current_time.rs` |
| **Type** | Synchronous |
| **Description** | Returns current local date/time in the format `"HH:MM:SS, Weekday DD Month YYYY"`. |
| **Parameters** | None |
| **Force triggers** | `"¿Qué hora es?"`, `"What time is it?"`, and similar Spanish/English time queries |
| **Dependencies** | `chrono::Local` |
| **Registered** | Always |

#### `noop` — `src/tools/noop.rs`

| Property | Value |
|---|---|
| **File** | `noop.rs` |
| **Type** | Synchronous |
| **Description** | Suppresses any LLM response — the pipeline stops, no audio output is produced. |
| **Parameters** | None |
| **Silent** | `true` (only tool with this behaviour) |
| **Force triggers** | None |
| **Dependencies** | None |
| **Config** | `NOOP_TOOL_INSTRUCTIONS` (env var sets the description shown to the LLM) |
| **Registered** | Only when `NOOP_TOOL_INSTRUCTIONS` is set |

---

### macOS & Clipboard Tools

#### `read_clipboard` — `src/tools/clipboard.rs`

| Property | Value |
|---|---|
| **File** | `clipboard.rs` |
| **Type** | Synchronous |
| **Description** | Returns the current macOS clipboard contents. |
| **Parameters** | None |
| **Dependencies** | macOS `pbpaste` |
| **Registered** | Always (macOS only) |

#### `set_clipboard` — `src/tools/clipboard.rs`

| Property | Value |
|---|---|
| **File** | `clipboard.rs` |
| **Type** | Synchronous |
| **Description** | Writes text to the macOS clipboard. |
| **Parameters** | `{ "text": "..." }` |
| **Dependencies** | macOS `pbcopy` |
| **Registered** | Always (macOS only) |

#### `open_app` — `src/tools/open_app.rs`

| Property | Value |
|---|---|
| **File** | `open_app.rs` |
| **Type** | Synchronous |
| **Description** | Opens a macOS application by name. |
| **Parameters** | `{ "name": "AppName" }` |
| **Force triggers** | `"abre "`, `"lanza "`, `"inicia "`, `"launch "`, `"start "` |
| **Dependencies** | macOS `open -a` |
| **Registered** | Always (macOS only) |

#### `apple_events` — `src/tools/apple_events.rs`

| Property | Value |
|---|---|
| **File** | `apple_events.rs` |
| **Type** | Synchronous |
| **Description** | Accesses macOS Calendar and Reminders via AppleScript (`osascript`). |
| **Parameters** | `{ "operation": "...", ... }` |
| **Operations** | **Calendar:** `list_calendars`, `list_events`, `create_event`, `delete_event` |
| | **Reminders:** `list_reminder_lists`, `list_reminders`, `create_reminder`, `complete_reminder`, `delete_reminder` |
| **Limits** | Max 50 reminders, max 5 events (Calendar AppleScript is slow for large calendars) |
| **Dependencies** | macOS `osascript` |
| **Config** | `APPLE_EVENTS_ENABLED` |
| **Registered** | Only when `APPLE_EVENTS_ENABLED` is set |

#### `open_terminal` — `src/tools/open_terminal.rs`

| Property | Value |
|---|---|
| **File** | `open_terminal.rs` |
| **Type** | Synchronous |
| **Description** | Opens macOS Terminal.app running the OpenCode TUI to watch agent progress. |
| **Parameters** | None |
| **Force triggers** | `"abre opencode"`, `"muestra la terminal"`, and similar |
| **Dependencies** | macOS `osascript` |
| **Registered** | Only on macOS when remote agents are configured |

---

### Search Tools

#### `web_search` — `src/tools/web_search.rs`

| Property | Value |
|---|---|
| **File** | `web_search.rs` |
| **Type** | **Background** |
| **Description** | Searches the web via a SearXNG instance. Returns formatted results (title, content, URL). May optionally run a secondary LLM synthesis pass for voice-ready summaries. |
| **Parameters** | `{ "query": "...", "max_results": 5 }` |
| **Preamble** | `"Buscando en internet."` |
| **Force triggers** | `"busca "`, `"search for "`, `"google "`, and similar |
| **Dependencies** | `reqwest`; optional `LlmProvider` for synthesis |
| **Config** | `SEARXNG_URL`, `SEARXNG_SECRET` |
| **Registered** | Only when `SEARXNG_URL` is configured |

#### `quick_search` — `src/tools/quick_search.rs`

| Property | Value |
|---|---|
| **File** | `quick_search.rs` |
| **Type** | Synchronous (~1–3 second response) |
| **Description** | Fast-path web search via a configured `SearchProvider` (Tavily, Exa, or SearXNG). Synchronous unlike `web_search`. |
| **Parameters** | `{ "query": "...", "max_results": 5 }` |
| **Dependencies** | `SearchProvider` trait |
| **Registered** | Only when a `SearchProvider` is configured |

---

### File & Shell Tools

#### `read_file` — `src/tools/read_file.rs`

| Property | Value |
|---|---|
| **File** | `read_file.rs` |
| **Type** | Synchronous |
| **Description** | Reads the text content of a file. Expands `~` to the home directory. Rejects binary files. |
| **Parameters** | `{ "path": "..." }` |
| **Output cap** | 16 KB (truncated with notice) |
| **Dependencies** | `tokio::fs` |
| **Registered** | Always |

#### `run_shell` — `src/tools/run_shell.rs`

| Property | Value |
|---|---|
| **File** | `run_shell.rs` |
| **Type** | **Background** |
| **Description** | Executes a shell command via `sh -c`. Returns stdout, stderr, and exit code. Includes a safety denylist. |
| **Parameters** | `{ "command": "..." }` |
| **Preamble** | `"Ejecutando el comando."` |
| **Output cap** | 2000 bytes |
| **Safety** | Denylist of dangerous patterns (`rm -rf /`, fork bombs, etc.) |
| **Dependencies** | `tokio::process` |
| **Config** | `SHELL_ENABLED=1`, `SHELL_TIMEOUT_SECS` (default 30) |
| **Registered** | Only when `SHELL_ENABLED=1` |

---

### Agent & Delegation Tools

#### `deep_research` — `src/tools/deep_research.rs`

| Property | Value |
|---|---|
| **File** | `deep_research.rs` |
| **Type** | **Background** |
| **Description** | Delegates complex research to an autonomous agent via CLI. Includes conversation history context. |
| **Parameters** | `{ "query": "..." }` |
| **Preamble** | `"Investigando en profundidad."` |
| **Dependencies** | `AgentConfig`, `run_agent::call_agent` |
| **Registered** | Only when an agent is configured |

#### `run_{name}` — `src/tools/run_agent.rs`

| Property | Value |
|---|---|
| **File** | `run_agent.rs` |
| **Type** | Synchronous (spawns background tasks internally) |
| **Description** | Unified agent delegation with three execution modes: |
| | - **`cli`:** one-shot subprocess via `call_agent()` |
| | - **`acp`:** persistent JSON-RPC 2.0 stdio session via `AcpWriter` |
| | - **`remote`:** HTTP transport to an OpenCode/Hermes server |
| | **Inline commands:** `"cancel"` (cancel running task), `"status"` (report active tasks) |
| **Parameters** | `{ "task": "..." }` or `"cancel"` / `"status"` |
| **Dependencies** | `AcpWriter`, `AcpSessionManager`, `DashMap`, `ProactiveEvent` |
| **Registered** | One per `AgentConfig` (via agent bridge) |

#### `list_tasks` — `src/tools/subtask.rs`

| Property | Value |
|---|---|
| **File** | `subtask.rs` |
| **Type** | Synchronous |
| **Description** | Lists the status of all background tasks (running, completed, failed) so the LLM can query progress. |
| **Parameters** | None |
| **Dependencies** | `SubtaskTracker` (auto-registered via `register_list_tasks()`) |
| **Registered** | Always (at end of registration flow) |

---

### State Management Tools

#### `set_conversation_mode` — `src/tools/conversation_mode.rs`

| Property | Value |
|---|---|
| **File** | `conversation_mode.rs` |
| **Type** | Synchronous |
| **Description** | Switches between **Active** and **Ambient** listening modes. `AmbientLocked` stays locked until the user explicitly requests Active. |
| **Parameters** | `{ "mode": "active" | "ambient" }` |
| **Dependencies** | `Arc<Mutex<ConversationMode>>` |
| **Registered** | Always |

#### `set_prompt_build` — `src/tools/prompt_build.rs`

| Property | Value |
|---|---|
| **File** | `prompt_build.rs` |
| **Type** | Synchronous |
| **Description** | Controls prompt-build mode (`start` / `update` / `cancel`). While active, all user messages are interpreted as prompt modification instructions. |
| **Parameters** | `{ "action": "start" | "update" | "cancel", "prompt": "..." }` |
| **Force triggers** | `"prompt build"`, `"construir un prompt"`, and similar |
| **Dependencies** | `Arc<Mutex<PromptBuildState>>` |
| **Registered** | Always |

#### `switch_plugin` — `src/tools/switch_plugin.rs`

| Property | Value |
|---|---|
| **File** | `switch_plugin.rs` |
| **Type** | Synchronous |
| **Description** | Activates or switches the active plugin. |
| **Parameters** | `{ "plugin_name": "..." }` |
| **Dependencies** | `PluginManager`, `PluginSwitchEvent` |
| **Registered** | Only when plugins are available |

---

### Special Tools

#### `take_screenshot` — `src/tools/take_screenshot.rs`

| Property | Value |
|---|---|
| **File** | `take_screenshot.rs` |
| **Type** | Synchronous |
| **Description** | Captures the screen via `screencapture`, sends the PNG to a vision-capable LLM, and returns a text description of what is shown. |
| **Parameters** | `{ "prompt": "optional focus for vision analysis" }` (optional) |
| **Dependencies** | `LlmProvider` (vision model), `screen_capture` module |
| **Config** | `SECONDARY_LLM_URL` |
| **Registered** | Only when `SECONDARY_LLM_URL` is configured |

#### `recover_historical_context` — `src/tools/recover_historical_context.rs`

| Property | Value |
|---|---|
| **File** | `recover_historical_context.rs` |
| **Type** | Synchronous |
| **Description** | Searches the L2 (long-term) message archive using SQLite FTS5 full-text search. Returns ranked results with rank, role, session, timestamp, snippet, and full content. |
| **Parameters** | `{ "query": "...", "session_id": "optional UUID", "limit": 10 }` |
| **Dependencies** | `Database` (SQLite FTS5) |
| **Registered** | Only when a database is available |

---

### Dynamic Tools

#### `mcp_tool` — `src/tools/mcp_tool.rs`

| Property | Value |
|---|---|
| **File** | `mcp_tool.rs` |
| **Type** | **Background** (always — MCP execution time is unpredictable) |
| **Description** | Wraps a single MCP server tool as a `dyn Tool`. The name takes the format `{server_name}_mcp__{tool_name}`. Parameters are inherited from the MCP server's `inputSchema`. |
| **Preamble** | `"Procesando en segundo plano."` |
| **Dependencies** | `McpClient` |
| **Discovery** | Tools are discovered at startup via MCP `tools/list` |
| **Registration** | For each MCP configuration, `McpClient::spawn_and_init()` is called, and every returned tool is registered as an `McpToolProxy` |

---

## Registration Flow

Tools are registered conditionally at startup based on configuration. The order below is the actual registration order in `src/main.rs`:

| # | Tool | Condition |
|---|---|---|
| 1 | `current_time` | Always |
| 2 | `read_file` | Always |
| 3 | `read_clipboard` | Always |
| 4 | `set_clipboard` | Always |
| 5 | `open_app` | Always |
| 6 | `set_conversation_mode` | Always |
| 7 | `set_prompt_build` | Always |
| 8 | `apple_events` | `APPLE_EVENTS_ENABLED` is set |
| 9 | `run_shell` | `SHELL_ENABLED=1` |
| 10 | `take_screenshot` | `SECONDARY_LLM_URL` is configured |
| 11 | `web_search` | `SEARXNG_URL` is configured |
| 12 | `quick_search` | `SearchProvider` is configured |
| 13 | `deep_research` | Agent is configured |
| 14 | `run_{name}` (RunAgent) | One per `AgentConfig` |
| 15 | `open_terminal` | macOS + remote agents configured |
| 16 | `mcp_tool` (McpToolProxy) | One per MCP-discovered tool |
| 17 | `switch_plugin` | Plugins are available |
| 18 | `recover_historical_context` | Database is available |
| 19 | `noop` | `NOOP_TOOL_INSTRUCTIONS` is set |
| 20 | `list_tasks` | Always (via `register_list_tasks()`) |

---

## Background vs Synchronous

### Synchronous tools (blocking)

| Tool | Typical latency |
|---|---|
| `current_time` | < 1 ms |
| `read_clipboard` | ~10 ms |
| `set_clipboard` | ~10 ms |
| `open_app` | ~100–500 ms |
| `apple_events` | ~100 ms – 5 s (calendar queries are slow) |
| `open_terminal` | ~100 ms |
| `take_screenshot` | ~2–5 s (capture + vision LLM) |
| `read_file` | ~1–50 ms |
| `recover_historical_context` | ~10–100 ms |
| `quick_search` | ~1–3 s |
| `set_conversation_mode` | < 1 ms |
| `set_prompt_build` | < 1 ms |
| `switch_plugin` | ~10 ms |
| `list_tasks` | < 1 ms |
| `noop` | < 1 ms |
| `run_{name}` (RunAgent) | Returns immediately (spawns internally) |

### Background tools (async)

| Tool | Preamble | Typical execution |
|---|---|---|
| `web_search` | `"Buscando en internet."` | ~2–10 s |
| `run_shell` | `"Ejecutando el comando."` | Configurable timeout (default 30 s) |
| `deep_research` | `"Investigando en profundidad."` | 30 s – several minutes |
| `mcp_tool` | `"Procesando en segundo plano."` | Unpredictable (depends on MCP server) |

Background tools are spawned via `tokio::spawn`. The pipeline immediately speaks the preamble and returns control to the user. Results arrive later through `ProactiveEvent::AgentResult`, which injects them into the conversation history. The `SubtaskTracker` keeps a running record of all background tasks so the LLM can query their status at any time via `list_tasks`.

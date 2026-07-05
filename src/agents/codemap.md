# src/agents/ — Agent Delegation

## Responsibility

Provides the infrastructure for delegating complex tasks to external AI agents via the **ACP (Agent Communication Protocol)**. Supports two communication modes: CLI (one-shot subprocess) and ACP (persistent JSON-RPC 2.0 over stdio). Manages agent configuration, session lifecycle, and event routing for proactive results.

## Design

### `AgentConfig` (`config.rs`)

Per-agent configuration loaded from environment variables:

```rust
pub struct AgentConfig {
    pub name: String,           // unique name → tool suffix: run_{name}
    pub mode: String,           // "cli" or "acp"
    pub command: Option<String>, // CLI command (e.g. "hermes chat")
    pub acp_command: String,    // ACP command (e.g. "hermes acp")
    pub acp_warmup: bool,       // send warmup prompt at startup
    pub when_to_use: String,    // LLM-facing: when to delegate
    pub instructions: String,   // agent-facing: role, capabilities, style
}
```

### `AgentRegistry` (`config.rs`)

Loads agents from environment variables with two formats:

1. **Multi-agent** (`AGENTS=hermes,oracle`): Each agent loaded via `AGENT_<NAME>_*` env vars (`AGENT_HERMES_MODE`, `AGENT_HERMES_ACP_COMMAND`, `AGENT_HERMES_WHEN_TO_USE`, `AGENT_HERMES_INSTRUCTIONS`).
2. **Legacy single-agent** (`AGENT_COMMAND` / `AGENT_MODE`): Backward-compatible single `"hermes"` agent.

`system_prompt_section()` generates the `## AGENTES EXTERNOS DISPONIBLES` section inserted into the system prompt.

### `ProactiveEvent` (`mod.rs`)

Enum of events that trigger proactive speech from the voicebot without a user utterance:

- **`AgentResult`** — Background agent task completed. Contains `task`, `result`, optional `tool_call_id`, and `correlation_id`. When `tool_call_id` is `Some`, the pipeline injects the result as an OpenAI tool result message.
- **`InferenceDaemon`** — The inference daemon decided there is something worth saying.
- **`AgentQuestion`** — An ACP agent is requesting user permission. Contains `task_id`, `agent_name`, `question`, `options`, and a `oneshot::Sender<String>` for the response.
- **`L1Saturated`** — Memory context saturation detected.
- **`PluginSwitch`** — Plugin activation requested.

### `AcpSessionManager` (`session_manager.rs`)

Manages persistent ACP sessions keyed by agent name:

```rust
pub struct AcpSessionManager {
    sessions: DashMap<String, SessionEntry>,
    backoff_states: DashMap<String, BackoffState>,
}
```

Key operations:
- **`get_or_create_session()`** — Reuses existing session or spawns new ACP subprocess via `AcpWriter::spawn()` + `initialize()`.
- **`get_healthy_session()`** — Checks if the ACP process is alive; respawns with exponential backoff if dead. Backoff: `base_secs * 2^attempts`, capped at `max_secs`.
- **`close_session()`** — Removes session and drains associated task IDs.
- **`cleanup_idle_sessions()`** — Removes sessions idle longer than a timeout.
- **`prewarm_agent()`** — Spawns + initializes + sends warmup prompt.

### `SessionEntry` (`session_manager.rs`)

Handle to a live ACP session:

```rust
pub struct SessionEntry {
    pub writer: Arc<Mutex<AcpWriter>>,
    pub inbound_rx: Arc<Mutex<mpsc::Receiver<JsonRpcMessage>>>,
    pub session_id: String,
    pub agent_name: String,
    pub created_at: Instant,
    pub last_used: Instant,
    pub task_ids: HashSet<String>,
    pub status: SessionStatus,  // Started | Idle | Busy | Closed
}
```

### `AcpSessionEvent` (`session_events.rs`)

Events extracted from `session/update` ACP notifications:

- `AgentMessageChunk` — Streaming text content from the agent.
- `AgentThoughtChunk` — Internal reasoning/thoughts.
- `ToolCall` — Agent initiated a tool call.
- `ToolCallUpdate` — Tool call status update.
- `PermissionRequest` — Agent requesting user permission.

`parse_session_update()` deserializes JSON params into the appropriate variant.

## Flow

### Agent Configuration Loading

```
Startup → AgentRegistry::from_env()
    → Check AGENTS env var → parse comma-separated names
    → For each name: load_agent_from_env(name) → AgentConfig
    → Fallback: check AGENT_COMMAND/AGENT_MODE → load_legacy_agent()
    → Result: Vec<AgentConfig>
```

### ACP Session Lifecycle

```
RunAgentTool.run_acp(task) → AcpSessionManager.get_or_create_session(config)
    → If session exists: check is_alive() → return or respawn with backoff
    → If new: AcpWriter::spawn(acp_command) → initialize() → session/new → SessionEntry
    → send_prompt(session_id, query) → collect_acp_response()
        → Loop: recv JsonRpcMessage
            → session/update (agent_message_chunk) → accumulate text
            → session/update (tool_call) → track progress
            → session/request_permission → ProactiveEvent::AgentQuestion → wait for user response
            → Response with matching id → task complete
            → cancel_rx → send_cancel() → return cancelled
    → synthesize_agent_result() → ProactiveEvent::AgentResult
```

### CLI Mode (Simpler Path)

```
RunAgentTool.run_cli(task) → build_agent_query(history, task, instructions)
    → call_agent(command, query) → spawn subprocess with -q flag
    → wait_with_output() → strip_hermes_cli_noise() → synthesize_agent_result()
    → ProactiveEvent::AgentResult
```

## Integration

**Consumers**:
- `src/tools/run_agent.rs` — `RunAgentTool` uses `AgentConfig`, `AcpSessionManager`, and emits `ProactiveEvent`.
- `src/tools/deep_research.rs` — `DeepResearchTool` delegates to `call_agent()` via `AgentConfig`.
- `src/plugins/agent_bridge.rs` — Resolves plugin agent configs to `AgentConfig`, registers `RunAgentTool` instances.
- `src/pipeline/` — Receives `ProactiveEvent` via `mpsc::Receiver`.
- `src/daemon.rs` — Handles `ProactiveEvent::AgentQuestion` for permission routing.

**Dependencies**:
- `src/tools/run_agent::{AcpWriter, JsonRpcMessage, ActiveTask}` — ACP subprocess I/O.
- `src/config::{Config, HermesSessionViewerMode}` — Backoff timing, log viewer mode.
- `src/llm::LlmProvider` — Result synthesis via secondary LLM.
- `dashmap`, `tokio::sync`, `anyhow`, `serde_json`, `tracing`.
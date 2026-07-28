# Agent System Internals — Architecture Reference

The agent system (`src/agents/`) enables Seneschal to delegate tasks to external sub-agents via different protocols: CLI subprocess, ACP JSON-RPC over stdio, HTTP remote (Hermes/OpenCode), and visible PTY sessions.

## Architecture Overview

```
AgentRegistry (config layer)
   │
   └── AgentConfig[]
        ├── mode: "cli"      → fire-and-forget subprocess
        ├── mode: "acp"      → persistent JSON-RPC session
        ├── mode: "remote"   → HTTP transport (Hermes or OpenCode)
        └── mode: "visible"  → PTY visible in Terminal.app
              │
              ▼
         RunAgentTool (registered as "run_{name}")
              │
              ├── AcpSessionManager   (for acp mode)
              ├── HttpAgentTransport  (for remote mode)
              └── VisibleSessionManager (for visible mode)
              │
              ▼
         ProactiveEvent channel
              │
              ▼
         Pipeline main loop (speak result, handle questions)
```

## AgentConfig

```rust
pub struct AgentConfig {
    pub name: String,
    pub mode: String,              // "cli", "acp", "remote", "visible"
    pub command: Option<String>,
    pub acp_command: String,
    pub acp_warmup: bool,
    pub remote_url: String,
    pub remote_dir: String,
    pub remote_session_path: String,
    pub remote_message_path: String,
    pub remote_event_path: String,
    pub remote_api_key: String,    // non-empty = Hermes protocol
    pub when_to_use: String,       // LLM-facing delegation instructions
    pub instructions: String,      // agent-facing instructions
}
```

### Loading Order (AgentRegistry)

```
1. AGENTS=name1,name2 env vars (multi-agent format)
2. AGENT_COMMAND + AGENT_MODE (legacy single agent)
3. [[agents]] TOML array in config file
4. Empty registry (no agents)
```

Environment variable convention:
- `AGENTS=hermes,other` (comma-separated)
- Per-agent: `AGENT_{UPPER_NAME}_MODE`, `AGENT_{UPPER_NAME}_ACP_COMMAND`, `AGENT_{UPPER_NAME}_REMOTE_URL`, etc.

## ACP Session Manager

`AcpSessionManager` manages persistent ACP (Agent Communication Protocol) sessions via JSON-RPC 2.0 over stdio.

```rust
pub struct AcpSessionManager { /* DashMap<String, SessionEntry> */ }
```

### Session Lifecycle

```
SessionManager::get_or_create_session(agent_config)
    │
    ├── existing & alive → return (mark idle if was not busy)
    │
    └── new or dead:
        1. AcpWriter::spawn(acp_command)
           └── spawns subprocess with piped stdin/stdout
        2. writer.initialize(rx, cwd, viewer_mode)
           ├── sends {"method": "initialize", "params": {...}}
           └── sends {"method": "session/new", "params": {...}}
        3. Store session in DashMap with status "Idle"
```

### Session States

| State | Meaning |
|-------|---------|
| `Idle` | Ready to accept a prompt |
| `Busy` | Currently processing (don't reuse until done) |
| `NeedsInput` | Agent requested user input (permission) |
| `Done` | Task completed |
| `Error` | Task failed |
| `Closed` | Session terminated |

### Key Methods

```rust
pub fn get_or_create_session(&self, config: &AgentConfig) -> Result<(String, ...)>;
pub fn mark_session_idle(&self, agent_name: &str);
pub fn mark_session_busy(&self, agent_name: &str);
pub fn close_session(&self, session_id: &str);
pub fn send_user_message(&self, agent_name: &str, text: &str);
pub fn prewarm_agent(&self, config: &AgentConfig);
pub fn cleanup_idle_sessions(&self, timeout_secs: u64);
```

### Health Check & Recovery

`get_healthy_session()` — if `writer.is_alive()` returns false:
1. Calls `close_session()` to clean up.
2. Retries `create_session()` with **exponential backoff** (`agent_acp_restart_backoff_secs` → `agent_acp_restart_max_backoff_secs`).

## AcpWriter (JSON-RPC stdio)

```rust
pub struct AcpWriter { /* child process, stdin writer */ }
```

### JSON-RPC Methods Sent

| Method | Direction | Purpose |
|--------|-----------|---------|
| `initialize` | Client → Server | Handshake; expects `protocolVersion: 1` |
| `session/new` | Client → Server | Create new session; expects `sessionId` |
| `session/prompt` | Client → Server | Send user prompt; expects task result |
| `session/cancel` | Client → Server (notification) | Cancel running task by request ID |
| `session/list` | Client → Server | List all sessions |
| `session/fork` | Client → Server | Fork existing session |
| `session/load` | Client → Server | Load persisted session |

### Drop Behavior
`Drop::drop` sends `SIGKILL` via `libc::kill` to the child PID.

## Remote Transport (HTTP)

`HttpAgentTransport` (aliased `OpenCodeHttpTransport`) handles both Hermes and OpenCode protocols.

### Protocol Detection
- **Hermes mode:** `remote_api_key` is set → `Authorization: Bearer <key>` header
- **OpenCode mode:** `remote_api_key` is empty → `x-opencode-directory: <dir>` header

### Endpoints

| Purpose | OpenCode | Hermes |
|---------|----------|--------|
| Session/Run creation | `POST /session` | `POST /v1/runs` |
| Message submission | `POST /session/{id}/message` | `POST /v1/runs` (reuses create path) |
| SSE event stream | `GET /event` | `GET /v1/runs/{id}/events` |
| Cancel | CancellationToken only | `POST /v1/runs/{id}/stop` |

### Key Methods

```rust
pub async fn get_or_create_session(&self) -> Result<OpenCodeSession>;
pub async fn submit_prompt(&self, session_id: &str, prompt: &str, cancel: CancellationToken) -> Result<String>;
pub fn subscribe_events(&self, session_id: &str) -> (mpsc::Receiver<OpenCodeMilestone>, CancellationToken);
pub fn subscribe_hermes_events(&self, run_id: &str) -> (mpsc::Receiver<HermesMilestone>, CancellationToken);
```

## Event Parsing (SSE)

Both protocols produce SSE text streams that are parsed into milestone events for proactive narration.

### Hermes SSE Events

| SSE event | HermesEvent variant | Milestone |
|-----------|---------------------|-----------|
| `run.started` | `RunStarted` | "Iniciando tarea remota" |
| `run.completed` | `RunCompleted` | (skipped) |
| `run.failed` | `RunFailed` | (skipped) |
| `tool.started` | `ToolStarted` | "Está usando {tool_name}" |
| `tool.completed` | `ToolCompleted` | "Terminó de usar {tool_name}" |
| `approval.request` | `ApprovalRequested` | "Hermes pide permiso para {action}" |

### OpenCode SSE Events

| SSE event | OpenCodeEvent variant | Milestone |
|-----------|----------------------|-----------|
| `tool.invoked` | `ToolInvoked` | "Está usando {tool_name}" |
| `tool.completed` | `ToolCompleted` | "Terminó de usar {tool_name}" |
| `permission.requested` | `PermissionRequested` | "OpenCode pide permiso para {action}" |

## Visible Agent Mode (PTY)

When `mode = "visible"`, the agent runs in a pseudo-terminal with a Terminal.app window showing live output.

```rust
pub struct VisibleSession { /* NativePtySystem, PTY master, child process */ }

pub fn spawn(command: &str, agent_name: &str, session_dir: &str) -> Result<Self>;
pub fn send(&mut self, text: &str);
pub fn receive(&mut self) -> Option<String>;
pub fn close(self);
pub fn is_alive(&self) -> bool;
```

Uses `portable_pty` crate for cross-platform PTY support (macOS Terminal integration is platform-specific via `osascript`).

## ProactiveEvent Channel

All agent results and questions flow through a single `mpsc::channel<ProactiveEvent>`.

```rust
pub enum ProactiveEvent {
    AgentResult { task, result, tool_call_id, correlation_id },
    AgentQuestion { task_id, agent_name, question, options, response_tx },
    AgentMilestone { agent_name, milestone, correlation_id },
    InferenceDaemon { message },
    McpNotification { server_name, method, params },
    L1Saturated { total_chars, threshold },
    PluginSwitch { plugin_id },
    DeviceConnected,
}
```

The main pipeline loop drains this channel and injects results as LLM context or speaks them directly via TTS.

## Startup Sequence

```
1. AgentRegistry::from_config_and_env(config.agents)
2. Arc<AcpSessionManager>::new()
3. Arc<VisibleSessionManager>::new()
4. For each agent:
   a. Create RunAgentTool
   b. If remote → attach HttpAgentTransport
   c. If acp → attach AcpSessionManager
   d. If visible → attach VisibleSessionManager
   e. Register as "run_{name}" in ToolRegistry
5. If acp_warmup → spawn prewarm task
6. If keepalive → spawn AcpKeepAliveDaemon
```

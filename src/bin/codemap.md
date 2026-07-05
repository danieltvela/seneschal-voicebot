# src/bin/ — Debug/Test Binaries

## Responsibility

Standalone debug and test binaries for the Voicebot project. Currently contains a single binary: `acp_agent_chat.rs` — an interactive terminal chat client for testing the ACP (Agent Communication Protocol) protocol over JSON-RPC 2.0 via stdio.

## Design

### Binary: `acp_agent_chat.rs`

**Purpose**: Debug/test TUI for interacting with an ACP agent (e.g., Hermes) via JSON-RPC 2.0 over stdio. Supports slash commands for testing the full ACP protocol surface.

**Configuration**: Reuses `voicebot::config::Config` (reads `.env` / env vars). Key config: `AGENT_ACP_COMMAND` (default `"hermes acp"`).

### Core Components

- **`AcpWriter`** — Spawns the ACP agent process, manages JSON-RPC communication over stdio.
- **`JsonRpcMessage`** — Enum for JSON-RPC messages: `Request`, `Response`, `Notification`.
- **`PermissionMode`** — How permission requests are handled: `Auto` (allow all), `Ask` (prompt user), `Deny` (deny all).

### Slash Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/quit` / `/exit` | Exit the chat |
| `/verbose` | Toggle raw JSON-RPC message logging |
| `/session` | Show current session info (ID, messages, uptime, verbose, permissions) |
| `/sessions` | List active sessions (unstable) |
| `/new` | Start a fresh session |
| `/fork` | Fork current session (unstable) |
| `/load <id>` | Load a previous session |
| `/resume <id>` | Resume a suspended session (unstable) |
| `/cancel` | Cancel current operation |
| `/permissions <mode>` | Change permission handling: `auto`, `ask`, `deny` |
| `/raw <json>` | Send raw JSON-RPC message |

### Session Management

```
AcpWriter::initialize(rx, cwd, HermesSessionViewerMode::Off)
  → Send initialize request
  → Wait for response with sessionId
  → Return session_id
```

## Flow

### Startup

```
main():
  → tracing_subscriber::fmt().init()
  → dotenvy::dotenv()
  → Config::from_env()
  
  → AcpWriter::spawn(acp_command) → (writer, rx)
  → writer.initialize(rx, cwd, Off) → session_id
  
  → Print splash screen
  → Enter main loop
```

### Main Loop

```
loop:
  → Print "You> " prompt
  → Read stdin line
  
  → If starts with '/': dispatch slash command
  → Else: send prompt to agent
  
  send_prompt:
    → prompt_id = writer.send_prompt(session_id, input)
    → last_prompt_id = prompt_id
    → message_count += 1
    → Print "Hermes> "
    → response = collect_response(writer, rx, prompt_id, permission_mode)
    → Print response
```

### Response Collection

```
collect_response(writer, rx, prompt_id, permission_mode):
  accumulated_text = ""
  progress = []
  
  loop:
    msg = rx.recv()
    
    match msg:
      Response { id == prompt_id }:
        → If error: return "[Error: {err}]"
        → If accumulated_text: return text + optional progress
        → If no text: return "[Agent finished with stopReason={reason}]"
      
      Notification { method: "session/update" }:
        agent_message_chunk:
          → accumulated_text += content.text
        agent_thought_chunk:
          → Skip (silent)
        tool_call:
          → Print "[using {tool_name}...]" to stderr
          → progress.push("using {tool_name}")
        tool_call_update / tool_result:
          → Skip
      
      Request { method: "session/request_permission" }:
        → Handle based on permission_mode:
          Auto: find_allow_option(params) → auto-allow
          Deny: None → cancel
          Ask: prompt user → select option
        → Send response (selected or cancelled)
      
      Other: → Skip
```

### Permission Handling

```
find_allow_option(params):
  → Search options array for optionId == "allow" or kind == "allow"
  → Fallback: first option in array
```

### Slash Command Dispatch

```
/quit → break loop
/help → print_help()
/verbose → toggle writer.verbose flag
/cancel → writer.send_cancel(last_prompt_id)
/session → Print session_id, message_count, uptime, verbose, permission_mode
/sessions → writer.send_list_sessions(cwd) → wait_for_response
/new → writer.send_new_session(cwd) → wait_for_session_id → update current
/fork → writer.send_fork_session(current_id, cwd) → wait_for_session_id → update
/load <id> → writer.send_load_session(id, cwd) → wait_for_session_id → update
/resume <id> → writer.send_resume_session(id, cwd) → wait_for_session_id → update
/permissions <mode> → Update permission_mode
/raw <json> → Parse JSON → writer.write_json(json)
```

### Cleanup

```
On exit:
  → writer.kill() → terminate ACP process
```

## Integration

### Dependencies
- `voicebot::config::{Config, HermesSessionViewerMode}` — Shared configuration.
- `voicebot::tools::run_agent::{AcpWriter, JsonRpcMessage}` — ACP protocol implementation.
- `tokio` — Async runtime.
- `tracing` / `tracing_subscriber` — Logging.
- `dotenvy` — Environment loading.
- `anyhow` — Error handling.
- `serde_json` — JSON serialization.

### Consumers
- Standalone binary: `cargo run --bin acp_agent_chat`.
- No other modules depend on this binary.

### Protocol
- JSON-RPC 2.0 over stdio (stdin/stdout of spawned process).
- ACP protocol methods: `initialize`, `prompt`, `cancel`, `list_sessions`, `new_session`, `fork_session`, `load_session`, `resume_session`, `request_permission`.
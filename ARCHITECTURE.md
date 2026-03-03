# Voicebot Architecture

## Project Structure

```
src/
├── main.rs                      # Application entry point
├── lib.rs                       # Library exports
├── config.rs                    # Configuration management
├── websocket_client.rs          # WebSocket client (existing)
│
├── audio/                       # Audio processing components
│   ├── mod.rs                   # Audio module exports
│   ├── audio_capture.rs         # Microphone input capture
│   ├── audio_transform.rs       # Audio transformation/resampling
│   ├── vad.rs                   # Voice Activity Detection
│   ├── buffer.rs                # Audio buffering
│   └── output.rs                # Speaker output
│
├── session/                     # Session management
│   ├── mod.rs                   # Session module exports
│   ├── manager.rs               # SessionManager - manages conversations
│   └── context.rs               # ConversationContext, Message types
│
├── s2s/                         # Speech-to-Speech models
│   ├── mod.rs                   # S2S module exports
│   ├── adapter.rs               # S2SAdapter - model abstraction layer
│   └── models/
│       ├── mod.rs               # Model types and config
│       ├── llama_omni.rs        # LLaMA-Omni implementation
│       ├── moshi.rs             # Moshi implementation
│       ├── ultravox.rs          # Ultravox implementation
│       └── lfm.rs               # LFM2.5-Audio implementation
│
├── tools/                       # Tool system
│   ├── mod.rs                   # Tool module exports
│   ├── router.rs                # ToolRouter - routes tool calls
│   ├── registry.rs              # ToolRegistry - manages tools
│   └── builtin/                 # Built-in tools
│       ├── mod.rs
│       ├── file_operations.rs   # File I/O tool
│       ├── web_search.rs        # Web search tool
│       └── system_info.rs       # System information tool
│
├── mcp/                         # Model Context Protocol
│   ├── mod.rs                   # MCP module exports
│   ├── server.rs                # McpServer - MCP integration
│   └── protocol.rs              # MCP protocol types
│
├── agents/                      # External agents
│   ├── mod.rs                   # Agents module exports
│   ├── manager.rs               # AgentManager - manages external agents
│   └── openclaw.rs              # OpenClaw agent integration
│
└── db/                          # Database layer
    ├── mod.rs                   # Database module exports
    ├── database.rs              # Database - SQLite operations
    └── schema.rs                # Database schema definitions
```

## Component Overview

### Audio Layer (`audio/`)
- **AudioCapture**: Captures audio from microphone
- **AudioTransformer**: Transforms/resamples audio
- **VoiceActivityDetector**: Detects speech vs silence
- **AudioBuffer**: Buffers audio chunks
- **AudioOutput**: Plays audio through speakers

### Session Layer (`session/`)
- **SessionManager**: Manages conversation sessions
- **ConversationContext**: Holds conversation state
- **Message**: Represents user/assistant messages

### S2S Model Layer (`s2s/`)
- **S2SAdapter**: Abstraction layer for interchangeable models
- **S2SModel trait**: Interface all models implement
- **Model implementations**: LlamaOmni, Moshi, Ultravox, LFM

### Tool Layer (`tools/`)
- **ToolRouter**: Routes tool calls to appropriate handlers
- **ToolRegistry**: Manages built-in tools
- **Built-in tools**: File operations, web search, system info

### MCP Layer (`mcp/`)
- **McpServer**: Integrates with MCP protocol
- **Protocol types**: Request/response structures

### Agent Layer (`agents/`)
- **AgentManager**: Manages external agents
- **OpenClawAgent**: Integration with OpenClaw

### Database Layer (`db/`)
- **Database**: SQLite operations
- **Schema**: Database structure

## Key Design Patterns

### 1. Adapter Pattern (S2S Models)
The `S2SAdapter` provides a unified interface for different S2S models, allowing them to be swapped without changing the rest of the system.

```rust
// Usage
let adapter = S2SAdapter::new(ModelType::LlamaOmni, config).await?;
let response = adapter.process(request).await?;
```

### 2. Registry Pattern (Tools)
The `ToolRegistry` maintains a registry of available tools, making it easy to add/remove tools dynamically.

### 3. Router Pattern (Tool Routing)
The `ToolRouter` routes tool calls to the appropriate handler (built-in, MCP, or agent).

### 4. Manager Pattern (Sessions & Agents)
Managers encapsulate the logic for handling multiple sessions or agents.

## Data Flow

### Input Path
```
Microphone → AudioCapture → VAD → AudioBuffer → SessionManager → S2SAdapter → Model
```

### Tool Execution Path
```
Model → ToolRouter → [ToolRegistry | McpServer | AgentManager] → Tool → Result → Model
```

### Output Path
```
Model → S2SAdapter → AudioOutput → Speaker
```

### Persistence Path
```
SessionManager ↔ Database (SQLite)
```

## Module Dependencies

```
main.rs
  ├─→ audio::*
  ├─→ session::SessionManager
  │     └─→ db::Database
  ├─→ s2s::S2SAdapter
  │     └─→ s2s::models::*
  ├─→ tools::ToolRouter
  │     ├─→ mcp::McpServer
  │     └─→ agents::AgentManager
  └─→ config::Config
```

## Async Architecture

All I/O operations are async using Tokio:
- Audio capture/playback
- Database operations
- Network requests (MCP, agents)
- Model inference (when supported)

## Error Handling

Using `anyhow::Result` for error propagation with context:
```rust
pub async fn process(&mut self) -> Result<Response> {
    self.model.process(request)
        .await
        .context("Model inference failed")?
}
```

## Testing Strategy

- **Unit tests**: Test individual components in isolation
- **Integration tests**: Test component interactions
- **Mock implementations**: Use mock models/tools for testing

## Future Enhancements

- [ ] Streaming support for real-time responses
- [ ] Multi-user support (currently mono-user)
- [ ] Model fine-tuning interface
- [ ] Advanced audio preprocessing
- [ ] Cloud model integration
- [ ] Voice biometrics

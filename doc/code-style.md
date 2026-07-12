# Code Style & Patterns

- **Error handling**: `anyhow::Result` with context strings; `thiserror` for custom types.
- **Logging**: `tracing` throughout (no println!); logs → `seneschal.log` when TUI active.
- **Async**: tokio runtime + channels (`mpsc`, `broadcast`) for inter-stage comms.
- **Cancellation**: `CancellationToken` (tokio-util) for barge-in support.
- **Serialization**: serde + serde_json.
- **Tool calling**: LLM uses `<tool_name: args>` syntax; parsed by ToolRegistry.

## When Adding Tools

1. Define tool schema in `src/tools/mod.rs` or dedicated module.
2. Implement handler returning `Result<String, Error>`.
3. Register in main pipeline's tool map.
4. Add doc comment explaining use case and limitations.

## Database Migrations

Use sqlx migrations:
```bash
sqlx migrate add <migration_name>
sqlx migrate run
```

Migrations live in `src/db/migrations/`.
# db-inspect

Standalone web viewer for Voicebot's SQLite database.

## Purpose

`db-inspect` is a small local HTTP server that exposes Voicebot's SQLite database through a read-write web interface. It is intended for debugging, manual inspection, and light administrative tasks such as removing stale sessions or messages.

## Build

From inside the `db-inspect` directory:

```bash
cargo build
```

For a release build:

```bash
cargo build --release
```

## Run

```bash
cargo run -- --db ../data/voicebot.db
```

The server starts on `http://127.0.0.1:3000` by default.

## Configuration

Configuration is layered, highest precedence first:

1. CLI flag: `--db <path>`
2. Environment variables
3. Built-in defaults

| Environment variable | Default | Description |
|---|---|---|
| `VOICEBOT_DB_PATH` | `../data/voicebot.db` | Path to the Voicebot SQLite database |
| `DB_INSPECT_PORT` | `3000` | HTTP server port |

The server always binds to `127.0.0.1` (local-only access).

## Routes

| Route | Description |
|---|---|
| `GET /` | Home / index page |
| `GET /sessions` | List all conversation sessions |
| `GET /sessions/{id}` | Detail view for a single session |
| `GET /messages` | List messages |
| `GET /search` | Full-text search across stored content |
| `GET /profile` | View user profile |
| `GET /memories` | View extracted memories |
| `GET /history` | View conversation history |
| `GET /dream-state` | View S-DREAM cold-path memory state |
| `GET /system-prompts` | View stored system prompts |
| `POST /system-prompts` | Create a new system prompt |
| `POST /system-prompts/{id}/activate` | Activate a system prompt (deactivates all others) |
| `POST /system-prompts/{id}/delete` | Delete a system prompt |
| `GET /sessions/{id}/delete` | Confirmation page to delete a session |
| `POST /sessions/{id}/delete` | Delete a session and its messages |
| `GET /messages/{id}/delete` | Confirmation page to delete a message |
| `POST /messages/{id}/delete` | Delete a single message |

## Delete operations

Deletion is performed via `POST` and requires confirming through the `GET` confirmation page. These operations modify the database, so use them with care.

## Tests

```bash
cargo test
```

## Notes

- The application opens the SQLite database in read-write mode.
- The server is intentionally bound to `127.0.0.1` and is not meant for network exposure.
- This tool is independent of Voicebot's main runtime; it only needs access to the SQLite file.

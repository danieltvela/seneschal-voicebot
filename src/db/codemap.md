# `src/db/` — SQLite Persistence Layer

## Responsibility

Persistent storage subsystem for the Voicebot inference pipeline. Manages a single SQLite database (`SqlitePool`) that stores conversation sessions, messages, user profile facts, persistent memories, system prompts, dream-state tracking, and full-text search indexes. Provides the sole data-access interface for all pipeline modules that require durability across process restarts.

## Design

### Module Structure

| File | Role |
|------|------|
| `mod.rs` | Re-exports `Database`, `Memory`, `NewMemory`, `SearchResult` |
| `database.rs` | Full implementation: schema, migrations, CRUD, FTS5, transactions |

### Schema (7 tables)

| Table | Primary Key | Purpose |
|-------|-------------|---------|
| `sessions` | `id TEXT` (UUID) | Conversation lifecycle: `is_active`, `summary`, `summary_through_id` |
| `messages` | `id INTEGER AUTOINCREMENT` | Individual turns: `session_id`, `role`, `content`, `timestamp` |
| `user_profile` | `key TEXT` | Key-value facts with `confidence` and `updated_at` |
| `profile_history` | `id INTEGER AUTOINCREMENT` | Audit trail of every profile fact change |
| `memories` | `id INTEGER AUTOINCREMENT` | Persistent memories with `category`, `is_active`, `source_session_id` |
| `dream_state` | `session_id TEXT` | S-DREAM incremental export watermark (`last_processed_at`) |
| `system_prompts` | `id INTEGER AUTOINCREMENT` | Versioned system prompts with single-`is_active` enforcement |

### FTS5 Full-Text Search

- Virtual table `messages_fts` uses **external content mode** (`content=messages`) to avoid duplicating message text.
- Tokenizer: `porter unicode61` (stemming + Unicode 6.1).
- Three synchronization triggers (`messages_ai`, `messages_ad`, `messages_au`) keep the FTS index in sync with `INSERT`/`DELETE`/`UPDATE` on `messages`.
- On every startup, `INSERT INTO messages_fts(messages_fts) VALUES('rebuild')` re-syncs the token index (no-op if already in sync).

### Migration Strategy

- All tables created with `CREATE TABLE IF NOT EXISTS` in `run_migrations()`.
- Additive column migrations use `ALTER TABLE ... ADD COLUMN` wrapped in error suppression (SQLite does not support `IF NOT EXISTS` for `ADD COLUMN`).
- No versioned migration framework; inline DDL in `run_migrations()` is idempotent.

### Key Types

```rust
pub struct Database { pool: SqlitePool }
pub struct Memory { id, content, category, source_session_id, created_at, updated_at }
pub struct NewMemory { content, category }
pub struct SearchResult { rank, message_id, session_id, role, content, timestamp, snippet }
```

## Flow

### Initialization

```
Database::new(path)
  → create parent directory (tokio::fs::create_dir_all)
  → SqliteConnectOptions::new().filename(path).create_if_missing(true)
  → SqlitePoolOptions::new().max_connections(5).connect_with(options)
  → run_migrations() (7 tables + 3 triggers + 1 FTS virtual table + rebuild)
```

### Session Lifecycle

```
get_or_create_session()
  → SELECT id FROM sessions WHERE is_active = 1 ORDER BY created_at DESC LIMIT 1
  → If found: parse UUID, return
  → If not: Uuid::new_v4(), INSERT INTO sessions, return new UUID

close_session(id)
  → UPDATE sessions SET closed_at = now, is_active = 0 WHERE id = ?
```

### Context Loading (with summary cutoff)

```
get_session_context(session_id, limit)
  → SELECT summary, summary_through_id FROM sessions WHERE id = ?
  → If limit == 0: get_messages_after_id(session_id, through_id)
  → If limit > 0: SELECT role, content FROM messages
                  WHERE session_id = ? AND id > ?
                  ORDER BY id DESC LIMIT ?
                  → reverse to chronological order
  → Return (Option<String>, Vec<(role, content)>)
```

### Message Persistence

```
save_message(session_id, role, content)
  → INSERT INTO messages (session_id, role, content, timestamp)
  → FTS trigger fires automatically

save_tool_exchanges(session_id, exchanges)
  → Serialize JSON array → single row with role "ToolExchanges"

update_user_message_content(session_id, old_content, new_content)
  → UPDATE messages SET content = ? WHERE session_id = ? AND role = 'User' AND content = ?
```

### Profile Fact Management

```
upsert_profile_fact(key, value, confidence)
  → Check existence → determine change_type (insert/update)
  → INSERT INTO profile_history (audit trail)
  → INSERT INTO user_profile ON CONFLICT(key) DO UPDATE
    WHERE excluded.confidence > user_profile.confidence
    (only overwrite when new confidence is strictly higher)

get_immutable_rules()
  → SELECT WHERE key LIKE 'correction:%' AND confidence = 1.0

compact_user_profile()
  → UPDATE user_profile SET is_under_review = 1
    WHERE confidence < 0.3 AND is_under_review = 0
```

### Memory Management

```
save_memories_batch(memories, session_id)
  → BEGIN TRANSACTION
  → For each memory: INSERT INTO memories
  → COMMIT

load_active_memories()
  → SELECT WHERE is_active = 1 ORDER BY updated_at DESC

deactivate_memory(id)
  → UPDATE memories SET is_active = 0, updated_at = now WHERE id = ?
```

### S-DREAM State Tracking

```
get_dream_last_processed(session_id) → last_processed_at from dream_state
set_dream_last_processed(session_id, timestamp) → UPSERT dream_state
get_messages_since(session_id, last_timestamp) → messages with timestamp > last_timestamp
```

### FTS5 Search

```
search_messages(query, session_id?, limit, offset)
  → If session_id provided: JOIN messages_fts + messages, filter by session
  → If not: JOIN messages_fts + messages, no session filter
  → ORDER BY bm25(messages_fts) (lowest rank = best match)
  → Returns SearchResult with snippet (highlighted matching terms)
```

### System Prompts

```
ensure_active_system_prompt(session_id, fallback_content)
  → get_active_system_prompt() → if exists, return
  → SELECT most recent prompt for session_id → if exists, activate it
  → If no prompt: INSERT with fallback_content, activate

activate_system_prompt(id)
  → BEGIN TRANSACTION
  → UPDATE system_prompts SET is_active = 0 (deactivate all)
  → UPDATE system_prompts SET is_active = 1 WHERE id = ?
  → COMMIT
```

## Integration

### Dependencies
- `sqlx` (sqlite feature) — async query execution
- `chrono` — RFC 3339 timestamps
- `uuid` — session UUIDs
- `serde_json` — tool exchange serialization, JSONL export

### Consumers

| Module | Methods Used |
|--------|-------------|
| `src/dream/` (S-DREAM) | `get_or_create_session`, `get_summary_through_id`, `count_messages_after_id`, `get_messages_with_timestamp_after_id`, `get_messages_since`, `get_dream_last_processed`, `set_dream_last_processed`, `load_active_memories`, `save_memories_batch`, `deactivate_memory`, `save_summary`, `upsert_profile_fact`, `compact_user_profile`, `get_session_context` |
| `src/llm/` (session management) | `get_session_context`, `save_message`, `save_tool_exchanges`, `update_user_message_content`, `get_message_id_at_offset`, `get_summary_through_id`, `save_summary`, `is_first_launch`, `get_or_create_session`, `ensure_active_system_prompt` |
| `src/memory/` | `load_active_memories` (via dream module) |
| `src/profile/` | `load_user_profile`, `upsert_profile_fact`, `get_immutable_rules` (via dream module) |
| `src/control/` (API) | `list_sessions_with_active`, `get_messages_with_timestamp_after_id` |
| `src/daemon.rs` | `get_or_create_session`, `is_first_launch` |

### Events/Hooks
- No event system; all access is direct async method calls.
- FTS5 triggers are database-level (fire automatically on DML).
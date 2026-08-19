# S-DREAM — Format & Consolidation Reference

**S**cheduled **DREAM** (S-DREAM) is the cold-path memory consolidation daemon (`src/dream/mod.rs`). It exports conversation history to JSONL archives and periodically distills conversation data into profile facts, memories, and summaries via the LLM provider.

## Scheduling

```rust
pub struct SDreamConfig {
    pub interval_secs: u64,           // fallback interval when no scheduled_hour
    pub on_idle: bool,                // only run when user is idle
    pub idle_threshold_secs: u64,     // idle threshold in seconds
    pub scheduled_hour: Option<u8>,   // run at this hour daily (0–23)
    pub l2_min_messages: usize,       // minimum new messages to trigger cycle
    pub jsonl_dir: String,            // directory for JSONL archives
}
```

### Scheduling Logic
```rust
async fn run_loop(self) {
    loop {
        // 1. Sleep until next scheduled_hour:00, or interval_secs
        // 2. Check gate conditions (idle, message count)
        // 3. If passed: run a consolidation cycle
        // 4. Repeat
    }
}
```

### Cycle Gate
A cycle is skipped if:
1. **Idle check** (`on_idle == true`): user has been active within `idle_threshold_secs`.
2. **Message count**: fewer than `l2_min_messages` new messages since last cycle.

## Consolidation Cycle

Each cycle runs sequentially:

### Stage 1 — Incremental JSONL Export

```
1. Get dream_last_processed timestamp from DB
2. Fetch messages after that timestamp (or after last summary_through_id)
3. Append to dated JSONL file
4. Update dream_last_processed
```

### Stage 2 — Distillation (requires LLM client)

```
1. Assemble conversation text: "role: content\n" for all new messages
2. extract_facts()    → upsert profile facts
3. extract_memories() → save new, archive stale
4. generate_summary() → save summary
```

### Stage 3 — Correction Detection

```
1. Load last 50 messages
2. For each user message: detect_corrections()
3. Save as correction:<topic> facts with confidence 1.0
```

### Stage 4 — Profile Compaction

```
db.compact_user_profile() → delete low-confidence facts
```

## JSONL Export Format

### File naming
```
{jsonl_dir}/2026-07-28.jsonl
```
When the file exceeds 10 MB or 10 000 lines, rotation occurs:
```
{jsonl_dir}/2026-07-28.001.jsonl
{jsonl_dir}/2026-07-28.002.jsonl
```
Old files are never deleted.

### Line format

One JSON object per line:
```jsonl
{"session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890", "timestamp": "2026-07-28T14:30:00Z", "role": "user", "content": "¿Qué tiempo hace hoy?"}
{"session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890", "timestamp": "2026-07-28T14:30:05Z", "role": "assistant", "content": "Hace sol, 25°C."}
```

### Example export code
```rust
async fn export_to_jsonl(
    &self,
    session_id: Uuid,
    messages: &[(i64, String, String, String)],  // (id, role, content, timestamp)
) -> Result<()> {
    let path = self.resolve_jsonl_path(&jsonl_path).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true).append(true).open(&path).await?;

    for (_, role, content, ts) in messages {
        let line = serde_json::json!({
            "session_id": session_id.to_string(),
            "timestamp": ts,
            "role": role,
            "content": content,
        });
        file.write_all(format!("{}\n", line).as_bytes()).await?;
    }
    Ok(())
}
```

## FTS5 Database Schema

S-DREAM uses the SQLite FTS5 engine for full-text search over memories and summaries. Key tables (defined in `src/db/database.rs`):

### Messages table
```sql
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);
```

### Memories table
```sql
CREATE TABLE IF NOT EXISTS memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL,
    content TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    active INTEGER NOT NULL DEFAULT 1,
    session_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### Profile facts table
```sql
CREATE TABLE IF NOT EXISTS profile_facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE,
    value TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### Compaction query

Low-confidence facts are deleted by:
```sql
DELETE FROM profile_facts
WHERE confidence < 0.3
  AND key NOT LIKE 'correction:%'
  AND key NOT IN (SELECT key FROM immutable_rules)
```

### Historical context recovery

```rust
pub fn recover_historical_context(
    db: &Database,
    query: &str,
    limit: usize,
) -> Vec<(String, String, f64)>;
```

Uses FTS5 `MATCH` queries against the memories and profile_facts tables to find relevant past context for the current conversation.

## Summary Generation

```rust
async fn generate_summary(&self, client: &dyn LlmProvider, conversation_text: &str) -> Result<String>;
```

Sends this prompt via `client.complete()`:
```
System: Summarize the following conversation excerpt concisely.
        Capture the key topics discussed, decisions made, and any important context.
        Write in the same language as the conversation.
        Keep it to 2-4 sentences.

User: Conversation:

<conversation_text>
```

The result is saved via `db.save_summary(session_id, summary, max_message_id)`.

## Env Vars

| Variable | Config field | Default | Description |
|----------|-------------|---------|-------------|
| `S_DREAM_INTERVAL_SECS` | `interval_secs` | `3600` | Seconds between cycles (when no `scheduled_hour`) |
| `S_DREAM_ON_IDLE` | `on_idle` | `1` (true) | Only run when user is idle |
| `S_DREAM_IDLE_THRESHOLD_SECS` | `idle_threshold_secs` | `600` | Idle seconds before triggering |
| `S_DREAM_SCHEDULED_HOUR` | `scheduled_hour` | `3` | Daily run hour (0–23); empty = disabled |
| `S_DREAM_L2_MIN_MESSAGES` | `l2_min_messages` | `50` | Minimum new messages to trigger cycle |
| `S_DREAM_JSONL_DIR` | `jsonl_dir` | `data/{env}/archives` | JSONL archive directory |

## Public API

```rust
pub struct SDreamDaemon {
    pub config: SDreamConfig,
    pub db: Database,
    pub client: Option<Arc<dyn LlmProvider>>,
    pub proactive_tx: mpsc::Sender<ProactiveEvent>,
    pub last_activity: Arc<AtomicU64>,
}

impl SDreamDaemon {
    /// Background task: runs the scheduling loop forever.
    pub fn spawn(self) -> tokio::task::JoinHandle<()>;

    /// One-shot run (for --dream CLI mode).
    pub async fn run_once(self) -> Result<()>;
}
```

When `client` is `None`, the JSONL export still runs but distillation (facts, memories, summary) is skipped.

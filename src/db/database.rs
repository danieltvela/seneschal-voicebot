use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use uuid::Uuid;

/// A persistent memory extracted from conversation history.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A new memory to be inserted (no id yet).
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub content: String,
    pub category: String,
}

/// A single full-text search result from the messages FTS5 index.
///
/// `rank` is the BM25 score (lower = better match). `snippet` contains
/// highlighted context around the matching terms.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub rank: f64,
    pub message_id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub snippet: String,
}

/// SQLite database for persistent chat history.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn new(database_path: &str) -> Result<Self> {
        if let Some(parent) = Path::new(database_path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(database_path)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .context("Failed to connect to database")?;

        let db = Self { pool };
        db.run_migrations().await?;
        Ok(db)
    }

    async fn run_migrations(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                closed_at TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                summary TEXT,
                summary_through_id INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await?;

        // Additive migration: add summary columns to existing databases.
        // SQLite does not support IF NOT EXISTS for ADD COLUMN, so we ignore the error.
        let _ = sqlx::query("ALTER TABLE sessions ADD COLUMN summary TEXT")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query(
            "ALTER TABLE sessions ADD COLUMN summary_through_id INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&self.pool)
        .await;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id)")
            .execute(&self.pool)
            .await?;

        // User profile: one row per fact key, updated in place.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_profile (
                key        TEXT PRIMARY KEY,
                value      TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 1.0,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Persistent memories extracted during context consolidation.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memories (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                content           TEXT NOT NULL,
                category          TEXT NOT NULL DEFAULT 'general',
                source_session_id TEXT,
                created_at        TEXT NOT NULL,
                updated_at        TEXT NOT NULL,
                is_active         INTEGER NOT NULL DEFAULT 1
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memories_active ON memories(is_active)")
            .execute(&self.pool)
            .await?;

        // Additive migration: is_under_review column for profile compaction.
        let _ = sqlx::query(
            "ALTER TABLE user_profile ADD COLUMN is_under_review INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&self.pool)
        .await;

        // Profile history table: tracks every change to user_profile for audit trail.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS profile_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                key         TEXT NOT NULL,
                value       TEXT NOT NULL,
                confidence  REAL NOT NULL,
                timestamp   TEXT NOT NULL,
                change_type TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_profile_history_key ON profile_history(key)")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS dream_state (
                session_id         TEXT PRIMARY KEY,
                last_processed_at  TEXT NOT NULL DEFAULT '',
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )",
        )
        .execute(&self.pool)
        .await?;

        // System prompts: one active prompt globally, associated with the
        // session that created it. is_active is enforced in code so that only
        // one row has the value 1 at a time.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS system_prompts (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                content    TEXT NOT NULL,
                is_active  INTEGER NOT NULL DEFAULT 0
                    CHECK (is_active IN (0, 1)),
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_system_prompts_session_id ON system_prompts(session_id)",
        )
        .execute(&self.pool)
        .await?;

        // ── FTS5 full-text search on messages ──────────────────────────────

        // Virtual table for fast keyword search across conversation history.
        // Uses external content mode (content=messages) to avoid duplicating
        // message text — the FTS index stores only tokenized search data.
        sqlx::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                role, content,
                content=messages,
                content_rowid=id,
                tokenize='porter unicode61'
            )",
        )
        .execute(&self.pool)
        .await?;

        // Rebuild FTS index for any pre-existing messages (runs on first
        // migration only; subsequent startups find the virtual table exists
        // and this is a no-op rebuild that re-syncs the token index).
        sqlx::query("INSERT INTO messages_fts(messages_fts) VALUES('rebuild')")
            .execute(&self.pool)
            .await?;

        // ── Synchronisation triggers ──────────────────────────────────────

        // After INSERT: add new message to FTS index.
        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, role, content)
                VALUES (new.id, new.role, new.content);
            END",
        )
        .execute(&self.pool)
        .await?;

        // After DELETE: remove deleted message from FTS index.
        // The two-step sequence ('delete' command + content insert) tells
        // FTS5 to remove the row matching old.id from its token index.
        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts) VALUES('delete');
                INSERT INTO messages_fts(rowid, role, content)
                VALUES (old.id, old.role, old.content);
            END",
        )
        .execute(&self.pool)
        .await?;

        // After UPDATE: remove old entry from FTS index, then insert new.
        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts) VALUES('delete');
                INSERT INTO messages_fts(rowid, role, content)
                VALUES (old.id, old.role, old.content);
                INSERT INTO messages_fts(rowid, role, content)
                VALUES (new.id, new.role, new.content);
            END",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Return the last active session ID, or create a new one.
    pub async fn get_or_create_session(&self) -> Result<Uuid> {
        let row = sqlx::query(
            "SELECT id FROM sessions WHERE is_active = 1 ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            let id: String = row.try_get("id")?;
            let uuid = Uuid::parse_str(&id)?;
            tracing::info!(target: "db", "Restored session {}", uuid);
            return Ok(uuid);
        }

        let id = Uuid::new_v4();
        self.create_session(id).await?;
        tracing::info!(target: "db", "Created new session {}", id);
        Ok(id)
    }

    pub async fn create_session(&self, session_id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO sessions (id, created_at, is_active) VALUES (?, ?, 1)")
            .bind(session_id.to_string())
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Load the session's summary (if any) and the messages after the summary cutoff.
    ///
    /// Returns `(summary_text, recent_messages)`. If no summary exists, all messages
    /// are returned. When `limit > 0`, only the last `limit` messages are returned
    /// (oldest-first), preventing context bloat on restart. Pass `0` for unlimited.
    pub async fn get_session_context(
        &self,
        session_id: Uuid,
        limit: usize,
    ) -> Result<(Option<String>, Vec<(String, String)>)> {
        let row = sqlx::query("SELECT summary, summary_through_id FROM sessions WHERE id = ?")
            .bind(session_id.to_string())
            .fetch_one(&self.pool)
            .await?;

        let summary: Option<String> = row.try_get("summary")?;
        let through_id: i64 = row.try_get("summary_through_id").unwrap_or(0);

        let messages = if limit == 0 {
            self.get_messages_after_id(session_id, through_id).await?
        } else {
            // Load only the last `limit` messages (DESC), then restore chronological order.
            let rows = sqlx::query(
                "SELECT role, content FROM messages
                 WHERE session_id = ? AND id > ?
                 ORDER BY id DESC LIMIT ?",
            )
            .bind(session_id.to_string())
            .bind(through_id)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;

            let mut result: Vec<(String, String)> = rows
                .into_iter()
                .map(|row| {
                    (
                        row.try_get("role").unwrap_or_default(),
                        row.try_get("content").unwrap_or_default(),
                    )
                })
                .collect();
            result.reverse();
            result
        };
        Ok((summary, messages))
    }

    /// Load messages with id > after_id. If after_id is 0, loads all messages.
    pub async fn get_messages_after_id(
        &self,
        session_id: Uuid,
        after_id: i64,
    ) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query(
            "SELECT role, content FROM messages
             WHERE session_id = ? AND id > ?
             ORDER BY id ASC",
        )
        .bind(session_id.to_string())
        .bind(after_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let role: String = row.try_get("role").unwrap_or_default();
                let content: String = row.try_get("content").unwrap_or_default();
                (role, content)
            })
            .collect())
    }

    /// Count messages with id > after_id for a session.
    pub async fn count_messages_after_id(&self, session_id: Uuid, after_id: i64) -> Result<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ? AND id > ?")
                .bind(session_id.to_string())
                .bind(after_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    /// Load messages with id > after_id, including id and timestamp.
    pub async fn get_messages_with_timestamp_after_id(
        &self,
        session_id: Uuid,
        after_id: i64,
    ) -> Result<Vec<(i64, String, String, String)>> {
        let rows = sqlx::query(
            "SELECT id, role, content, timestamp FROM messages
             WHERE session_id = ? AND id > ?
             ORDER BY id ASC",
        )
        .bind(session_id.to_string())
        .bind(after_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let id: i64 = row.try_get("id").unwrap_or(0);
                let role: String = row.try_get("role").unwrap_or_default();
                let content: String = row.try_get("content").unwrap_or_default();
                let timestamp: String = row.try_get("timestamp").unwrap_or_default();
                (id, role, content, timestamp)
            })
            .collect())
    }

    /// Load messages newer than `last_timestamp` for a session.
    ///
    /// Used by the S-DREAM daemon for incremental JSONL export — only
    /// returns messages whose `timestamp` is strictly greater than the
    /// last value already exported.
    pub async fn get_messages_since(
        &self,
        session_id: Uuid,
        last_timestamp: &str,
    ) -> Result<Vec<(i64, String, String, String)>> {
        let rows = sqlx::query(
            "SELECT id, role, content, timestamp FROM messages
             WHERE session_id = ? AND timestamp > ?
             ORDER BY id ASC",
        )
        .bind(session_id.to_string())
        .bind(last_timestamp)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let id: i64 = row.try_get("id").unwrap_or(0);
                let role: String = row.try_get("role").unwrap_or_default();
                let content: String = row.try_get("content").unwrap_or_default();
                let timestamp: String = row.try_get("timestamp").unwrap_or_default();
                (id, role, content, timestamp)
            })
            .collect())
    }

    /// Return the last processed timestamp for a session from the dream_state table.
    ///
    /// Returns an empty string if no record exists yet.
    pub async fn get_dream_last_processed(&self, session_id: Uuid) -> Result<String> {
        let row = sqlx::query("SELECT last_processed_at FROM dream_state WHERE session_id = ?")
            .bind(session_id.to_string())
            .fetch_optional(&self.pool)
            .await?;

        Ok(row
            .map(|r| {
                r.try_get::<String, _>("last_processed_at")
                    .unwrap_or_default()
            })
            .unwrap_or_default())
    }

    /// Update (or insert) the last processed timestamp for a session.
    pub async fn set_dream_last_processed(&self, session_id: Uuid, timestamp: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO dream_state (session_id, last_processed_at)
             VALUES (?, ?)
             ON CONFLICT(session_id) DO UPDATE SET last_processed_at = excluded.last_processed_at",
        )
        .bind(session_id.to_string())
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── System prompts ───────────────────────────────────────────────────────

    /// Return the content of the single globally active system prompt, if any.
    pub async fn get_active_system_prompt(&self) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT content FROM system_prompts WHERE is_active = 1 ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.try_get("content").unwrap_or_default()))
    }

    /// Activate a system prompt, deactivating all others in the same transaction.
    pub async fn activate_system_prompt(&self, prompt_id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE system_prompts SET is_active = 0")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE system_prompts SET is_active = 1 WHERE id = ?")
            .bind(prompt_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Insert a new system prompt row for a session.
    ///
    /// When `active` is true, all other prompts are deactivated first.
    pub async fn insert_system_prompt(
        &self,
        session_id: Uuid,
        content: &str,
        active: bool,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        if active {
            sqlx::query("UPDATE system_prompts SET is_active = 0")
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO system_prompts (session_id, content, is_active, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(session_id.to_string())
        .bind(content)
        .bind(if active { 1 } else { 0 })
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Ensure a system prompt is active for the current session.
    ///
    /// Returns the currently active prompt if one exists. Otherwise, reuses the
    /// most recent prompt stored for `session_id` and activates it. If no prompt
    /// exists for the session, inserts `fallback_content` (typically the value
    /// from `LLM_SYSTEM_PROMPT`) for this session, activates it, and returns it.
    pub async fn ensure_active_system_prompt(
        &self,
        session_id: Uuid,
        fallback_content: &str,
    ) -> Result<String> {
        if let Some(content) = self.get_active_system_prompt().await? {
            return Ok(content);
        }

        let existing = sqlx::query(
            "SELECT id, content FROM system_prompts
             WHERE session_id = ?
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = existing {
            let id: i64 = row.try_get("id")?;
            let content: String = row.try_get("content")?;
            self.activate_system_prompt(id).await?;
            return Ok(content);
        }

        self.insert_system_prompt(session_id, fallback_content, true)
            .await?;
        Ok(fallback_content.to_string())
    }

    /// Return the message id at a 0-based offset within a session (ordered by id ASC),
    /// counting only messages with `id > after_id`.
    ///
    /// Pass `after_id = 0` to count from the beginning.
    /// Pass the current `summary_through_id` to count only within the currently-loaded
    /// batch — this ensures the new cutoff is always strictly ahead of the old one.
    pub async fn get_message_id_at_offset(
        &self,
        session_id: Uuid,
        after_id: i64,
        offset: usize,
    ) -> Result<Option<i64>> {
        let row = sqlx::query(
            "SELECT id FROM messages WHERE session_id = ? AND id > ? ORDER BY id ASC LIMIT 1 OFFSET ?",
        )
        .bind(session_id.to_string())
        .bind(after_id)
        .bind(offset as i64)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.try_get::<i64, _>("id").unwrap_or(0)))
    }

    /// Return the current `summary_through_id` for a session (0 if no summary yet).
    pub async fn get_summary_through_id(&self, session_id: Uuid) -> Result<i64> {
        let row = sqlx::query("SELECT summary_through_id FROM sessions WHERE id = ?")
            .bind(session_id.to_string())
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("summary_through_id").unwrap_or(0))
    }

    /// Persist the conversation summary and the id of the last summarized message.
    ///
    /// On the next startup, only messages with id > through_message_id will be loaded,
    /// and the summary will be injected into the system prompt.
    pub async fn save_summary(
        &self,
        session_id: Uuid,
        summary: &str,
        through_message_id: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE sessions SET summary = ?, summary_through_id = ? WHERE id = ?")
            .bind(summary)
            .bind(through_message_id)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Persist the tool-call exchange messages for a turn (assistant tool_calls + tool results).
    ///
    /// Serialises the full JSON array into a single row with role "ToolExchanges" so that
    /// on next startup the session can reconstruct the exact tool-call context the LLM saw.
    /// Without this, the model only sees the final assistant text response after a tool call
    /// and cannot distinguish correctly-called tools from hallucinated ones.
    pub async fn save_tool_exchanges(
        &self,
        session_id: Uuid,
        exchanges: &[serde_json::Value],
    ) -> Result<()> {
        if exchanges.is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string(exchanges)?;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?, ?, ?, ?)",
        )
        .bind(session_id.to_string())
        .bind("ToolExchanges")
        .bind(json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persist a single message turn.
    pub async fn save_message(&self, session_id: Uuid, role: &str, content: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?, ?, ?, ?)",
        )
        .bind(session_id.to_string())
        .bind(role)
        .bind(content)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Overwrite a previously-inserted user-turn row with new content.
    ///
    /// Used by the LLM task on assistant-commit to reconcile a SQLite row that
    /// was speculatively inserted by a barge-in transcript and has since been
    /// appended to.
    pub async fn update_user_message_content(
        &self,
        session_id: Uuid,
        old_content: &str,
        new_content: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE messages SET content = ? \
             WHERE session_id = ? AND role = 'User' AND content = ?",
        )
        .bind(new_content)
        .bind(session_id.to_string())
        .bind(old_content)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn close_session(&self, session_id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE sessions SET closed_at = ?, is_active = 0 WHERE id = ?")
            .bind(now)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── User profile ──────────────────────────────────────────────────────────

    /// Load all profile facts ordered by key.
    pub async fn load_user_profile(&self) -> Result<Vec<(String, String, f64)>> {
        let rows = sqlx::query("SELECT key, value, confidence FROM user_profile ORDER BY key ASC")
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let key: String = r.try_get("key").unwrap_or_default();
                let value: String = r.try_get("value").unwrap_or_default();
                let confidence: f64 = r.try_get("confidence").unwrap_or(1.0);
                (key, value, confidence)
            })
            .collect())
    }

    pub async fn get_immutable_rules(&self) -> Result<Vec<(String, String, f64)>> {
        let rows = sqlx::query(
            "SELECT key, value, confidence FROM user_profile
             WHERE key LIKE 'correction:%' AND confidence = 1.0
             ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let key: String = r.try_get("key").unwrap_or_default();
                let value: String = r.try_get("value").unwrap_or_default();
                let confidence: f64 = r.try_get("confidence").unwrap_or(1.0);
                (key, value, confidence)
            })
            .collect())
    }

    /// Insert or update a profile fact.
    ///
    /// An existing fact is only overwritten when the new confidence is strictly
    /// higher — this prevents low-quality inferences from degrading confirmed facts.
    /// Each call also appends an entry to `profile_history` for audit trail,
    /// recording whether this was an insert or an update.
    #[allow(dead_code)]
    pub async fn upsert_profile_fact(&self, key: &str, value: &str, confidence: f64) -> Result<()> {
        let now = Utc::now().to_rfc3339();

        let exists: bool =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user_profile WHERE key = ?")
                .bind(key)
                .fetch_one(&self.pool)
                .await?
                > 0;

        let change_type = if exists { "update" } else { "insert" };

        sqlx::query(
            "INSERT INTO profile_history (key, value, confidence, timestamp, change_type)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(key)
        .bind(value)
        .bind(confidence)
        .bind(&now)
        .bind(change_type)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "INSERT INTO user_profile (key, value, confidence, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET
                 value      = excluded.value,
                 confidence = excluded.confidence,
                 updated_at = excluded.updated_at
             WHERE excluded.confidence > user_profile.confidence",
        )
        .bind(key)
        .bind(value)
        .bind(confidence)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return recent history for a profile key (or all keys if key is None).
    #[allow(dead_code)]
    pub async fn get_profile_history(
        &self,
        key: Option<&str>,
        limit: i64,
    ) -> Result<Vec<(String, String, f64, String, String)>> {
        let rows = if let Some(k) = key {
            sqlx::query(
                "SELECT key, value, confidence, timestamp, change_type
                 FROM profile_history WHERE key = ?
                 ORDER BY timestamp DESC LIMIT ?",
            )
            .bind(k)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT key, value, confidence, timestamp, change_type
                 FROM profile_history
                 ORDER BY timestamp DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.try_get("key").unwrap_or_default(),
                    r.try_get("value").unwrap_or_default(),
                    r.try_get("confidence").unwrap_or(1.0),
                    r.try_get("timestamp").unwrap_or_default(),
                    r.try_get("change_type").unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Mark profile facts with confidence < 0.3 for human review.
    /// Returns the number of facts newly marked.
    #[allow(dead_code)]
    pub async fn compact_user_profile(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE user_profile
             SET is_under_review = 1
             WHERE confidence < 0.3 AND is_under_review = 0",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Return all profile facts currently flagged for human review.
    #[allow(dead_code)]
    pub async fn get_profile_facts_under_review(&self) -> Result<Vec<(String, String, f64)>> {
        let rows = sqlx::query(
            "SELECT key, value, confidence
             FROM user_profile
             WHERE is_under_review = 1
             ORDER BY key ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.try_get("key").unwrap_or_default(),
                    r.try_get("value").unwrap_or_default(),
                    r.try_get("confidence").unwrap_or(1.0),
                )
            })
            .collect())
    }

    #[allow(dead_code)]
    pub async fn list_sessions(&self) -> Result<Vec<(Uuid, DateTime<Utc>)>> {
        let rows = sqlx::query("SELECT id, created_at FROM sessions ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await?;

        let mut sessions = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let created_at: String = row.try_get("created_at")?;
            let uuid = Uuid::parse_str(&id)?;
            let timestamp = DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc);
            sessions.push((uuid, timestamp));
        }
        Ok(sessions)
    }

    /// List sessions with active status (for API history endpoints).
    pub async fn list_sessions_with_active(&self) -> Result<Vec<(String, String, bool)>> {
        let rows =
            sqlx::query("SELECT id, created_at, is_active FROM sessions ORDER BY created_at DESC")
                .fetch_all(&self.pool)
                .await?;

        let mut sessions = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let created_at: String = row.try_get("created_at")?;
            let is_active: bool = row.try_get("is_active")?;
            sessions.push((id, created_at, is_active));
        }
        Ok(sessions)
    }

    // ── Memories ──────────────────────────────────────────────────────────────

    /// Load all active memories, most recently updated first.
    pub async fn load_active_memories(&self) -> Result<Vec<Memory>> {
        let rows = sqlx::query(
            "SELECT id, content, category, source_session_id, created_at, updated_at
             FROM memories WHERE is_active = 1 ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| Memory {
                id: r.try_get("id").unwrap_or(0),
                content: r.try_get("content").unwrap_or_default(),
                category: r.try_get("category").unwrap_or_default(),
                source_session_id: r.try_get("source_session_id").ok(),
                created_at: r.try_get("created_at").unwrap_or_default(),
                updated_at: r.try_get("updated_at").unwrap_or_default(),
            })
            .collect())
    }

    /// Insert multiple memories in a single transaction.
    pub async fn save_memories_batch(
        &self,
        memories: &[NewMemory],
        session_id: Uuid,
    ) -> Result<()> {
        if memories.is_empty() {
            return Ok(());
        }
        let now = Utc::now().to_rfc3339();
        let sid = session_id.to_string();

        let mut tx = self.pool.begin().await?;
        for mem in memories {
            sqlx::query(
                "INSERT INTO memories (content, category, source_session_id, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&mem.content)
            .bind(&mem.category)
            .bind(&sid)
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Soft-delete a memory by setting is_active = 0.
    pub async fn deactivate_memory(&self, memory_id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE memories SET is_active = 0, updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(memory_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── FTS5 search & export ───────────────────────────────────────────────

    /// Search messages using FTS5 full-text search.
    ///
    /// Returns results ranked by BM25 relevance (lowest rank = best match).
    /// Each result includes a `snippet` with highlighted matching terms.
    ///
    /// Optionally filter by `session_id` to scope the search to a single
    /// conversation. Use `limit` and `offset` for pagination.
    pub async fn search_messages(
        &self,
        query: &str,
        session_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchResult>> {
        let rows = match session_id {
            Some(sid) => {
                sqlx::query(
                    "SELECT m.id, m.session_id, m.role, m.content, m.timestamp,
                            bm25(messages_fts) AS rank,
                            snippet(messages_fts, 1, '<b>', '</b>', '…', 32) AS snippet
                     FROM messages_fts
                     JOIN messages m ON messages_fts.rowid = m.id
                     WHERE messages_fts MATCH ?
                       AND m.session_id = ?
                     ORDER BY rank
                     LIMIT ? OFFSET ?",
                )
                .bind(query)
                .bind(sid.to_string())
                .bind(limit as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT m.id, m.session_id, m.role, m.content, m.timestamp,
                            bm25(messages_fts) AS rank,
                            snippet(messages_fts, 1, '<b>', '</b>', '…', 32) AS snippet
                     FROM messages_fts
                     JOIN messages m ON messages_fts.rowid = m.id
                     WHERE messages_fts MATCH ?
                     ORDER BY rank
                     LIMIT ? OFFSET ?",
                )
                .bind(query)
                .bind(limit as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows
            .into_iter()
            .map(|r| SearchResult {
                rank: r.try_get("rank").unwrap_or(f64::MAX),
                message_id: r.try_get("id").unwrap_or(0),
                session_id: r.try_get("session_id").unwrap_or_default(),
                role: r.try_get("role").unwrap_or_default(),
                content: r.try_get("content").unwrap_or_default(),
                timestamp: r.try_get("timestamp").unwrap_or_default(),
                snippet: r.try_get("snippet").unwrap_or_default(),
            })
            .collect())
    }

    /// Export all messages for a session as JSONL (one JSON object per line).
    ///
    /// Each line has fields: `timestamp`, `role`, `content`.
    /// Messages are ordered oldest-first by `id`.
    pub async fn export_session_jsonl(&self, session_id: Uuid) -> Result<String> {
        let rows = sqlx::query(
            "SELECT role, content, timestamp FROM messages
             WHERE session_id = ? ORDER BY id ASC",
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let mut lines: Vec<String> = Vec::with_capacity(rows.len());
        for row in rows {
            let entry = serde_json::json!({
                "timestamp": row.try_get::<String, _>("timestamp").unwrap_or_default(),
                "role": row.try_get::<String, _>("role").unwrap_or_default(),
                "content": row.try_get::<String, _>("content").unwrap_or_default(),
            });
            lines.push(entry.to_string());
        }
        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_db() -> (Database, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap()).await.unwrap();
        (db, dir)
    }

    #[tokio::test]
    async fn new_creates_file_and_migrates() {
        let (_db, dir) = fresh_db().await;
        let p = dir.path().join("test.db");
        assert!(p.exists(), "Database file must be created on disk");
        assert!(p.metadata().unwrap().len() > 0, "DB must not be empty");
    }

    #[tokio::test]
    async fn get_or_create_session_creates_then_reuses() {
        let (db, _d) = fresh_db().await;
        let a = db.get_or_create_session().await.unwrap();
        let b = db.get_or_create_session().await.unwrap();
        assert_eq!(a, b, "Second call must return the same active session");
    }

    #[tokio::test]
    async fn close_session_deactivates_and_new_session_is_created() {
        let (db, _d) = fresh_db().await;
        let a = db.get_or_create_session().await.unwrap();
        db.close_session(a).await.unwrap();
        let b = db.get_or_create_session().await.unwrap();
        assert_ne!(a, b, "After close, a new active session is created");
    }

    #[tokio::test]
    async fn save_and_load_messages_preserve_order() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "hola").await.unwrap();
        db.save_message(sid, "assistant", "buenos dias")
            .await
            .unwrap();
        db.save_message(sid, "user", "que hora es").await.unwrap();

        let (summary, msgs) = db.get_session_context(sid, 0).await.unwrap();
        assert!(summary.is_none());
        assert_eq!(
            msgs,
            vec![
                ("user".to_string(), "hola".to_string()),
                ("assistant".to_string(), "buenos dias".to_string()),
                ("user".to_string(), "que hora es".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn get_session_context_limit_returns_recent_chronological() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        for i in 0..5 {
            db.save_message(sid, "user", &format!("msg {i}"))
                .await
                .unwrap();
        }
        let (_summary, msgs) = db.get_session_context(sid, 2).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].1, "msg 3");
        assert_eq!(msgs[1].1, "msg 4");
    }

    #[tokio::test]
    async fn save_summary_marks_cutoff_and_subsequent_load_uses_it() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "uno").await.unwrap();
        db.save_message(sid, "assistant", "dos").await.unwrap();
        db.save_message(sid, "user", "tres").await.unwrap();
        let cutoff = db
            .get_message_id_at_offset(sid, 0, 1)
            .await
            .unwrap()
            .unwrap();
        db.save_summary(sid, "user said hola once", cutoff)
            .await
            .unwrap();

        assert_eq!(db.get_summary_through_id(sid).await.unwrap(), cutoff);
        let (summary, msgs) = db.get_session_context(sid, 0).await.unwrap();
        assert_eq!(summary.as_deref(), Some("user said hola once"));
        assert_eq!(msgs.len(), 1, "Only messages with id > cutoff load");
        assert_eq!(msgs[0].1, "tres");
    }

    #[tokio::test]
    async fn get_message_id_at_offset_out_of_range_returns_none() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "uno").await.unwrap();
        let id = db.get_message_id_at_offset(sid, 0, 99).await.unwrap();
        assert!(id.is_none());
    }

    #[tokio::test]
    async fn save_tool_exchanges_round_trip() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        let exchanges = vec![
            serde_json::json!({"role":"assistant","tool_calls":[{"id":"1","name":"time","args":{}}]}),
            serde_json::json!({"role":"tool","content":"12:00"}),
        ];
        db.save_tool_exchanges(sid, &exchanges).await.unwrap();
        let (_summary, msgs) = db.get_session_context(sid, 0).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, "ToolExchanges");
        let parsed: serde_json::Value = serde_json::from_str(&msgs[0].1).unwrap();
        assert_eq!(parsed, serde_json::json!(exchanges));
    }

    #[tokio::test]
    async fn upsert_profile_fact_inserts_then_updates_in_place() {
        let (db, _d) = fresh_db().await;
        db.upsert_profile_fact("name", "Daniel", 0.9).await.unwrap();
        let p1 = db.load_user_profile().await.unwrap();
        assert_eq!(p1, vec![("name".to_string(), "Daniel".to_string(), 0.9)]);

        db.upsert_profile_fact("name", "Daniel V.", 1.0)
            .await
            .unwrap();
        let p2 = db.load_user_profile().await.unwrap();
        assert_eq!(p2.len(), 1, "Upsert must not insert a duplicate row");
        assert_eq!(p2[0].1, "Daniel V.");
        assert!((p2[0].2 - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn list_sessions_returns_all_newest_first() {
        let (db, _d) = fresh_db().await;
        let s1 = db.get_or_create_session().await.unwrap();
        db.close_session(s1).await.unwrap();
        let s2 = db.get_or_create_session().await.unwrap();
        let list = db.list_sessions().await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[0].0, s2,
            "Newest session comes first (ORDER BY created_at DESC)"
        );
        assert_eq!(list[1].0, s1);
    }

    #[tokio::test]
    async fn save_memories_batch_persists_all_with_is_active_one() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        let items = vec![
            NewMemory {
                content: "prefiero español".to_string(),
                category: "language".to_string(),
            },
            NewMemory {
                content: "trabajo en IA".to_string(),
                category: "work".to_string(),
            },
        ];
        db.save_memories_batch(&items, sid).await.unwrap();
        let active = db.load_active_memories().await.unwrap();
        assert_eq!(active.len(), 2);
        let cats: Vec<_> = active.iter().map(|m| m.category.as_str()).collect();
        assert!(cats.contains(&"language"));
        assert!(cats.contains(&"work"));
    }

    #[tokio::test]
    async fn deactivate_memory_soft_deletes_from_active_list() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        let items = vec![NewMemory {
            content: "obsoleto".to_string(),
            category: "general".to_string(),
        }];
        db.save_memories_batch(&items, sid).await.unwrap();
        let m = &db.load_active_memories().await.unwrap()[0];
        db.deactivate_memory(m.id).await.unwrap();
        assert!(db.load_active_memories().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn upsert_profile_fact_records_history() {
        let (db, _d) = fresh_db().await;

        // First insert.
        db.upsert_profile_fact("city", "Madrid", 0.8).await.unwrap();
        let hist = db.get_profile_history(Some("city"), 10).await.unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].4, "insert");

        db.upsert_profile_fact("city", "Barcelona", 0.9)
            .await
            .unwrap();
        let hist = db.get_profile_history(Some("city"), 10).await.unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].4, "update");
        assert_eq!(hist[0].1, "Barcelona");

        db.upsert_profile_fact("city", "Seville", 0.2)
            .await
            .unwrap();
        let hist = db.get_profile_history(Some("city"), 10).await.unwrap();
        assert_eq!(hist.len(), 3);
        assert_eq!(hist[0].1, "Seville");
        assert_eq!(hist[0].4, "update");
    }

    #[tokio::test]
    async fn get_profile_history_none_key_returns_all() {
        let (db, _d) = fresh_db().await;
        db.upsert_profile_fact("color", "azul", 0.7).await.unwrap();
        db.upsert_profile_fact("age", "30", 0.9).await.unwrap();
        let hist = db.get_profile_history(None, 10).await.unwrap();
        assert_eq!(hist.len(), 2);
    }

    #[tokio::test]
    async fn compact_user_profile_marks_low_confidence_facts() {
        let (db, _d) = fresh_db().await;
        db.upsert_profile_fact("high", "confirmed", 0.9)
            .await
            .unwrap();
        db.upsert_profile_fact("low", "doubtful", 0.2)
            .await
            .unwrap();

        let marked = db.compact_user_profile().await.unwrap();
        assert_eq!(marked, 1, "Only the low-confidence fact should be marked");

        let under_review = db.get_profile_facts_under_review().await.unwrap();
        assert_eq!(under_review.len(), 1);
        assert_eq!(under_review[0].0, "low");
    }

    #[tokio::test]
    async fn compact_user_profile_idempotent() {
        let (db, _d) = fresh_db().await;
        db.upsert_profile_fact("low", "doubtful", 0.2)
            .await
            .unwrap();
        db.compact_user_profile().await.unwrap();
        let second = db.compact_user_profile().await.unwrap();
        assert_eq!(
            second, 0,
            "Second compaction must not mark already-marked facts"
        );
    }

    #[tokio::test]
    async fn get_profile_facts_under_review_empty_when_none_marked() {
        let (db, _d) = fresh_db().await;
        db.upsert_profile_fact("high", "confirmed", 0.9)
            .await
            .unwrap();
        let under_review = db.get_profile_facts_under_review().await.unwrap();
        assert!(under_review.is_empty());
    }

    // ── System prompts ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn ensure_active_system_prompt_inserts_fallback_when_empty() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        let fallback = "fallback prompt";
        let content = db.ensure_active_system_prompt(sid, fallback).await.unwrap();
        assert_eq!(content, fallback);
        assert_eq!(
            db.get_active_system_prompt().await.unwrap().as_deref(),
            Some(fallback)
        );
    }

    #[tokio::test]
    async fn ensure_active_system_prompt_reuses_existing_active() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.insert_system_prompt(sid, "stored prompt", true)
            .await
            .unwrap();

        let content = db
            .ensure_active_system_prompt(sid, "different fallback")
            .await
            .unwrap();
        assert_eq!(content, "stored prompt");
    }

    #[tokio::test]
    async fn ensure_active_system_prompt_activates_existing_for_session() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.insert_system_prompt(sid, "inactive prompt", false)
            .await
            .unwrap();

        let content = db
            .ensure_active_system_prompt(sid, "fallback")
            .await
            .unwrap();
        assert_eq!(content, "inactive prompt");
        assert_eq!(
            db.get_active_system_prompt().await.unwrap().as_deref(),
            Some("inactive prompt")
        );
    }

    #[tokio::test]
    async fn insert_system_prompt_activating_deactivates_others() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.insert_system_prompt(sid, "first", true).await.unwrap();
        db.insert_system_prompt(sid, "second", true).await.unwrap();

        let rows: Vec<(i64, i64)> =
            sqlx::query_as("SELECT is_active, COUNT(*) FROM system_prompts GROUP BY is_active")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|(active, count)| *active == 0 && *count == 1)
        );
        assert!(
            rows.iter()
                .any(|(active, count)| *active == 1 && *count == 1)
        );
        assert_eq!(
            db.get_active_system_prompt().await.unwrap().as_deref(),
            Some("second")
        );
    }

    #[tokio::test]
    async fn activate_system_prompt_switches_active_flag() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.insert_system_prompt(sid, "first", true).await.unwrap();
        db.insert_system_prompt(sid, "second", false).await.unwrap();

        let inactive_id: i64 =
            sqlx::query_scalar("SELECT id FROM system_prompts WHERE content = ?")
                .bind("second")
                .fetch_one(&db.pool)
                .await
                .unwrap();

        db.activate_system_prompt(inactive_id).await.unwrap();
        assert_eq!(
            db.get_active_system_prompt().await.unwrap().as_deref(),
            Some("second")
        );
    }

    // ── FTS5 search ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_messages_returns_ranked_results() {
        let (db, _d) = fresh_db().await;
        let sid1 = db.get_or_create_session().await.unwrap();
        let sid2 = Uuid::new_v4();
        db.create_session(sid2).await.unwrap();

        // Insert messages across two sessions.
        db.save_message(sid1, "user", "hola que tal").await.unwrap();
        db.save_message(sid1, "assistant", "muy bien gracias")
            .await
            .unwrap();
        db.save_message(sid1, "user", "que hora es en madrid")
            .await
            .unwrap();
        db.save_message(sid2, "user", "madrid es una ciudad bonita")
            .await
            .unwrap();
        db.save_message(sid1, "assistant", "son las tres en madrid")
            .await
            .unwrap();

        // Search across all sessions.
        let results = db.search_messages("madrid", None, 10, 0).await.unwrap();

        assert!(!results.is_empty(), "FTS5 must find matches for 'madrid'");
        assert!(
            results[0].rank <= results[1].rank,
            "Results must be sorted by BM25 rank (ascending)"
        );
        for r in &results {
            assert!(
                r.content.contains("madrid"),
                "Each result content must contain the query term"
            );
            assert!(!r.snippet.is_empty(), "Snippet must not be empty");
        }
    }

    #[tokio::test]
    async fn search_messages_filters_by_session() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        let other = Uuid::new_v4();
        db.create_session(other).await.unwrap();

        db.save_message(sid, "user", "hola que tal").await.unwrap();
        db.save_message(other, "user", "madrid es bonita")
            .await
            .unwrap();

        let results = db
            .search_messages("madrid", Some(sid), 10, 0)
            .await
            .unwrap();
        assert!(
            results.is_empty(),
            "Search scoped to session without 'madrid' should be empty"
        );

        let results = db
            .search_messages("madrid", Some(other), 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role, "user");
    }

    #[tokio::test]
    async fn search_messages_pagination_works() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();

        for i in 0..10 {
            db.save_message(sid, "user", &format!("palabra clave numero {i}"))
                .await
                .unwrap();
        }

        // Search with limit and offset.
        let page1 = db.search_messages("clave", None, 3, 0).await.unwrap();
        assert_eq!(page1.len(), 3);

        let page2 = db.search_messages("clave", None, 3, 3).await.unwrap();
        assert_eq!(page2.len(), 3);

        // Pages should be different.
        let ids1: Vec<i64> = page1.iter().map(|r| r.message_id).collect();
        let ids2: Vec<i64> = page2.iter().map(|r| r.message_id).collect();
        assert!(
            ids1.iter().all(|id| !ids2.contains(id)),
            "Pages must not overlap"
        );
    }

    #[tokio::test]
    async fn search_messages_empty_query_returns_empty() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "hola").await.unwrap();

        // Empty or non-matching query.
        let results = db
            .search_messages("xyznonexistent", None, 10, 0)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    // ── JSONL export ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_immutable_rules_filters_by_prefix_and_confidence() {
        let (db, _d) = fresh_db().await;

        db.upsert_profile_fact("correction:name", "Daniel", 1.0)
            .await
            .unwrap();
        db.upsert_profile_fact("correction:city", "Madrid", 1.0)
            .await
            .unwrap();
        db.upsert_profile_fact("name", "David", 0.9).await.unwrap();
        db.upsert_profile_fact("correction:age", "30", 0.8)
            .await
            .unwrap();

        let rules = db.get_immutable_rules().await.unwrap();
        assert_eq!(rules.len(), 2);
        assert!(
            rules
                .iter()
                .any(|(k, v, _)| k == "correction:name" && v == "Daniel")
        );
        assert!(
            rules
                .iter()
                .any(|(k, v, _)| k == "correction:city" && v == "Madrid")
        );
        assert!(
            !rules.iter().any(|(k, _, _)| k == "name"),
            "regular profile facts must be excluded"
        );
        assert!(
            !rules.iter().any(|(k, _, _)| k == "correction:age"),
            "low-confidence corrections must be excluded"
        );
    }

    #[tokio::test]
    async fn get_immutable_rules_empty_when_none() {
        let (db, _d) = fresh_db().await;
        db.upsert_profile_fact("name", "Daniel", 0.9).await.unwrap();
        let rules = db.get_immutable_rules().await.unwrap();
        assert!(rules.is_empty());
    }

    #[tokio::test]
    async fn export_session_jsonl_format_and_content() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "hola").await.unwrap();
        db.save_message(sid, "assistant", "buenos dias")
            .await
            .unwrap();

        let jsonl = db.export_session_jsonl(sid).await.unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2, "JSONL must have one line per message");

        // Parse first line and verify structure.
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["role"], "user");
        assert_eq!(first["content"], "hola");
        assert!(first["timestamp"].is_string());
        assert!(!first["timestamp"].as_str().unwrap().is_empty());

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["role"], "assistant");
        assert_eq!(second["content"], "buenos dias");
    }

    #[tokio::test]
    async fn export_session_jsonl_empty_session() {
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();

        let jsonl = db.export_session_jsonl(sid).await.unwrap();
        assert!(jsonl.is_empty(), "Empty session must produce empty JSONL");
    }

    #[tokio::test]
    async fn export_session_jsonl_embedding_in_sync_via_triggers() {
        // Verify the FTS5 trigger fires on INSERT by searching for content
        // that was inserted via save_message.
        let (db, _d) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();

        db.save_message(sid, "user", "busca esta frase unica")
            .await
            .unwrap();

        let results = db
            .search_messages("frase unica", None, 10, 0)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "FTS5 trigger must index newly inserted messages"
        );
    }
}

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

    /// Insert or update a profile fact.
    ///
    /// An existing fact is only overwritten when the new confidence is strictly
    /// higher — this prevents low-quality inferences from degrading confirmed facts.
    #[allow(dead_code)]
    pub async fn upsert_profile_fact(&self, key: &str, value: &str, confidence: f64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
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
}

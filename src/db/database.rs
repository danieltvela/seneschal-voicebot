use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::path::Path;
use uuid::Uuid;

use crate::session::context::Message;

/// SQLite database manager for persistent storage
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Create a new database connection
    pub async fn new(database_path: &str) -> Result<Self> {
        // Ensure parent directory exists
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

    /// Run database migrations
    async fn run_migrations(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                closed_at TEXT,
                is_active INTEGER NOT NULL DEFAULT 1
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                content_type TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_messages_session_id 
            ON messages(session_id)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS config (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Create a new session
    pub async fn create_session(&self, session_id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO sessions (id, created_at, is_active)
            VALUES (?, ?, 1)
            "#,
        )
        .bind(session_id.to_string())
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Save a message to the database
    pub async fn save_message(&self, session_id: Uuid, message: &Message) -> Result<()> {
        let role = format!("{:?}", message.role);
        let (content, content_type) = match &message.content {
            crate::session::context::MessageContent::Text(text) => (text.clone(), "text"),
            crate::session::context::MessageContent::Audio(_) => ("".to_string(), "audio"),
            crate::session::context::MessageContent::ToolCall { name, args } => {
                (format!("{}:{}", name, args), "tool_call")
            }
            crate::session::context::MessageContent::ToolResult { name, result } => {
                (format!("{}:{}", name, result), "tool_result")
            }
        };

        sqlx::query(
            r#"
            INSERT INTO messages (session_id, role, content, content_type, timestamp)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id.to_string())
        .bind(role)
        .bind(content)
        .bind(content_type)
        .bind(message.timestamp.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get all messages for a session
    pub async fn get_session_messages(&self, session_id: Uuid) -> Result<Vec<Message>> {
        let rows = sqlx::query(
            r#"
            SELECT role, content, content_type, timestamp
            FROM messages
            WHERE session_id = ?
            ORDER BY timestamp ASC
            "#,
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let mut messages = Vec::new();
        for row in rows {
            let role: String = row.try_get("role")?;
            let content: String = row.try_get("content")?;
            let content_type: String = row.try_get("content_type")?;
            let timestamp: String = row.try_get("timestamp")?;

            let role = match role.as_str() {
                "User" => crate::session::context::MessageRole::User,
                "Assistant" => crate::session::context::MessageRole::Assistant,
                "System" => crate::session::context::MessageRole::System,
                "Tool" => crate::session::context::MessageRole::Tool,
                _ => continue,
            };

            let content = match content_type.as_str() {
                "text" => crate::session::context::MessageContent::Text(content),
                "audio" => crate::session::context::MessageContent::Audio(Vec::new()),
                "tool_call" => {
                    let parts: Vec<&str> = content.splitn(2, ':').collect();
                    crate::session::context::MessageContent::ToolCall {
                        name: parts[0].to_string(),
                        args: parts.get(1).unwrap_or(&"").to_string(),
                    }
                }
                "tool_result" => {
                    let parts: Vec<&str> = content.splitn(2, ':').collect();
                    crate::session::context::MessageContent::ToolResult {
                        name: parts[0].to_string(),
                        result: parts.get(1).unwrap_or(&"").to_string(),
                    }
                }
                _ => continue,
            };

            let timestamp = DateTime::parse_from_rfc3339(&timestamp)?.with_timezone(&Utc);

            messages.push(Message {
                role,
                content,
                timestamp,
            });
        }

        Ok(messages)
    }

    /// Close a session
    pub async fn close_session(&self, session_id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE sessions
            SET closed_at = ?, is_active = 0
            WHERE id = ?
            "#,
        )
        .bind(now)
        .bind(session_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// List all sessions
    pub async fn list_sessions(&self) -> Result<Vec<(Uuid, DateTime<Utc>)>> {
        let rows = sqlx::query(
            r#"
            SELECT id, created_at
            FROM sessions
            ORDER BY created_at DESC
            "#,
        )
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

    /// Save configuration value
    pub async fn save_config(&self, key: &str, value: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO config (key, value, updated_at)
            VALUES (?, ?, ?)
            ON CONFLICT(key) DO UPDATE SET value = ?, updated_at = ?
            "#,
        )
        .bind(key)
        .bind(value)
        .bind(&now)
        .bind(value)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get configuration value
    pub async fn get_config(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query(
            r#"
            SELECT value FROM config WHERE key = ?
            "#,
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.try_get("value").unwrap()))
    }
}

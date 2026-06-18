use sqlx::SqlitePool;

pub struct AppState {
    pub pool: SqlitePool,
}

impl AppState {
    pub async fn new(db_path: &str) -> anyhow::Result<Self> {
        let connection_str = format!("sqlite://{db_path}");
        let pool = SqlitePool::connect(&connection_str).await?;
        Ok(Self { pool })
    }

    pub async fn list_sessions(
        &self,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<super::models::Session>> {
        let sessions = sqlx::query_as::<_, super::models::Session>(
            "SELECT id, created_at, closed_at, is_active, summary, summary_through_id \
             FROM sessions \
             ORDER BY created_at DESC \
             LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions)
    }

    pub async fn get_session(&self, id: &str) -> anyhow::Result<Option<super::models::Session>> {
        let session = sqlx::query_as::<_, super::models::Session>(
            "SELECT id, created_at, closed_at, is_active, summary, summary_through_id \
             FROM sessions \
             WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(session)
    }

    pub async fn count_sessions(&self) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn list_messages_for_session(
        &self,
        session_id: &str,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<super::models::Message>> {
        let rows: Vec<MessageFromRow> = sqlx::query_as::<_, MessageFromRow>(
            "SELECT id, session_id, role, content, timestamp \
             FROM messages \
             WHERE session_id = ? \
             ORDER BY timestamp ASC, id ASC \
             LIMIT ? OFFSET ?",
        )
        .bind(session_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_all_messages(
        &self,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<super::models::Message>> {
        let rows: Vec<MessageFromRow> = sqlx::query_as::<_, MessageFromRow>(
            "SELECT id, session_id, role, content, timestamp \
             FROM messages \
             ORDER BY timestamp DESC, id DESC \
             LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn count_messages_for_session(&self, session_id: &str) -> anyhow::Result<i64> {
        let count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    pub async fn count_all_messages(&self) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn get_message(&self, id: i64) -> anyhow::Result<Option<super::models::Message>> {
        let row: Option<MessageFromRow> = sqlx::query_as::<_, MessageFromRow>(
            "SELECT id, session_id, role, content, timestamp \
             FROM messages \
             WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn search_messages(
        &self,
        query: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<super::models::Message>> {
        let rows: Vec<MessageFromRow> = sqlx::query_as::<_, MessageFromRow>(
            "SELECT m.id, m.session_id, m.role, m.content, m.timestamp \
             FROM messages m \
             INNER JOIN messages_fts fts ON m.id = fts.rowid \
             WHERE messages_fts MATCH ? \
             ORDER BY rank \
             LIMIT ?",
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_profile(&self) -> anyhow::Result<Vec<super::models::UserProfileEntry>> {
        let entries = sqlx::query_as::<_, super::models::UserProfileEntry>(
            "SELECT key, value, confidence, updated_at, is_under_review \
             FROM user_profile \
             ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(entries)
    }

    pub async fn count_profile(&self) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user_profile")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn list_memories(
        &self,
        active_only: Option<bool>,
    ) -> anyhow::Result<Vec<super::models::Memory>> {
        let memories = match active_only {
            Some(true) => {
                sqlx::query_as::<_, super::models::Memory>(
                    "SELECT id, content, category, source_session_id, created_at, updated_at, is_active \
                     FROM memories \
                     WHERE is_active = 1 \
                     ORDER BY updated_at DESC",
                )
                .fetch_all(&self.pool)
                .await?
            }
            Some(false) => {
                sqlx::query_as::<_, super::models::Memory>(
                    "SELECT id, content, category, source_session_id, created_at, updated_at, is_active \
                     FROM memories \
                     WHERE is_active = 0 \
                     ORDER BY updated_at DESC",
                )
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, super::models::Memory>(
                    "SELECT id, content, category, source_session_id, created_at, updated_at, is_active \
                     FROM memories \
                     ORDER BY updated_at DESC",
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(memories)
    }

    pub async fn count_memories(&self) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM memories")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn list_profile_history(
        &self,
    ) -> anyhow::Result<Vec<super::models::ProfileHistoryEntry>> {
        let entries = sqlx::query_as::<_, super::models::ProfileHistoryEntry>(
            "SELECT id, key, value, confidence, timestamp, change_type \
             FROM profile_history \
             ORDER BY timestamp DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(entries)
    }

    pub async fn list_dream_state(&self) -> anyhow::Result<Vec<super::models::DreamState>> {
        let states = sqlx::query_as::<_, super::models::DreamState>(
            "SELECT session_id, last_processed_at \
             FROM dream_state \
             ORDER BY last_processed_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(states)
    }

    pub async fn list_system_prompts(&self) -> anyhow::Result<Vec<super::models::SystemPrompt>> {
        let prompts = sqlx::query_as::<_, super::models::SystemPrompt>(
            "SELECT id, session_id, content, is_active, created_at \
             FROM system_prompts \
             ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(prompts)
    }

    pub async fn count_system_prompts(&self) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM system_prompts")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn create_system_prompt(
        &self,
        session_id: &str,
        content: &str,
        active: bool,
    ) -> anyhow::Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        if active {
            sqlx::query("UPDATE system_prompts SET is_active = 0")
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO system_prompts (session_id, content, is_active, created_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(content)
        .bind(if active { 1 } else { 0 })
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn delete_system_prompt(&self, id: i64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM system_prompts WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn activate_system_prompt(&self, id: i64) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE system_prompts SET is_active = 0")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE system_prompts SET is_active = 1 WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM dream_state WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_message(&self, message_id: i64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM messages WHERE id = ?")
            .bind(message_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct MessageFromRow {
    id: i64,
    session_id: String,
    role: String,
    content: String,
    timestamp: String,
}

impl From<MessageFromRow> for super::models::Message {
    fn from(row: MessageFromRow) -> Self {
        Self {
            id: row.id,
            session_id: row.session_id,
            role: row.role.as_str().into(),
            content: row.content,
            timestamp: row.timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    async fn fixture_state() -> AppState {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/voicebot.db");
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("voicebot.db");
        std::fs::copy(&fixture, &db_path).unwrap();
        // Leak the temp dir so the database file remains accessible for the
        // lifetime of the test. This is acceptable in test code.
        let _ = Box::leak(Box::new(temp_dir));
        AppState::new(db_path.to_str().unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn create_system_prompt_inserts_row() {
        let state = fixture_state().await;
        let id = state
            .create_system_prompt("550e8400-e29b-41d4-a716-446655440001", "test prompt", false)
            .await
            .unwrap();
        let prompts = state.list_system_prompts().await.unwrap();
        assert!(
            prompts
                .iter()
                .any(|p| p.id == id && p.content == "test prompt")
        );
    }

    #[tokio::test]
    async fn create_system_prompt_active_deactivates_others() {
        let state = fixture_state().await;
        let first = state
            .create_system_prompt("550e8400-e29b-41d4-a716-446655440001", "first", true)
            .await
            .unwrap();
        let second = state
            .create_system_prompt("550e8400-e29b-41d4-a716-446655440001", "second", true)
            .await
            .unwrap();

        let prompts = state.list_system_prompts().await.unwrap();
        let first_prompt = prompts.iter().find(|p| p.id == first).unwrap();
        let second_prompt = prompts.iter().find(|p| p.id == second).unwrap();
        assert!(!first_prompt.is_active);
        assert!(second_prompt.is_active);
    }

    #[tokio::test]
    async fn delete_system_prompt_removes_row() {
        let state = fixture_state().await;
        let id = state
            .create_system_prompt("550e8400-e29b-41d4-a716-446655440001", "to delete", false)
            .await
            .unwrap();
        state.delete_system_prompt(id).await.unwrap();
        let prompts = state.list_system_prompts().await.unwrap();
        assert!(!prompts.iter().any(|p| p.id == id));
    }

    #[tokio::test]
    async fn activate_system_prompt_deactivates_others() {
        let state = fixture_state().await;
        let first = state
            .create_system_prompt("550e8400-e29b-41d4-a716-446655440001", "first", true)
            .await
            .unwrap();
        let second = state
            .create_system_prompt("550e8400-e29b-41d4-a716-446655440001", "second", false)
            .await
            .unwrap();

        state.activate_system_prompt(second).await.unwrap();
        let prompts = state.list_system_prompts().await.unwrap();
        let first_prompt = prompts.iter().find(|p| p.id == first).unwrap();
        let second_prompt = prompts.iter().find(|p| p.id == second).unwrap();
        assert!(!first_prompt.is_active);
        assert!(second_prompt.is_active);
    }
}

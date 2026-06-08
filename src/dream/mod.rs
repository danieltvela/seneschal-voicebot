use std::sync::Arc;
/// S-DREAM — Scheduled Dream Daemon.
///
/// Background daemon that performs cold-path memory consolidation
/// at scheduled night hours or when the user is idle.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::agents::ProactiveEvent;
use crate::db::Database;
use crate::llm::{LlmProvider, Message};
use crate::memory::extract_memories;
use crate::profile::extract_facts;

/// Configuration for the S-DREAM daemon.
#[derive(Debug, Clone)]
pub struct SDreamConfig {
    pub interval_secs: u64,
    pub on_idle: bool,
    pub idle_threshold_secs: u64,
    pub scheduled_hour: Option<u8>,
    pub l2_min_messages: usize,
    pub jsonl_dir: String,
}

/// Background daemon for cold-path memory consolidation.
pub struct SDreamDaemon {
    pub config: SDreamConfig,
    pub db: Database,
    pub secondary_client: Option<Arc<dyn LlmProvider>>,
    pub proactive_tx: mpsc::Sender<ProactiveEvent>,
    pub last_activity: Arc<AtomicU64>,
}

impl SDreamDaemon {
    /// Spawns the S-DREAM daemon as a background tokio task.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.run_loop().await;
        })
    }

    async fn run_loop(self) {
        info!(
            target: "dream",
            "S-DREAM daemon started (interval={}s, scheduled_hour={:?}, on_idle={})",
            self.config.interval_secs,
            self.config.scheduled_hour,
            self.config.on_idle
        );

        loop {
            let sleep_secs = if let Some(hour) = self.config.scheduled_hour {
                let now = Local::now();
                let target = now.date_naive().and_hms_opt(hour as u32, 0, 0).unwrap();
                let target = Local.from_local_datetime(&target).single().unwrap();
                let target = if target <= now {
                    target + chrono::Duration::try_days(1).unwrap_or(chrono::Duration::zero())
                } else {
                    target
                };
                (target - now).num_seconds().max(0) as u64
            } else {
                self.config.interval_secs
            };

            tokio::time::sleep(tokio::time::Duration::from_secs(sleep_secs)).await;

            if !self.should_run_cycle().await {
                debug!(target: "dream", "Skipping cycle — conditions not met");
                continue;
            }

            if let Err(e) = self.run_cycle().await {
                warn!(target: "dream", "S-DREAM cycle failed: {}", e);
            }
        }
    }

    async fn should_run_cycle(&self) -> bool {
        if self.config.on_idle {
            let last = self.last_activity.load(Ordering::Relaxed);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let idle_secs = now.saturating_sub(last);
            if idle_secs < self.config.idle_threshold_secs {
                debug!(
                    target: "dream",
                    "User not idle enough ({}s < {}s), skipping cycle",
                    idle_secs,
                    self.config.idle_threshold_secs
                );
                return false;
            }
        }

        let session_id = match self.db.get_or_create_session().await {
            Ok(id) => id,
            Err(e) => {
                warn!(target: "dream", "Failed to get session: {}", e);
                return false;
            }
        };

        let through_id = match self.db.get_summary_through_id(session_id).await {
            Ok(id) => id,
            Err(e) => {
                warn!(target: "dream", "Failed to get summary_through_id: {}", e);
                return false;
            }
        };

        let count = match self
            .db
            .count_messages_after_id(session_id, through_id)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(target: "dream", "Failed to count messages: {}", e);
                return false;
            }
        };

        if count < self.config.l2_min_messages as i64 {
            debug!(
                target: "dream",
                "Not enough new messages ({} < {}), skipping cycle",
                count,
                self.config.l2_min_messages
            );
            return false;
        }

        true
    }

    async fn run_cycle(&self) -> Result<()> {
        info!(target: "dream", "S-DREAM cycle starting");

        let session_id = self.db.get_or_create_session().await?;
        let through_id = self.db.get_summary_through_id(session_id).await?;

        // --- Incremental JSONL export ---
        let last_processed = self.db.get_dream_last_processed(session_id).await?;
        let jsonl_messages = if last_processed.is_empty() {
            self.db
                .get_messages_with_timestamp_after_id(session_id, through_id)
                .await?
        } else {
            self.db
                .get_messages_since(session_id, &last_processed)
                .await?
        };
        self.export_to_jsonl(session_id, &jsonl_messages).await?;
        if let Some(last_msg) = jsonl_messages.last() {
            self.db
                .set_dream_last_processed(session_id, &last_msg.3)
                .await?;
        }

        // --- Distillation (full batch since through_id) ---
        let messages = self
            .db
            .get_messages_with_timestamp_after_id(session_id, through_id)
            .await?;
        if messages.is_empty() {
            debug!(target: "dream", "No new messages to distil");
            return Ok(());
        }

        let conversation_text = messages
            .iter()
            .map(|(_, role, content, _)| format!("{}: {}", role, content))
            .collect::<Vec<_>>()
            .join("\n");

        if let Some(client) = &self.secondary_client {
            let facts = extract_facts(client.as_ref(), "", &conversation_text).await;
            for fact in facts {
                if let Err(e) = self
                    .db
                    .upsert_profile_fact(&fact.key, &fact.value, fact.confidence)
                    .await
                {
                    warn!(target: "dream", "Failed to save profile fact: {}", e);
                }
            }

            let existing_memories = self.db.load_active_memories().await?;
            let extraction =
                extract_memories(client.as_ref(), &conversation_text, &existing_memories).await;

            if !extraction.new_memories.is_empty()
                && let Err(e) = self
                    .db
                    .save_memories_batch(&extraction.new_memories, session_id)
                    .await
            {
                warn!(target: "dream", "Failed to save memories: {}", e);
            }

            for archive_id in extraction.archive_ids {
                if let Err(e) = self.db.deactivate_memory(archive_id).await {
                    warn!(target: "dream", "Failed to archive memory {}: {}", archive_id, e);
                }
            }

            let summary = self
                .generate_summary(client.as_ref(), &conversation_text)
                .await?;
            let max_id = messages
                .last()
                .map(|(id, _, _, _)| *id)
                .unwrap_or(through_id);
            self.db.save_summary(session_id, &summary, max_id).await?;
        } else {
            debug!(target: "dream", "No secondary client available, skipping distillation");
        }

        if let Err(e) = self.detect_corrections(session_id).await {
            warn!(target: "dream", "Correction detection failed: {}", e);
        }

        let compacted = self.db.compact_user_profile().await?;
        if compacted > 0 {
            info!(
                target: "dream",
                "Compacted {} low-confidence profile facts",
                compacted
            );
        }

        info!(target: "dream", "S-DREAM cycle complete");
        Ok(())
    }

    /// Append messages to a dated JSONL file, rotating when the file
    /// exceeds 10 MB or 10 000 messages.  Old files are never deleted.
    async fn export_to_jsonl(
        &self,
        session_id: Uuid,
        messages: &[(i64, String, String, String)],
    ) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }

        let dir = std::path::Path::new(&self.config.jsonl_dir);
        tokio::fs::create_dir_all(dir).await?;

        let date = Local::now().format("%Y-%m-%d");
        let base_path = dir.join(format!("{}.jsonl", date));
        let path = self.resolve_jsonl_path(&base_path).await?;

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .with_context(|| format!("Failed to open JSONL file: {}", path.display()))?;

        for (_, role, content, timestamp) in messages {
            let entry = serde_json::json!({
                "session_id": session_id.to_string(),
                "timestamp": timestamp,
                "role": role,
                "content": content,
            });
            let line = entry.to_string();
            tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes()).await?;
            tokio::io::AsyncWriteExt::write_all(&mut file, b"\n").await?;
        }

        info!(
            target: "dream",
            "Appended {} messages to {}",
            messages.len(),
            path.display()
        );
        Ok(())
    }

    /// Find the first JSONL file for today that does not need rotation.
    /// If the base file is full, tries `.001.jsonl`, `.002.jsonl`, etc.
    async fn resolve_jsonl_path(&self, base_path: &std::path::Path) -> Result<std::path::PathBuf> {
        let stem = base_path.file_stem().unwrap_or_default().to_string_lossy();
        let parent = base_path.parent().unwrap_or(std::path::Path::new("."));

        let mut candidate = base_path.to_path_buf();
        if !tokio::fs::try_exists(&candidate).await? {
            return Ok(candidate);
        }
        if !self.needs_rotation(&candidate).await? {
            return Ok(candidate);
        }

        let mut suffix = 1u32;
        loop {
            candidate = parent.join(format!("{}.{:03}.jsonl", stem, suffix));
            if !tokio::fs::try_exists(&candidate).await? {
                return Ok(candidate);
            }
            if !self.needs_rotation(&candidate).await? {
                return Ok(candidate);
            }
            suffix += 1;
        }
    }

    /// Returns `true` when the file is larger than 10 MB or contains more
    /// than 10 000 lines.
    async fn needs_rotation(&self, path: &std::path::Path) -> Result<bool> {
        let metadata = tokio::fs::metadata(path).await?;
        if metadata.len() > 10 * 1024 * 1024 {
            return Ok(true);
        }
        let content = tokio::fs::read_to_string(path).await?;
        Ok(content.lines().count() > 10_000)
    }

    async fn generate_summary(
        &self,
        client: &dyn LlmProvider,
        conversation_text: &str,
    ) -> Result<String> {
        let messages = vec![
            Message::system(
                "Summarize the following conversation excerpt concisely. \
                 Capture the key topics discussed, decisions made, and any important context. \
                 Write in the same language as the conversation. \
                 Keep it to 2-4 sentences.",
            ),
            Message::user(format!("Conversation:\n\n{}", conversation_text)),
        ];

        client.complete(&messages).await
    }

    async fn detect_corrections(&self, session_id: Uuid) -> Result<()> {
        let (_, messages) = self.db.get_session_context(session_id, 50).await?;
        for (role, content) in &messages {
            if role == "user" {
                let corrections = crate::profile::detect_corrections(content, "");
                for c in corrections {
                    let key = format!("correction:{}", c.topic);
                    if let Err(e) = self
                        .db
                        .upsert_profile_fact(&key, &c.correction_text, c.confidence)
                        .await
                    {
                        warn!(target: "dream", "Failed to save correction '{}': {}", key, e);
                    } else {
                        info!(target: "dream", "Saved immutable rule: {} = {}", key, c.correction_text);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[tokio::test]
    async fn daemon_spawns_without_panic() {
        let (db, _dir) = fresh_db().await;
        let (tx, _rx) = mpsc::channel(1);
        let last_activity = Arc::new(AtomicU64::new(0));

        let daemon = SDreamDaemon {
            config: SDreamConfig {
                interval_secs: 3600,
                on_idle: false,
                idle_threshold_secs: 600,
                scheduled_hour: None,
                l2_min_messages: 50,
                jsonl_dir: "data/archives".to_string(),
            },
            db,
            secondary_client: None,
            proactive_tx: tx,
            last_activity,
        };

        let handle = daemon.spawn();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        handle.abort();
    }

    async fn fresh_db() -> (Database, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(path.to_str().unwrap()).await.unwrap();
        (db, dir)
    }

    fn build_daemon(db: Database, jsonl_dir: &std::path::Path) -> SDreamDaemon {
        let (tx, _rx) = mpsc::channel(1);
        let last_activity = Arc::new(AtomicU64::new(0));
        SDreamDaemon {
            config: SDreamConfig {
                interval_secs: 3600,
                on_idle: false,
                idle_threshold_secs: 600,
                scheduled_hour: None,
                l2_min_messages: 1,
                jsonl_dir: jsonl_dir.to_str().unwrap().to_string(),
            },
            db,
            secondary_client: None,
            proactive_tx: tx,
            last_activity,
        }
    }

    #[tokio::test]
    async fn jsonl_rotation_by_size_creates_sequential_file() {
        let (db, _db_dir) = fresh_db().await;
        let jsonl_dir = tempfile::TempDir::new().unwrap();
        let daemon = build_daemon(db, jsonl_dir.path());

        let date = Local::now().format("%Y-%m-%d");
        let base = jsonl_dir.path().join(format!("{}.jsonl", date));

        // Pre-seed base file with > 10 MB of data.
        let padding = "x".repeat(1024 * 1024);
        let mut file = tokio::fs::File::create(&base).await.unwrap();
        for _ in 0..11 {
            tokio::io::AsyncWriteExt::write_all(&mut file, padding.as_bytes())
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut file, b"\n")
                .await
                .unwrap();
        }
        drop(file);

        let sid = Uuid::new_v4();
        let messages = vec![(
            1i64,
            "user".to_string(),
            "hello".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
        )];
        daemon.export_to_jsonl(sid, &messages).await.unwrap();

        let rotated = jsonl_dir.path().join(format!("{}.001.jsonl", date));
        assert!(
            rotated.exists(),
            "Rotation must create .001.jsonl when base > 10 MB"
        );

        let content = tokio::fs::read_to_string(&rotated).await.unwrap();
        assert!(
            content.contains("hello"),
            "Rotated file must contain the new message"
        );
    }

    #[tokio::test]
    async fn jsonl_rotation_by_message_count_creates_sequential_file() {
        let (db, _db_dir) = fresh_db().await;
        let jsonl_dir = tempfile::TempDir::new().unwrap();
        let daemon = build_daemon(db, jsonl_dir.path());

        let date = Local::now().format("%Y-%m-%d");
        let base = jsonl_dir.path().join(format!("{}.jsonl", date));

        // Pre-seed base file with 10 001 lines.
        let mut file = tokio::fs::File::create(&base).await.unwrap();
        for i in 0..10_001 {
            let line = format!("{{\"n\":{}}}\n", i);
            tokio::io::AsyncWriteExt::write_all(&mut file, line.as_bytes())
                .await
                .unwrap();
        }
        drop(file);

        let sid = Uuid::new_v4();
        let messages = vec![(
            1i64,
            "user".to_string(),
            "count".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
        )];
        daemon.export_to_jsonl(sid, &messages).await.unwrap();

        let rotated = jsonl_dir.path().join(format!("{}.001.jsonl", date));
        assert!(
            rotated.exists(),
            "Rotation must create .001.jsonl when base > 10 000 messages"
        );

        let content = tokio::fs::read_to_string(&rotated).await.unwrap();
        assert!(content.contains("count"));
    }

    #[tokio::test]
    async fn jsonl_incremental_export_only_new_messages() {
        let (db, _db_dir) = fresh_db().await;
        let jsonl_dir = tempfile::TempDir::new().unwrap();
        let daemon = build_daemon(db, jsonl_dir.path());

        let sid = Uuid::new_v4();
        let ts1 = "2024-01-01T00:00:00Z";
        let ts2 = "2024-01-01T00:01:00Z";

        let batch1 = vec![(
            1i64,
            "user".to_string(),
            "first".to_string(),
            ts1.to_string(),
        )];
        daemon.export_to_jsonl(sid, &batch1).await.unwrap();

        let batch2 = vec![(
            2i64,
            "assistant".to_string(),
            "second".to_string(),
            ts2.to_string(),
        )];
        daemon.export_to_jsonl(sid, &batch2).await.unwrap();

        let date = Local::now().format("%Y-%m-%d");
        let base = jsonl_dir.path().join(format!("{}.jsonl", date));
        let content = tokio::fs::read_to_string(&base).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "Both batches must be in the same file");

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["content"], "first");
        assert_eq!(second["content"], "second");
    }

    #[tokio::test]
    async fn should_run_cycle_false_when_not_idle() {
        let (db, _db_dir) = fresh_db().await;
        let (tx, _rx) = mpsc::channel(1);
        let last_activity = Arc::new(AtomicU64::new(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        ));
        let daemon = SDreamDaemon {
            config: SDreamConfig {
                interval_secs: 3600,
                on_idle: true,
                idle_threshold_secs: 600,
                scheduled_hour: None,
                l2_min_messages: 1,
                jsonl_dir: "data/archives".to_string(),
            },
            db,
            secondary_client: None,
            proactive_tx: tx,
            last_activity,
        };
        assert!(
            !daemon.should_run_cycle().await,
            "Cycle must not run when user is not idle"
        );
    }

    #[tokio::test]
    async fn should_run_cycle_false_when_not_enough_messages() {
        let (db, _db_dir) = fresh_db().await;
        let (tx, _rx) = mpsc::channel(1);
        let last_activity = Arc::new(AtomicU64::new(0));
        let daemon = SDreamDaemon {
            config: SDreamConfig {
                interval_secs: 3600,
                on_idle: false,
                idle_threshold_secs: 600,
                scheduled_hour: None,
                l2_min_messages: 50,
                jsonl_dir: "data/archives".to_string(),
            },
            db,
            secondary_client: None,
            proactive_tx: tx,
            last_activity,
        };
        assert!(
            !daemon.should_run_cycle().await,
            "Cycle must not run when there are not enough messages"
        );
    }

    #[tokio::test]
    async fn should_run_cycle_true_when_idle_and_enough_messages() {
        let (db, _db_dir) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        for i in 0..5 {
            db.save_message(sid, "user", &format!("msg {i}"))
                .await
                .unwrap();
        }
        let (tx, _rx) = mpsc::channel(1);
        let last_activity = Arc::new(AtomicU64::new(0));
        let daemon = SDreamDaemon {
            config: SDreamConfig {
                interval_secs: 3600,
                on_idle: false,
                idle_threshold_secs: 600,
                scheduled_hour: None,
                l2_min_messages: 1,
                jsonl_dir: "data/archives".to_string(),
            },
            db,
            secondary_client: None,
            proactive_tx: tx,
            last_activity,
        };
        assert!(
            daemon.should_run_cycle().await,
            "Cycle must run when idle and enough messages exist"
        );
    }

    #[tokio::test]
    async fn run_cycle_with_mock_llm_extracts_facts_and_saves_summary() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content":
                    r#"[{"key": "name", "value": "Daniel", "confidence": 0.95}]"#
                }}]
            })))
            .mount(&server)
            .await;

        let client = crate::llm::OpenAiLlmProvider::new(&server.uri(), "test", 256, 0.1);
        let (db, _db_dir) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "Me llamo Daniel.")
            .await
            .unwrap();
        db.save_message(sid, "assistant", "Hola Daniel.")
            .await
            .unwrap();

        let jsonl_dir = tempfile::TempDir::new().unwrap();
        let (tx, _rx) = mpsc::channel(1);
        let last_activity = Arc::new(AtomicU64::new(0));
        let daemon = SDreamDaemon {
            config: SDreamConfig {
                interval_secs: 3600,
                on_idle: false,
                idle_threshold_secs: 600,
                scheduled_hour: None,
                l2_min_messages: 1,
                jsonl_dir: jsonl_dir.path().to_str().unwrap().to_string(),
            },
            db: db.clone(),
            secondary_client: Some(Arc::new(client)),
            proactive_tx: tx,
            last_activity,
        };

        daemon.run_cycle().await.unwrap();

        let profile = db.load_user_profile().await.unwrap();
        assert!(
            profile.iter().any(|(k, v, _)| k == "name" && v == "Daniel"),
            "Profile fact must be saved after cycle"
        );
    }

    #[tokio::test]
    async fn detect_corrections_saves_immutable_rules() {
        let (db, _db_dir) = fresh_db().await;
        let sid = db.get_or_create_session().await.unwrap();
        db.save_message(sid, "user", "No, en realidad me llamo Daniel.")
            .await
            .unwrap();

        let jsonl_dir = tempfile::TempDir::new().unwrap();
        let (tx, _rx) = mpsc::channel(1);
        let last_activity = Arc::new(AtomicU64::new(0));
        let daemon = SDreamDaemon {
            config: SDreamConfig {
                interval_secs: 3600,
                on_idle: false,
                idle_threshold_secs: 600,
                scheduled_hour: None,
                l2_min_messages: 1,
                jsonl_dir: jsonl_dir.path().to_str().unwrap().to_string(),
            },
            db: db.clone(),
            secondary_client: None,
            proactive_tx: tx,
            last_activity,
        };

        daemon.detect_corrections(sid).await.unwrap();

        let rules = db.get_immutable_rules().await.unwrap();
        assert!(
            !rules.is_empty(),
            "Correction must be saved as immutable rule"
        );
        assert!(
            rules
                .iter()
                .any(|(k, _v, c)| k.starts_with("correction:") && c == &1.0),
            "Saved rule must have correction: prefix and confidence 1.0"
        );
    }
}

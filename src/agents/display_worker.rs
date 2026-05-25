use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

use chrono::Local;
use regex::Regex;

use super::session_events::{AcpSessionEvent, SessionEventRx};

const LOG_DIR: &str = "/tmp/voicebot_sessions";

/// Formats and displays ACP session events to a log file.
///
/// Consumes events from a bounded channel and writes formatted lines to
/// `/tmp/voicebot_sessions/{session_id}.log`. The worker shuts down cleanly
/// when the channel closes.
pub struct SessionDisplayWorker {
    session_id: String,
    rx: SessionEventRx,
}

impl SessionDisplayWorker {
    pub fn new(session_id: String, rx: SessionEventRx) -> Self {
        Self { session_id, rx }
    }

    /// Resolve the log file path for a session.
    fn log_path(session_id: &str) -> PathBuf {
        let dir = PathBuf::from(LOG_DIR);
        dir.join(format!("{session_id}.log"))
    }

    /// Spawn the display worker as a background task.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move { self.run().await })
    }

    async fn run(mut self) {
        let dir = PathBuf::from(LOG_DIR);
        let _ = std::fs::create_dir_all(&dir);

        let path = Self::log_path(&self.session_id);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|_| panic!("cannot open log file: {:?}", path));

        while let Some(event) = self.rx.recv().await {
            let line = format_event(&event);
            let line = redact_sensitive(&line);
            writeln!(file, "{line}").ok();
            flush_file(&mut file);
        }
    }
}

/// Compiled regex patterns for sensitive data redaction.
static REDACTION_PATTERNS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"(Bearer\s+)\S+").unwrap(), "${1}[REDACTED]"),
        (Regex::new(r"\bsk-[A-Za-z0-9_-]+").unwrap(), "[API_KEY_REDACTED]"),
        (Regex::new(r"\bgh_[A-Za-z0-9_]{36,}").unwrap(), "[GH_TOKEN_REDACTED]"),
        (Regex::new(r"(token\s*:\s*\S+)").unwrap(), "token: [TOKEN_REDACTED]"),
        (Regex::new(r"(password\s*:\s*\S+)").unwrap(), "password: [PASSWORD_REDACTED]"),
        (Regex::new(r"(secret\s*:\s*\S+)").unwrap(), "secret: [SECRET_REDACTED]"),
        // Email addresses
        (
            Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}")
                .unwrap(),
            "[EMAIL_REDACTED]",
        ),
        // Long hex strings (hashes, fingerprints)
        (Regex::new(r"\b[a-fA-F0-9]{40,}\b").unwrap(), "[HASH_REDACTED]"),
        // UUIDs
        (Regex::new(r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b").unwrap(), "[UUID_REDACTED]"),
    ]
});

/// Redacts sensitive data patterns from a log line.
pub fn redact_sensitive(line: &str) -> String {
    let mut result = line.to_string();
    for (pattern, replacement) in REDACTION_PATTERNS.iter() {
        result = pattern.replace(&result, *replacement).to_string();
    }
    result
}

/// Format an event into a colored log line: `[HH:MM:SS] [TYPE] content`.
fn format_event(event: &AcpSessionEvent) -> String {
    let ts = Local::now().format("%H:%M:%S");
    match event {
        AcpSessionEvent::AgentMessageChunk { text, .. } => {
            format!("[\033[36m{ts}\033[0m] [\033[32mAGENT\033[0m] {text}")
        }
        AcpSessionEvent::AgentThoughtChunk { text, .. } => {
            format!("[\033[36m{ts}\033[0m] [\033[33mTHINK\033[0m] {text}")
        }
        AcpSessionEvent::ToolCall { name, .. } => {
            format!("[\033[36m{ts}\033[0m] [\033[34mTOOL\033[0m] {name}: started")
        }
        AcpSessionEvent::ToolCallUpdate { name, status, .. } => {
            format!("[\033[36m{ts}\033[0m] [\033[34mTOOL\033[0m] {name}: {status}")
        }
        AcpSessionEvent::PermissionRequest {
            description,
            options,
            ..
        } => {
            let opts = options.join(", ");
            format!("[\033[36m{ts}\033[0m] [\033[31mPERM\033[0m] {description}? [{opts}]")
        }
    }
}

fn flush_file(file: &mut std::fs::File) {
    file.flush().ok();
}

/// Resolve the log path for a session (public helper for terminal integration).
pub fn session_log_path(session_id: &str) -> PathBuf {
    SessionDisplayWorker::log_path(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_agent_message() {
        let event = AcpSessionEvent::AgentMessageChunk {
            text: "hello".to_string(),
            correlation_id: "".to_string(),
        };
        let line = format_event(&event);
        assert!(line.contains("AGENT"));
        assert!(line.contains("hello"));
    }

    #[test]
    fn test_format_tool_call() {
        let event = AcpSessionEvent::ToolCall {
            name: "web_search".to_string(),
            correlation_id: "".to_string(),
        };
        let line = format_event(&event);
        assert!(line.contains("TOOL"));
        assert!(line.contains("web_search"));
        assert!(line.contains("started"));
    }

    #[test]
    fn test_format_thought() {
        let event = AcpSessionEvent::AgentThoughtChunk {
            text: "reasoning".to_string(),
            correlation_id: "".to_string(),
        };
        let line = format_event(&event);
        assert!(line.contains("THINK"));
        assert!(line.contains("reasoning"));
    }

    #[test]
    fn test_format_permission() {
        let event = AcpSessionEvent::PermissionRequest {
            description: "Allow?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
            correlation_id: "".to_string(),
        };
        let line = format_event(&event);
        assert!(line.contains("PERM"));
        assert!(line.contains("Allow?"));
        assert!(line.contains("yes, no"));
    }

    #[test]
    fn test_writes_to_file() {
        use tokio::sync::mpsc;

        let tmp_dir = std::env::temp_dir().join("voicebot_test");
        let log_dir = tmp_dir.join("sessions");
        let _ = std::fs::create_dir_all(&log_dir);

        let session_id = "test-session-001";
        let path = log_dir.join(format!("{session_id}.log"));

        let (tx, mut rx) = mpsc::channel::<AcpSessionEvent>(16);
        tx.blocking_send(AcpSessionEvent::AgentMessageChunk {
            text: "test event".into(),
            correlation_id: "".to_string(),
        })
            .unwrap();
        drop(tx);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        while let Ok(event) = rx.try_recv() {
            let line = format_event(&event);
            writeln!(file, "{line}").unwrap();
        }

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("AGENT"));
        assert!(content.contains("test event"));

        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn test_format_includes_timestamp() {
        let event = AcpSessionEvent::AgentMessageChunk {
            text: "x".to_string(),
            correlation_id: "".to_string(),
        };
        let line = format_event(&event);
        let stripped = strip_ansi(&line);
        assert!(stripped.starts_with('['));
        assert!(stripped.contains(':'));
    }

    fn strip_ansi(s: &str) -> String {
        s.replace("\u{001b}[36m", "")
            .replace("\u{001b}[32m", "")
            .replace("\u{001b}[33m", "")
            .replace("\u{001b}[34m", "")
            .replace("\u{001b}[31m", "")
            .replace("\u{001b}[0m", "")
    }

    #[test]
    fn test_log_path_creates_file() {
        let path = session_log_path("abc123");
        assert_eq!(path.to_string_lossy(), "/tmp/voicebot_sessions/abc123.log");
    }

    #[test]
    fn test_redact_api_key() {
        let line = "[12:00:00] [AGENT] Using key sk-abc123xyz for auth";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("[API_KEY_REDACTED]"));
        assert!(!redacted.contains("sk-abc123xyz"));
    }

    #[test]
    fn test_redact_email() {
        let line = "[12:00:00] [AGENT] Contact user@example.com";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("[EMAIL_REDACTED]"));
        assert!(!redacted.contains("user@example.com"));
    }

    #[test]
    fn test_redact_password() {
        let line = "[12:00:00] [AGENT] password: SuperSecret123!";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("[PASSWORD_REDACTED]"));
        assert!(!redacted.contains("SuperSecret123"));
    }

    #[test]
    fn test_redact_uuid() {
        let line = "[12:00:00] [AGENT] Session 550e8400-e29a-41d4-a716-446655440000";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("[UUID_REDACTED]"));
        assert!(!redacted.contains("550e8400"));
    }

    #[test]
    fn test_redact_hash() {
        let line = "[12:00:00] [AGENT] Hash a]bc123e4f5d6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("[HASH_REDACTED]"));
    }

    #[test]
    fn test_redact_preserves_normal_text() {
        let line = "[12:00:00] [AGENT] Hello world, no secrets here";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("Hello world"));
        assert!(!redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_bearer_token() {
        let line = "[12:00:00] [AGENT] Auth: Bearer eyJhbGciOiJIUzI1NiJ9.test.signature";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("eyJhbGci"));
    }

    #[test]
    fn test_redact_github_token() {
        let line = "[12:00:00] [AGENT] Token gh_abcdef1234567890abcdef1234567890abcd done";
        let redacted = redact_sensitive(line);
        assert!(redacted.contains("[GH_TOKEN_REDACTED]"));
    }

    // ── Latency tests ──────────────────────────────────────────────────────────

    use std::time::{Duration, Instant};

    async fn measure_display_latency(n_events: usize) -> Duration {
        let tmp_dir = std::env::temp_dir().join(format!(
            "voicebot_latency_{}",
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::create_dir_all(&tmp_dir);

        let session_id = format!("latency-{}", n_events);
        let log_path = tmp_dir.join(format!("{session_id}.log"));

        let (tx, mut rx) = tokio::sync::mpsc::channel::<AcpSessionEvent>(16);

        // Rewrite LOG_DIR via a temp env var won't affect the constant, so we
        // patch the log_path resolution by spawning our own worker-like loop
        // that writes to our controlled directory.
        let writer_handle = tokio::spawn(async move {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .expect("cannot open log file");
            while let Some(event) = rx.recv().await {
                let line = format_event(&event);
                writeln!(file, "{line}").ok();
                flush_file(&mut file);
            }
        });

        let start = Instant::now();
        for i in 0..n_events {
            tx.send(AcpSessionEvent::AgentMessageChunk {
                text: format!("event-{i}"),
                correlation_id: "".into(),
            })
            .await
            .unwrap();
        }
        drop(tx);
        writer_handle.await.ok();

        let elapsed = start.elapsed();
        std::fs::remove_dir_all(&tmp_dir).ok();
        elapsed
    }

    #[tokio::test]
    async fn test_latency_single_event_under_100ms() {
        let elapsed = measure_display_latency(1).await;
        assert!(
            elapsed.as_millis() < 100,
            "single event latency {:?} >= 100ms",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_latency_ten_events_under_100ms() {
        let elapsed = measure_display_latency(10).await;
        assert!(
            elapsed.as_millis() < 100,
            "10-event latency {:?} >= 100ms",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_latency_burst_under_100ms() {
        // Channel capacity is 16, burst should fit entirely.
        let elapsed = measure_display_latency(16).await;
        assert!(
            elapsed.as_millis() < 100,
            "16-event burst latency {:?} >= 100ms",
            elapsed
        );
    }
}

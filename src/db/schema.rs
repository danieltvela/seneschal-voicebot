/// Database schema definitions
///
/// This module contains the SQL schema for the voicebot database.
/// The actual migrations are run in database.rs

pub const SESSIONS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    closed_at TEXT,
    is_active INTEGER NOT NULL DEFAULT 1
)
"#;

pub const MESSAGES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    content_type TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
)
"#;

pub const CONFIG_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
)
"#;

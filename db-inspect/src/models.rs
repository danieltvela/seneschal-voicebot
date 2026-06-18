/// Rust structs mirroring Voicebot's SQLite schema.
///
/// These map 1:1 to the tables created in `src/db/database.rs` migrations.
use serde::Serialize;
use sqlx::FromRow;

// ── Sessions ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub closed_at: Option<String>,
    pub is_active: bool,
    pub summary: Option<String>,
    pub summary_through_id: i64,
}

// ── Messages ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: i64,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    ToolExchanges,
}

impl From<&str> for MessageRole {
    fn from(role: &str) -> Self {
        match role {
            "User" => Self::User,
            "Assistant" => Self::Assistant,
            "System" => Self::System,
            "ToolExchanges" => Self::ToolExchanges,
            _ => Self::System,
        }
    }
}

// ── User Profile ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UserProfileEntry {
    pub key: String,
    pub value: String,
    pub confidence: f64,
    pub updated_at: String,
    pub is_under_review: bool,
}

// ── Memories ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub is_active: bool,
}

// ── Profile History ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProfileHistoryEntry {
    pub id: i64,
    pub key: String,
    pub value: String,
    pub confidence: f64,
    pub timestamp: String,
    pub change_type: String,
}

// ── Dream State ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DreamState {
    pub session_id: String,
    pub last_processed_at: String,
}

// ── System Prompts ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SystemPrompt {
    pub id: i64,
    pub session_id: String,
    pub content: String,
    pub is_active: bool,
    pub created_at: String,
}

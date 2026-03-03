use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Conversation context for a session
#[derive(Debug, Clone)]
pub struct ConversationContext {
    session_id: Uuid,
    messages: Vec<Message>,
    metadata: ContextMetadata,
}

impl ConversationContext {
    pub fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            messages: Vec::new(),
            metadata: ContextMetadata::default(),
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.metadata.last_updated = Utc::now();
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn metadata(&self) -> &ContextMetadata {
        &self.metadata
    }

    /// Get recent messages for context window
    pub fn recent_messages(&self, count: usize) -> &[Message] {
        let start = self.messages.len().saturating_sub(count);
        &self.messages[start..]
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: MessageContent,
    pub timestamp: DateTime<Utc>,
}

impl Message {
    pub fn new(role: MessageRole, content: MessageContent) -> Self {
        Self {
            role,
            content,
            timestamp: Utc::now(),
        }
    }

    pub fn user_text(text: String) -> Self {
        Self::new(MessageRole::User, MessageContent::Text(text))
    }

    pub fn assistant_text(text: String) -> Self {
        Self::new(MessageRole::Assistant, MessageContent::Text(text))
    }

    pub fn user_audio(audio: Vec<f32>) -> Self {
        Self::new(MessageRole::User, MessageContent::Audio(audio))
    }

    pub fn assistant_audio(audio: Vec<f32>) -> Self {
        Self::new(MessageRole::Assistant, MessageContent::Audio(audio))
    }

    pub fn tool_call(name: String, args: String) -> Self {
        Self::new(MessageRole::Tool, MessageContent::ToolCall { name, args })
    }

    pub fn tool_result(name: String, result: String) -> Self {
        Self::new(
            MessageRole::Tool,
            MessageContent::ToolResult { name, result },
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageContent {
    Text(String),
    Audio(Vec<f32>),
    ToolCall { name: String, args: String },
    ToolResult { name: String, result: String },
}

#[derive(Debug, Clone)]
pub struct ContextMetadata {
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub total_tokens: usize,
}

impl Default for ContextMetadata {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            created_at: now,
            last_updated: now,
            total_tokens: 0,
        }
    }
}

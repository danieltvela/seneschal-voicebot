use anyhow::Result;
use uuid::Uuid;
use std::collections::HashMap;
use chrono::{DateTime, Utc};

use crate::db::Database;
use super::context::{ConversationContext, Message};

/// Manages user sessions and conversation state
pub struct SessionManager {
    db: Database,
    active_sessions: HashMap<Uuid, ConversationContext>,
    current_session_id: Option<Uuid>,
}

impl SessionManager {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            active_sessions: HashMap::new(),
            current_session_id: None,
        }
    }

    /// Create a new session
    pub async fn create_session(&mut self) -> Result<Uuid> {
        let session_id = Uuid::new_v4();
        let context = ConversationContext::new(session_id);
        
        // Save to database
        self.db.create_session(session_id).await?;
        
        self.active_sessions.insert(session_id, context);
        self.current_session_id = Some(session_id);
        
        Ok(session_id)
    }

    /// Get current session
    pub fn current_session(&self) -> Option<&ConversationContext> {
        self.current_session_id
            .and_then(|id| self.active_sessions.get(&id))
    }

    /// Get mutable current session
    pub fn current_session_mut(&mut self) -> Option<&mut ConversationContext> {
        self.current_session_id
            .and_then(|id| self.active_sessions.get_mut(&id))
    }

    /// Add message to current session
    pub async fn add_message(&mut self, message: Message) -> Result<()> {
        if let Some(session_id) = self.current_session_id {
            // Add to in-memory context
            if let Some(context) = self.active_sessions.get_mut(&session_id) {
                context.add_message(message.clone());
            }
            
            // Persist to database
            self.db.save_message(session_id, &message).await?;
        }
        Ok(())
    }

    /// Get conversation history for current session
    pub fn get_history(&self) -> Vec<Message> {
        self.current_session()
            .map(|ctx| ctx.messages().to_vec())
            .unwrap_or_default()
    }

    /// Load session from database
    pub async fn load_session(&mut self, session_id: Uuid) -> Result<()> {
        let messages = self.db.get_session_messages(session_id).await?;
        let mut context = ConversationContext::new(session_id);
        
        for msg in messages {
            context.add_message(msg);
        }
        
        self.active_sessions.insert(session_id, context);
        self.current_session_id = Some(session_id);
        
        Ok(())
    }

    /// Close current session
    pub async fn close_session(&mut self) -> Result<()> {
        if let Some(session_id) = self.current_session_id {
            self.db.close_session(session_id).await?;
            self.active_sessions.remove(&session_id);
            self.current_session_id = None;
        }
        Ok(())
    }

    /// Get all session IDs
    pub async fn list_sessions(&self) -> Result<Vec<(Uuid, DateTime<Utc>)>> {
        self.db.list_sessions().await
    }
}

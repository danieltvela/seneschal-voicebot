use askama::Template;
use askama_web::WebTemplate;
use axum::{
    extract::{Path, Query, State},
    response::Html,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use crate::{
    db::AppState,
    error::AppError,
    models::{Message, MessageRole, Session},
};

const PAGE_SIZE: i64 = 100;

#[derive(Debug, Deserialize)]
pub struct PaginationQuery {
    #[serde(default = "default_page")]
    page: i64,
}

fn default_page() -> i64 {
    1
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionDisplay {
    pub id: String,
    pub created_at: String,
    pub closed_at: Option<String>,
    pub is_active: bool,
    pub summary: Option<String>,
}

impl From<Session> for SessionDisplay {
    fn from(session: Session) -> Self {
        Self {
            id: session.id,
            created_at: session.created_at,
            closed_at: session.closed_at,
            is_active: session.is_active,
            summary: session.summary,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MessageDisplay {
    pub timestamp: String,
    pub role_class: String,
    pub role_display: String,
    pub display_content: String,
}

impl From<Message> for MessageDisplay {
    fn from(msg: Message) -> Self {
        let (role_class, role_display) = match &msg.role {
            MessageRole::User => ("user".to_string(), "User".to_string()),
            MessageRole::Assistant => ("assistant".to_string(), "Assistant".to_string()),
            MessageRole::System => ("system".to_string(), "System".to_string()),
            MessageRole::ToolExchanges => ("tool".to_string(), "ToolExchanges".to_string()),
        };

        let display_content = if matches!(&msg.role, MessageRole::ToolExchanges) {
            match serde_json::from_str::<Value>(&msg.content) {
                Ok(json) => {
                    let pretty =
                        serde_json::to_string_pretty(&json).unwrap_or_else(|_| msg.content.clone());
                    format!("<pre>{}</pre>", html_escape::encode_text(&pretty))
                }
                Err(_) => {
                    format!("<pre>{}</pre>", html_escape::encode_text(&msg.content))
                }
            }
        } else {
            html_escape::encode_text(&msg.content).to_string()
        };

        Self {
            timestamp: msg.timestamp,
            role_class,
            role_display,
            display_content,
        }
    }
}

#[derive(Debug, Template, WebTemplate)]
#[template(path = "session_detail.html")]
pub struct SessionDetailTemplate {
    pub session: SessionDisplay,
    pub messages: Vec<MessageDisplay>,
    pub has_next: bool,
    pub next_page: i64,
    pub current_page: i64,
}

pub async fn session_detail(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Html<String>, AppError> {
    let page = pagination.page.max(1);
    let offset = (page - 1) * PAGE_SIZE;

    let session = state
        .get_session(&session_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let messages = state
        .list_messages_for_session(&session_id, PAGE_SIZE + 1, offset)
        .await?;

    let has_next = messages.len() > PAGE_SIZE as usize;
    let messages: Vec<MessageDisplay> = messages
        .into_iter()
        .take(PAGE_SIZE as usize)
        .map(MessageDisplay::from)
        .collect();

    let template = SessionDetailTemplate {
        session: SessionDisplay::from(session),
        messages,
        has_next,
        next_page: page + 1,
        current_page: page,
    };

    Ok(Html(template.render()?))
}

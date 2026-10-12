use askama::Template;
use askama_web::WebTemplate;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::db::AppState;
use crate::error::AppError;
use crate::models::Session;

/// Query parameters for session list pagination.
#[derive(Debug, Deserialize)]
pub struct SessionsQuery {
    #[serde(default = "default_page")]
    pub page: u32,
}

fn default_page() -> u32 {
    1
}

const PAGE_SIZE: i64 = 50;

/// Session with message count for list display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionWithCount {
    pub id: String,
    pub created_at: String,
    pub closed_at: Option<String>,
    pub is_active: bool,
    pub summary: Option<String>,
    pub message_count: i64,
}

impl From<(Session, i64)> for SessionWithCount {
    fn from((session, count): (Session, i64)) -> Self {
        Self {
            id: session.id,
            created_at: session.created_at,
            closed_at: session.closed_at,
            is_active: session.is_active,
            summary: session.summary,
            message_count: count,
        }
    }
}

/// Template for the sessions list page.
#[derive(Template, WebTemplate)]
#[template(path = "sessions.html")]
pub struct SessionsTemplate {
    pub sessions: Vec<SessionWithCount>,
    pub page: u32,
    pub has_next: bool,
    pub next_page: u32,
    pub deleted: bool,
}

/// GET /sessions — List sessions with pagination (50 per page).
pub async fn sessions_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionsQuery>,
) -> Result<Html<String>, AppError> {
    let page = query.page.max(1);
    let offset = ((page - 1) as i64) * PAGE_SIZE;

    let sessions = state.list_sessions(PAGE_SIZE, offset).await?;

    let mut sessions_with_count = Vec::with_capacity(sessions.len());
    for session in sessions {
        let count = state.count_messages_for_session(&session.id).await?;
        sessions_with_count.push(SessionWithCount::from((session, count)));
    }

    let has_next = sessions_with_count.len() == PAGE_SIZE as usize;
    let next_page = page + 1;

    let template = SessionsTemplate {
        sessions: sessions_with_count,
        page,
        has_next,
        next_page,
        deleted: false,
    };

    Ok(Html(template.render()?))
}

/// GET /sessions?deleted=true — Sessions list with deletion success banner.
pub async fn sessions_handler_with_deleted(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionsQuery>,
) -> Result<Html<String>, AppError> {
    let page = query.page.max(1);
    let offset = ((page - 1) as i64) * PAGE_SIZE;

    let sessions = state.list_sessions(PAGE_SIZE, offset).await?;

    let mut sessions_with_count = Vec::with_capacity(sessions.len());
    for session in sessions {
        let count = state.count_messages_for_session(&session.id).await?;
        sessions_with_count.push(SessionWithCount::from((session, count)));
    }

    let has_next = sessions_with_count.len() == PAGE_SIZE as usize;
    let next_page = page + 1;

    let template = SessionsTemplate {
        sessions: sessions_with_count,
        page,
        has_next,
        next_page,
        deleted: true,
    };

    Ok(Html(template.render()?))
}

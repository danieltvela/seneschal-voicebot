use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Query, State};
use std::sync::Arc;

use crate::db::AppState;
use crate::error::AppError;
use crate::models::MessageRole;

#[derive(Template, WebTemplate)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    query: String,
    messages: Option<Vec<SearchRow>>,
}

pub struct SearchRow {
    pub session_id: String,
    pub timestamp: String,
    pub role_class: String,
    pub role_display: String,
    pub content: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

pub async fn search_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> Result<SearchTemplate, AppError> {
    let query = params.q.unwrap_or_default().trim().to_string();
    let messages = if query.is_empty() {
        None
    } else {
        let rows = state.search_messages(&query, 100).await?;
        Some(
            rows.into_iter()
                .map(|msg| SearchRow {
                    session_id: msg.session_id,
                    timestamp: msg.timestamp,
                    role_class: role_class(&msg.role),
                    role_display: role_display(&msg.role),
                    content: msg.content,
                })
                .collect(),
        )
    };

    Ok(SearchTemplate { query, messages })
}

fn role_class(role: &MessageRole) -> String {
    match role {
        MessageRole::User => "user".to_string(),
        MessageRole::Assistant => "assistant".to_string(),
        MessageRole::System => "system".to_string(),
        MessageRole::ToolExchanges => "tool".to_string(),
    }
}

fn role_display(role: &MessageRole) -> String {
    match role {
        MessageRole::User => "User".to_string(),
        MessageRole::Assistant => "Assistant".to_string(),
        MessageRole::System => "System".to_string(),
        MessageRole::ToolExchanges => "Tool".to_string(),
    }
}

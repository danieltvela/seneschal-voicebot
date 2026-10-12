use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::db::AppState;
use crate::error::AppError;
use crate::models::MessageRole;

const PAGE_SIZE: i64 = 100;

#[derive(Deserialize)]
pub struct MessagesQuery {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default)]
    pub deleted: bool,
}

fn default_page() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize)]
pub struct PageLink {
    pub page: i64,
    pub label: String,
    pub is_current: bool,
    pub is_separator: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pagination {
    pub current_page: i64,
    pub total_pages: i64,
    pub has_previous: bool,
    pub has_next: bool,
    pub previous_page: i64,
    pub next_page: i64,
    pub links: Vec<PageLink>,
}

/// Template for the messages list page.
#[derive(Template)]
#[template(path = "messages.html")]
pub struct MessagesTemplate {
    pub messages: Vec<MessageRow>,
    pub pagination: Pagination,
    pub deleted: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MessageRow {
    pub id: i64,
    pub session_id: String,
    pub timestamp: String,
    pub role_class: String,
    pub role_display: String,
    pub content: String,
}

impl From<crate::models::Message> for MessageRow {
    fn from(msg: crate::models::Message) -> Self {
        let (role_class, role_display) = match msg.role {
            MessageRole::User => ("user", "User"),
            MessageRole::Assistant => ("assistant", "Assistant"),
            MessageRole::System => ("system", "System"),
            MessageRole::ToolExchanges => ("tool", "Tool"),
        };
        let content = if msg.content.chars().count() > 80 {
            format!("{}…", msg.content.chars().take(80).collect::<String>())
        } else {
            msg.content
        };
        Self {
            id: msg.id,
            session_id: msg.session_id,
            timestamp: msg.timestamp,
            role_class: role_class.to_string(),
            role_display: role_display.to_string(),
            content,
        }
    }
}

fn build_pagination(current_page: i64, total: i64) -> Pagination {
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let current_page = current_page.clamp(1, total_pages);

    let has_previous = current_page > 1;
    let has_next = current_page < total_pages;
    let previous_page = (current_page - 1).max(1);
    let next_page = (current_page + 1).min(total_pages);

    let mut links = Vec::new();

    // Always show first page
    links.push(PageLink {
        page: 1,
        label: "1".to_string(),
        is_current: current_page == 1,
        is_separator: false,
    });

    // Determine range of pages to show around current
    let start = (current_page - 2).max(2);
    let end = (current_page + 2).min(total_pages - 1);

    // Add separator before range if needed
    if start > 2 {
        links.push(PageLink {
            page: 0,
            label: "…".to_string(),
            is_current: false,
            is_separator: true,
        });
    }

    // Add pages in range
    for p in start..=end {
        links.push(PageLink {
            page: p,
            label: p.to_string(),
            is_current: p == current_page,
            is_separator: false,
        });
    }

    // Add separator after range if needed
    if end < total_pages - 1 {
        links.push(PageLink {
            page: 0,
            label: "…".to_string(),
            is_current: false,
            is_separator: true,
        });
    }

    // Always show last page (if different from first)
    if total_pages > 1 {
        links.push(PageLink {
            page: total_pages,
            label: total_pages.to_string(),
            is_current: current_page == total_pages,
            is_separator: false,
        });
    }

    Pagination {
        current_page,
        total_pages,
        has_previous,
        has_next,
        previous_page,
        next_page,
        links,
    }
}

pub async fn messages_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MessagesQuery>,
) -> Result<Html<String>, AppError> {
    let page = query.page.max(1);
    let offset = (page - 1) * PAGE_SIZE;

    let messages = state.list_all_messages(PAGE_SIZE, offset).await?;
    let total = state.count_all_messages().await?;

    let message_rows: Vec<MessageRow> = messages.into_iter().map(Into::into).collect();
    let pagination = build_pagination(page, total);

    let template = MessagesTemplate {
        messages: message_rows,
        pagination,
        deleted: query.deleted,
    };

    Ok(Html(template.render()?))
}

pub async fn messages_deleted_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MessagesQuery>,
) -> Result<Html<String>, AppError> {
    let page = query.page.max(1);
    let offset = (page - 1) * PAGE_SIZE;

    let messages = state.list_all_messages(PAGE_SIZE, offset).await?;
    let total = state.count_all_messages().await?;

    let message_rows: Vec<MessageRow> = messages.into_iter().map(Into::into).collect();
    let pagination = build_pagination(page, total);

    let template = MessagesTemplate {
        messages: message_rows,
        pagination,
        deleted: true,
    };

    Ok(Html(template.render()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_row_truncates_unicode_safely() {
        let msg = crate::models::Message {
            id: 1,
            session_id: "test".to_string(),
            role: MessageRole::User,
            content: "Prueba tu integración con Hermes, lánzale una búsqueda, por ejemplo, de los últimos resultados de Fórmula 1.".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        };
        let row = MessageRow::from(msg);
        assert!(row.content.chars().count() <= 81);
        assert!(row.content.ends_with('…'));
    }

    #[test]
    fn message_row_keeps_short_content_intact() {
        let msg = crate::models::Message {
            id: 2,
            session_id: "test".to_string(),
            role: MessageRole::Assistant,
            content: "Short reply.".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        };
        let row = MessageRow::from(msg);
        assert_eq!(row.content, "Short reply.");
    }
}

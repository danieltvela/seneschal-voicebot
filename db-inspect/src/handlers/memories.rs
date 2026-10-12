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
use crate::models::Memory;

/// Query parameters for memories list filtering.
#[derive(Debug, Deserialize)]
pub struct MemoriesQuery {
    /// Filter by active status: "1" = active only, "0" = inactive only, absent = all
    #[serde(default, deserialize_with = "deserialize_active")]
    pub active: Option<bool>,
}

fn deserialize_active<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s.as_deref() {
        Some("1") => Ok(Some(true)),
        Some("0") => Ok(Some(false)),
        _ => Ok(None),
    }
}

/// Template for the memories list page.
#[derive(Template, WebTemplate)]
#[template(path = "memories.html")]
pub struct MemoriesTemplate {
    pub memories: Vec<Memory>,
    pub filter_all_active: bool,
    pub filter_active_active: bool,
    pub filter_inactive_active: bool,
}

/// GET /memories — List memories with optional active filter.
pub async fn memories_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MemoriesQuery>,
) -> Result<Html<String>, AppError> {
    let memories = state.list_memories(query.active).await?;

    let filter_all_active = query.active.is_none();
    let filter_active_active = query.active == Some(true);
    let filter_inactive_active = query.active == Some(false);

    let template = MemoriesTemplate {
        memories,
        filter_all_active,
        filter_active_active,
        filter_inactive_active,
    };

    Ok(Html(template.render()?))
}

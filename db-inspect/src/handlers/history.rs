use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::State, response::Html};
use std::sync::Arc;

use crate::db::AppState;
use crate::error::AppError;
use crate::models::ProfileHistoryEntry;

#[derive(Template, WebTemplate)]
#[template(path = "history.html")]
struct HistoryPage {
    entries: Vec<ProfileHistoryEntry>,
}

pub async fn history(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let entries = state.list_profile_history().await?;
    let template = HistoryPage { entries };
    Ok(Html(template.render()?))
}

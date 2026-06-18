use crate::db::AppState;
use crate::error::AppError;
use crate::models::DreamState;
use askama::Template;
use askama_web::WebTemplate;
use axum::extract::State;
use std::sync::Arc;

#[derive(Template, WebTemplate)]
#[template(path = "dream_state.html")]
pub struct DreamStatePage {
    entries: Vec<DreamState>,
}

pub async fn dream_state(State(state): State<Arc<AppState>>) -> Result<DreamStatePage, AppError> {
    let entries = state.list_dream_state().await?;
    Ok(DreamStatePage { entries })
}

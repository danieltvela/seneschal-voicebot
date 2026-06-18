use crate::db::AppState;
use crate::error::AppError;
use askama::Template;
use askama_web::WebTemplate;
use axum::extract::State;
use std::sync::Arc;

#[derive(Template, WebTemplate)]
#[template(path = "index.html")]
pub struct IndexPage {
    session_count: i64,
    message_count: i64,
    memory_count: i64,
    profile_count: i64,
    system_prompt_count: i64,
}

pub async fn home(State(state): State<Arc<AppState>>) -> Result<IndexPage, AppError> {
    let session_count = state.count_sessions().await?;
    let message_count = state.count_all_messages().await?;
    let memory_count = state.count_memories().await?;
    let profile_count = state.count_profile().await?;
    let system_prompt_count = state.count_system_prompts().await?;

    Ok(IndexPage {
        session_count,
        message_count,
        memory_count,
        profile_count,
        system_prompt_count,
    })
}

use crate::db::AppState;
use crate::error::AppError;
use crate::models::SystemPrompt;
use askama::Template;
use askama_web::WebTemplate;
use axum::{
    extract::{Form, Path, State},
    response::Redirect,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Template, WebTemplate)]
#[template(path = "system_prompts.html")]
pub struct SystemPromptsPage {
    prompts: Vec<SystemPrompt>,
}

#[derive(Deserialize)]
pub struct CreatePromptForm {
    pub session_id: String,
    pub content: String,
    pub active: Option<String>,
}

pub async fn list_system_prompts(
    State(state): State<Arc<AppState>>,
) -> Result<SystemPromptsPage, AppError> {
    let prompts = state.list_system_prompts().await?;
    Ok(SystemPromptsPage { prompts })
}

pub async fn create_system_prompt(
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreatePromptForm>,
) -> Result<Redirect, AppError> {
    let active = form.active.as_deref() == Some("on");
    state
        .create_system_prompt(&form.session_id, &form.content, active)
        .await?;
    Ok(Redirect::to("/system-prompts"))
}

pub async fn delete_system_prompt(
    Path(id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Result<Redirect, AppError> {
    state.delete_system_prompt(id).await?;
    Ok(Redirect::to("/system-prompts"))
}

pub async fn activate_system_prompt(
    Path(id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Result<Redirect, AppError> {
    state.activate_system_prompt(id).await?;
    Ok(Redirect::to("/system-prompts"))
}

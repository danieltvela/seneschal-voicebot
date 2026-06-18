use crate::db::AppState;
use crate::error::AppError;
use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Path, State};
use axum::response::Redirect;
use std::sync::Arc;

#[derive(Template, WebTemplate)]
#[template(path = "delete_session.html")]
pub struct DeleteSessionPage {
    session_id: String,
    message_count: i64,
}

pub async fn confirm_delete(
    Path(session_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<DeleteSessionPage, AppError> {
    let _session = state
        .get_session(&session_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let message_count = state.count_messages_for_session(&session_id).await?;
    Ok(DeleteSessionPage {
        session_id,
        message_count,
    })
}

pub async fn perform_delete(
    Path(session_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Redirect, AppError> {
    state.delete_session(&session_id).await?;
    Ok(Redirect::to("/sessions?deleted=1"))
}

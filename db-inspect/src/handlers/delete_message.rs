use crate::db::AppState;
use crate::error::AppError;
use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Path, State};
use axum::response::Redirect;
use std::sync::Arc;

#[derive(Template, WebTemplate)]
#[template(path = "delete_message.html")]
pub struct DeleteMessagePage {
    message_id: i64,
    content_preview: String,
}

pub async fn confirm_delete(
    Path(message_id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Result<DeleteMessagePage, AppError> {
    let msg = state
        .get_message(message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let preview = if msg.content.chars().count() > 200 {
        format!("{}...", msg.content.chars().take(200).collect::<String>())
    } else {
        msg.content.clone()
    };
    Ok(DeleteMessagePage {
        message_id,
        content_preview: preview,
    })
}

pub async fn perform_delete(
    Path(message_id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Result<Redirect, AppError> {
    state.delete_message(message_id).await?;
    Ok(Redirect::to("/messages?deleted=1"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn preview_truncates_unicode_safely() {
        let content = "áéíóú".repeat(50);
        let preview = if content.chars().count() > 200 {
            format!("{}...", content.chars().take(200).collect::<String>())
        } else {
            content.clone()
        };
        assert!(preview.chars().count() <= 203);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn preview_keeps_short_content_intact() {
        let content = "Brief message.".to_string();
        let preview = if content.chars().count() > 200 {
            format!("{}...", content.chars().take(200).collect::<String>())
        } else {
            content.clone()
        };
        assert_eq!(preview, "Brief message.");
    }
}

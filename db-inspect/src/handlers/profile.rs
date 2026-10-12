use crate::db::AppState;
use crate::error::AppError;
use crate::models::UserProfileEntry;
use askama::Template;
use askama_web::WebTemplate;
use axum::extract::State;
use axum::response::Html;
use std::sync::Arc;

#[derive(Template, WebTemplate)]
#[template(path = "profile.html")]
struct ProfilePage {
    entries: Vec<UserProfileEntry>,
}

pub async fn profile(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let entries = state.list_profile().await?;
    let template = ProfilePage { entries };
    Ok(Html(template.render()?))
}

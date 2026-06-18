use axum::{
    Router,
    routing::{get, post},
};
use std::sync::Arc;
use tower_http::services::ServeDir;

use crate::db::AppState;
use crate::handlers::{
    delete_message, delete_session, dream_state, history, home, memories, messages, profile,
    search, session_detail, sessions, system_prompts,
};

/// Build the application router with shared state.
pub fn create_router(app_state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(home::home))
        .route("/memories", get(memories::memories_handler))
        .route("/profile", get(profile::profile))
        .route("/search", get(search::search_handler))
        .route("/history", get(history::history))
        .route("/dream-state", get(dream_state::dream_state))
        .route("/sessions", get(sessions::sessions_handler))
        .route("/messages", get(messages::messages_handler))
        .route(
            "/sessions/{id}/delete",
            get(delete_session::confirm_delete).post(delete_session::perform_delete),
        )
        .route("/sessions/{id}", get(session_detail::session_detail))
        .route(
            "/messages/{id}/delete",
            get(delete_message::confirm_delete).post(delete_message::perform_delete),
        )
        .route(
            "/system-prompts",
            get(system_prompts::list_system_prompts).post(system_prompts::create_system_prompt),
        )
        .route(
            "/system-prompts/{id}/delete",
            post(system_prompts::delete_system_prompt),
        )
        .route(
            "/system-prompts/{id}/activate",
            post(system_prompts::activate_system_prompt),
        )
        .nest_service("/static", ServeDir::new("static"))
        .with_state(app_state)
}

use axum::http::StatusCode;
use axum::response::IntoResponse;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Not found")]
    NotFound,

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Template error: {0}")]
    Template(#[from] askama::Error),

    #[error("Internal server error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                axum::response::Html("<h1>404 Not Found</h1>"),
            )
                .into_response(),
            AppError::Database(_) | AppError::Internal(_) | AppError::Template(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::response::Html("<h1>500 Internal Server Error</h1>"),
            )
                .into_response(),
        }
    }
}

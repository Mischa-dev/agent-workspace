use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        error!(error = ?self, "request failed");

        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: "internal_error",
                    message: "The request could not be completed.",
                },
            }),
        )
            .into_response()
    }
}

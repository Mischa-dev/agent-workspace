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
    #[error("{0} was not found")]
    NotFound(&'static str),
    #[error("{0} conflict: {1}")]
    Conflict(&'static str, &'static str),
    #[error("invalid request: {0}")]
    Invalid(&'static str),
    #[error("development authentication is unavailable")]
    DevelopmentAuthUnavailable,
    #[error("internal invariant failed: {0}")]
    Internal(&'static str),
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
        let (status, code, message) = match self {
            Self::NotFound(_) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "The requested resource was not found.",
            ),
            Self::Conflict(_, _) => (
                StatusCode::CONFLICT,
                "conflict",
                "The request conflicts with current durable state.",
            ),
            Self::Invalid(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The request is invalid.",
            ),
            Self::DevelopmentAuthUnavailable => (
                StatusCode::FORBIDDEN,
                "development_auth_unavailable",
                "Development authentication is unavailable.",
            ),
            Self::Database(_) | Self::Internal(_) => {
                error!(error = ?self, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "The request could not be completed.",
                )
            }
        };

        (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody { code, message },
            }),
        )
            .into_response()
    }
}

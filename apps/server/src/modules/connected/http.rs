//! The one route a connected app calls without an account yet: it trades
//! the one-time install code Workspace sent to its install address for its
//! account's secret and the signing secrets (M1.12). Everything after that
//! it does as its account, through the ordinary API.

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::json;

use crate::{AppState, error::AppError};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/connected-modules/token", post(token))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenRequest {
    code: String,
}

async fn token(
    State(state): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> Result<Response, AppError> {
    let refused = || {
        (
            StatusCode::BAD_REQUEST,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({
                "error": "invalid_grant",
                "error_description": "That install code isn't valid: it was used, it lapsed, or the module was revoked.",
            })),
        )
            .into_response()
    };
    match super::exchange(&state, request.code.trim()).await? {
        Some(exchanged) => Ok((
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Json(exchanged),
        )
            .into_response()),
        None => Ok(refused()),
    }
}

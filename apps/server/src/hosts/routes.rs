//! A1.11: a host's own routes (`/api/v1/hosts/enrol`, `/api/v1/host/…`),
//! which carry its bearer token. Managing hosts (list, enrolment codes,
//! binding agents, unlinking) is the `host_*` operations in `hosts::ops`.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;

use super::{
    HostRow,
    protocol::{self, HostError},
};
use crate::{AppState, error::ApiJson};

pub const PROTOCOL_HEADER: &str = "aw-host-protocol";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/hosts/enrol", post(enrol_route))
        .route("/api/v1/host/describe", post(describe_route))
        .route("/api/v1/host/heartbeat", post(heartbeat_route))
        .route("/api/v1/host/notices", post(notice_route))
}

// ---------------------------------------------------------------- hosts

impl IntoResponse for HostError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Send the host's token as Authorization: Bearer <token>.".to_owned(),
            ),
            Self::Revoked => (
                StatusCode::UNAUTHORIZED,
                "host_revoked",
                "This host was unlinked. Enrol it again with a new code.".to_owned(),
            ),
            Self::CodeRefused => (
                StatusCode::UNAUTHORIZED,
                "enrolment_refused",
                "That code is wrong, used or expired. Ask for a new one.".to_owned(),
            ),
            Self::ProtocolUnsupported => (
                StatusCode::BAD_REQUEST,
                "protocol_unsupported",
                format!(
                    "This server speaks host protocol {}.",
                    super::PROTOCOLS.join(", ")
                ),
            ),
            Self::Invalid { field, message } => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "invalid",
                        "code": "invalid",
                        "field": field,
                        "message": message,
                    })),
                )
                    .into_response();
            }
            Self::AgentUnknown => (
                StatusCode::NOT_FOUND,
                "agent_unknown",
                "No agent with that hostAgentRef is bound to this host.".to_owned(),
            ),
            Self::App(error) => return error.into_response(),
        };
        (
            status,
            Json(json!({ "error": code, "code": code, "message": message })),
        )
            .into_response()
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn protocol_header(headers: &HeaderMap) -> Result<(), HostError> {
    protocol::check_protocol(
        headers
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok()),
    )
}

/// The host making this call, after the rate limit and protocol checks.
async fn host_caller(state: &AppState, headers: &HeaderMap) -> Result<HostRow, HostError> {
    let token = bearer(headers).ok_or(HostError::Unauthorized)?;
    state.rate_limits.check_mcp_auth(token)?;
    protocol_header(headers)?;
    protocol::authenticate(&state.pool, token).await
}

async fn enrol_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<protocol::EnrolRequest>,
) -> Result<(StatusCode, Json<protocol::Enrolled>), HostError> {
    state.rate_limits.check_mcp_auth(&request.code)?;
    protocol_header(&headers)?;
    Ok((
        StatusCode::CREATED,
        Json(protocol::enrol(&state.pool, request).await?),
    ))
}

async fn describe_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<protocol::DescribeRequest>,
) -> Result<Json<protocol::Described>, HostError> {
    let host = host_caller(&state, &headers).await?;
    Ok(Json(protocol::describe(&state.pool, &host, request).await?))
}

async fn heartbeat_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<protocol::HeartbeatRequest>,
) -> Result<Json<protocol::HeartbeatReply>, HostError> {
    let host = host_caller(&state, &headers).await?;
    Ok(Json(
        protocol::heartbeat(&state.pool, &host, request).await?,
    ))
}

async fn notice_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<protocol::NoticeRequest>,
) -> Result<Json<protocol::NoticeReply>, HostError> {
    let host = host_caller(&state, &headers).await?;
    Ok(Json(protocol::notice(&state.pool, &host, request).await?))
}

//! A1.11: the hosts routes. People's routes (`/api/v1/hosts…`) sign in as
//! usual and match the `host_*` MCP tools; a host's own routes
//! (`/api/v1/hosts/enrol`, `/api/v1/host/…`) carry its bearer token.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{
    EnrolmentCode, HostRow, HostView, Owner,
    protocol::{self, HostError},
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    policy::ActorKind,
};

pub const PROTOCOL_HEADER: &str = "aw-host-protocol";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/hosts", get(list_route))
        .route("/api/v1/hosts/enrolments", post(enrolment_route))
        .route("/api/v1/hosts/{host_id}/agents", post(bind_route))
        .route(
            "/api/v1/hosts/{host_id}/agents/{account_id}",
            delete(unbind_route),
        )
        .route("/api/v1/hosts/{host_id}/unlink", post(unlink_route))
        .route("/api/v1/hosts/enrol", post(enrol_route))
        .route("/api/v1/host/describe", post(describe_route))
        .route("/api/v1/host/heartbeat", post(heartbeat_route))
        .route("/api/v1/host/notices", post(notice_route))
}

fn kind_of(principal: &AuthenticatedActor) -> ActorKind {
    let person = principal.actor_kind == "human"; // kind-check: mechanical (maps the principal onto the policy kind)
    if person {
        ActorKind::Human
    } else {
        ActorKind::ExternalAgent
    }
}

// ---------------------------------------------------------------- people

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    workspace_id: Option<Uuid>,
}

async fn list_route(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let hosts = super::list(&mut connection, principal.actor_id, query.workspace_id).await?;
    Ok(Json(json!({ "hosts": hosts })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnrolmentRequest {
    /// The workspace that will own the host; left out, the caller does.
    workspace_id: Option<Uuid>,
    label: Option<String>,
}

async fn enrolment_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<EnrolmentRequest>,
) -> Result<(StatusCode, Json<EnrolmentCode>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let owner = request
        .workspace_id
        .map_or(Owner::Account(principal.actor_id), Owner::Workspace);
    let mut transaction = state.pool.begin().await?;
    let code = super::create_enrolment(
        &mut transaction,
        principal.actor_id,
        kind_of(&principal),
        owner,
        request.label.as_deref(),
    )
    .await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(code)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindRequest {
    account_id: Uuid,
    host_agent_ref: String,
}

async fn bind_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(host_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<BindRequest>,
) -> Result<Json<HostView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let host = super::bind(
        &mut transaction,
        principal.actor_id,
        host_id,
        request.account_id,
        &request.host_agent_ref,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(host))
}

async fn unbind_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((host_id, account_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<HostView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let host = super::unbind(&mut transaction, principal.actor_id, host_id, account_id).await?;
    transaction.commit().await?;
    Ok(Json(host))
}

async fn unlink_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(host_id): ApiPath<Uuid>,
) -> Result<Json<HostView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let host = super::unlink(&mut transaction, principal.actor_id, host_id).await?;
    transaction.commit().await?;
    Ok(Json(host))
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

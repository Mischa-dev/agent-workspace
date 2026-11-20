//! A1.11: a host's own routes (`/api/v1/hosts/enrol`, `/api/v1/host/…`),
//! which carry its bearer token. Managing hosts (list, enrolment codes,
//! binding agents, unlinking) is the `host_*` operations in `hosts::ops`.

use std::{convert::Infallible, time::Duration};

use agent_workspace_host_protocol::{Ack, AckReply, CommandList};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header::ACCEPT},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::stream::{self, Stream};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::builtin::install::BuiltinCall;
use super::{
    HostRow, commands,
    protocol::{self, HostError},
};
use crate::{
    AppState,
    error::{ApiJson, AppError},
};

pub const PROTOCOL_HEADER: &str = "aw-host-protocol";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/hosts/enrol", post(enrol_route))
        .route("/api/v1/host/builtin/enrol", post(builtin_enrol_route))
        .route("/api/v1/host/describe", post(describe_route))
        .route("/api/v1/host/heartbeat", post(heartbeat_route))
        .route("/api/v1/host/windows", post(windows_route))
        .route("/api/v1/host/notices", post(notice_route))
        .route("/api/v1/host/agents/register", post(register_route))
        .route("/api/v1/host/runs", post(adopt_route))
        .route("/api/v1/host/runs/{run_id}/model", post(model_route))
        .route("/api/v1/host/runs/{run_id}/stream", post(stream_route))
        .route("/api/v1/host/commands", get(commands_route))
        .route("/api/v1/host/commands/{command_id}", get(command_route))
        .route("/api/v1/host/commands/{command_id}/ack", post(ack_route))
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
            Self::CommandUnknown => (
                StatusCode::NOT_FOUND,
                "command_unknown",
                "This host has no command with that id.".to_owned(),
            ),
            Self::CommandClosed => (
                StatusCode::GONE,
                "command_closed",
                "That command expired or was replaced; don't carry it out.".to_owned(),
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
    state.rate_limits.check_mcp_auth(token).await?;
    protocol_header(headers)?;
    let host = protocol::authenticate(&state.pool, token).await?;
    crate::tenancy::signed_in_as(crate::tenancy::Tenancy::System("hosts.protocol"));
    Ok(host)
}

/// The host a call about one command comes from: the caller's own, or, for
/// the built-in host (A1.16), the built-in row the command was written for.
async fn command_caller(
    state: &AppState,
    builtin: BuiltinCall,
    headers: &HeaderMap,
    command_id: Uuid,
) -> Result<HostRow, HostError> {
    if builtin.is_none() {
        return host_caller(state, headers).await;
    }
    protocol_header(headers)?;
    // The built-in host, calling in process: its commands are of every
    // workspace, as any host's are.
    crate::tenancy::signed_in_as(crate::tenancy::Tenancy::System("hosts.protocol"));
    let mut transaction = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(AppError::from)?;
    let host_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT command.host_id FROM host_commands command \
         JOIN hosts host ON host.id = command.host_id AND host.builtin \
         WHERE command.id = $1",
    )
    .bind(command_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(AppError::from)?
    .ok_or(HostError::CommandUnknown)?;
    let host = super::get(&mut transaction, host_id)
        .await?
        .ok_or(HostError::CommandUnknown)?;
    transaction.commit().await.map_err(AppError::from)?;
    Ok(host)
}

async fn enrol_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<protocol::EnrolRequest>,
) -> Result<(StatusCode, Json<protocol::Enrolled>), HostError> {
    // The one-time code, not a session, names the workspace: it is known only once the code is read.
    crate::tenancy::system(
        "hosts.protocol",
        Box::pin(enrol_route_here(State(state), headers, ApiJson(request))),
    )
    .await
}

async fn enrol_route_here(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<protocol::EnrolRequest>,
) -> Result<(StatusCode, Json<protocol::Enrolled>), HostError> {
    state.rate_limits.check_mcp_auth(&request.code).await?;
    protocol_header(&headers)?;
    Ok((
        StatusCode::CREATED,
        Json(protocol::enrol(&state, request).await?),
    ))
}

/// A1.17: the built-in host program's first start, its one-time code for
/// the install's token.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuiltinEnrolRequest {
    code: String,
}

async fn builtin_enrol_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<BuiltinEnrolRequest>,
) -> Result<(StatusCode, Json<Value>), HostError> {
    state.rate_limits.check_mcp_auth(&request.code).await?;
    protocol_header(&headers)?;
    let token = super::builtin::install::enrol(&state.pool, &request.code).await?;
    Ok((StatusCode::CREATED, Json(json!({ "token": token }))))
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

/// A1.24: the plan windows of the accounts the host described.
async fn windows_route(
    State(state): State<AppState>,
    builtin: BuiltinCall,
    headers: HeaderMap,
    ApiJson(request): ApiJson<super::windows::WindowsReport>,
) -> Result<Json<super::windows::WindowsStored>, HostError> {
    use super::windows::{Reporter, report};
    if builtin.is_some() {
        protocol_header(&headers)?;
        // The built-in host, calling in process, about accounts of every
        // workspace.
        crate::tenancy::signed_in_as(crate::tenancy::Tenancy::System("hosts.protocol"));
        return Ok(Json(report(&state, Reporter::Builtin, request).await?));
    }
    let host = host_caller(&state, &headers).await?;
    Ok(Json(report(&state, Reporter::Host(&host), request).await?))
}

/// A2.26: a run the host started itself, adopted once.
async fn adopt_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<super::adopt::AdoptRequest>,
) -> Result<(StatusCode, Json<super::adopt::Adopted>), HostError> {
    let host = host_caller(&state, &headers).await?;
    let adopted = super::adopt::adopt(&state, &host, request).await?;
    let status = if adopted.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(adopted)))
}

/// A1.25: one model turn for the built-in assistant's run, by the run's
/// credential (see `builtin::assistant`).
async fn model_route(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    headers: HeaderMap,
    ApiJson(turn): ApiJson<agent_workspace_host_protocol::ModelTurn>,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return super::builtin::assistant::ModelRefusal::Unauthorized.into_response();
    };
    if let Err(error) = state.rate_limits.check_mcp_auth(token).await {
        return error.into_response();
    }
    // The run's credential is read before its account is known, as the
    // other host calls read theirs.
    let turn = super::builtin::assistant::model_turn(&state, token, run_id, turn);
    match crate::tenancy::system("hosts.protocol", turn).await {
        Ok(reply) => Json(reply).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// A1.17: a run's reply as it arrives, shown live and never stored.
async fn stream_route(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    headers: HeaderMap,
    ApiJson(delta): ApiJson<agent_workspace_host_protocol::StreamDelta>,
) -> Result<(StatusCode, Json<Value>), HostError> {
    let token = bearer(&headers).ok_or(HostError::Unauthorized)?;
    state.rate_limits.check_run_stream(token).await?;
    // The run's credential is read before its account is known, as the
    // other host calls read theirs.
    let delta = super::stream::delta(&state, token, run_id, delta);
    match crate::tenancy::system("hosts.protocol", delta).await {
        Ok(()) => Ok((StatusCode::ACCEPTED, Json(json!({})))),
        Err(HostError::Unauthorized) => {
            state.rate_limits.note_mcp_auth_failure().await;
            Err(HostError::Unauthorized)
        }
        Err(error) => Err(error),
    }
}

async fn notice_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<protocol::NoticeRequest>,
) -> Result<Json<protocol::NoticeReply>, HostError> {
    let host = host_caller(&state, &headers).await?;
    Ok(Json(protocol::notice(&state.pool, &host, request).await?))
}

/// A2.19: the host proposes an agent it made, for a person to approve.
async fn register_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<super::register::RegisterRequest>,
) -> Result<(StatusCode, Json<super::register::Registered>), HostError> {
    let host = host_caller(&state, &headers).await?;
    let registered = super::register::propose(&state, &host, request).await?;
    let status = if registered.status == "proposed" {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(registered)))
}

// ------------------------------------------------------------- commands

#[derive(Debug, Default, Deserialize)]
struct CommandsQuery {
    /// The `next` of the last answer (0 at first).
    after: Option<i64>,
    /// Seconds to wait for a command (at most 30).
    wait: Option<u64>,
}

/// How long one stream stays open before the host reconnects (with
/// `Last-Event-ID`), so no request holds a server for long.
const STREAM_FOR: Duration = Duration::from_secs(300);

/// A1.12: the host's commands. A long-poll by default; with `Accept:
/// text/event-stream`, a stream of `host.command` events from `after` (or
/// `Last-Event-ID`). Either counts as a heartbeat.
async fn commands_route(
    State(state): State<AppState>,
    builtin: BuiltinCall,
    headers: HeaderMap,
    Query(query): Query<CommandsQuery>,
) -> Result<Response, HostError> {
    if builtin.is_some() {
        // A1.16: the built-in host, for every workspace at once; it reads
        // in process and never waits here.
        protocol_header(&headers)?;
        crate::tenancy::signed_in_as(crate::tenancy::Tenancy::System("hosts.protocol"));
        let after = query.after.unwrap_or(0).max(0);
        return Ok(Json(commands::due_builtin(&state.pool, after).await?).into_response());
    }
    let host = host_caller(&state, &headers).await?;
    let resume = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok());
    let after = resume.or(query.after).unwrap_or(0).max(0);
    let streamed = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"));
    if streamed {
        return Ok(Sse::new(command_stream(state, host, after))
            .keep_alive(KeepAlive::default())
            .into_response());
    }
    let listed: CommandList =
        commands::long_poll(&state.pool, &host, after, query.wait.unwrap_or(0)).await?;
    Ok(Json(listed).into_response())
}

fn command_stream(
    state: AppState,
    host: HostRow,
    after: i64,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let until = tokio::time::Instant::now() + STREAM_FOR;
    stream::unfold(
        (state, host, after, Vec::<Event>::new()),
        move |(state, host, mut after, mut queued)| async move {
            loop {
                if let Some(event) = queued.pop() {
                    return Some((Ok(event), (state, host, after, queued)));
                }
                if tokio::time::Instant::now() >= until {
                    return None;
                }
                // Polled outside the request that opened it.
                let listed = crate::tenancy::system(
                    "hosts.protocol",
                    commands::long_poll(&state.pool, &host, after, 15),
                )
                .await
                .ok()?;
                after = listed.next;
                // Popped from the end, so queued newest first.
                queued = listed
                    .commands
                    .iter()
                    .rev()
                    .filter_map(|command| {
                        Event::default()
                            .id(command.seq.to_string())
                            .event("host.command")
                            .json_data(command)
                            .ok()
                    })
                    .collect();
            }
        },
    )
}

/// A1.12: one command whole, with any credential minted for it.
async fn command_route(
    State(state): State<AppState>,
    builtin: BuiltinCall,
    headers: HeaderMap,
    Path(command_id): Path<Uuid>,
) -> Result<Json<Value>, HostError> {
    let host = command_caller(&state, builtin, &headers, command_id).await?;
    Ok(Json(commands::fetch(&state, &host, command_id).await?))
}

/// A1.12: the host's answer to a command.
async fn ack_route(
    State(state): State<AppState>,
    builtin: BuiltinCall,
    headers: HeaderMap,
    Path(command_id): Path<Uuid>,
    ApiJson(ack): ApiJson<Ack>,
) -> Result<Json<AckReply>, HostError> {
    let host = command_caller(&state, builtin, &headers, command_id).await?;
    Ok(Json(
        commands::ack(&state.pool, &host, command_id, ack).await?,
    ))
}

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

use super::{
    HostRow, commands,
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
        Json(protocol::enrol(&state, request).await?),
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
    headers: HeaderMap,
    Query(query): Query<CommandsQuery>,
) -> Result<Response, HostError> {
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
                let listed = commands::long_poll(&state.pool, &host, after, 15).await.ok()?;
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
    headers: HeaderMap,
    Path(command_id): Path<Uuid>,
) -> Result<Json<Value>, HostError> {
    let host = host_caller(&state, &headers).await?;
    Ok(Json(commands::fetch(&state, &host, command_id).await?))
}

/// A1.12: the host's answer to a command.
async fn ack_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(command_id): Path<Uuid>,
    ApiJson(ack): ApiJson<Ack>,
) -> Result<Json<AckReply>, HostError> {
    let host = host_caller(&state, &headers).await?;
    Ok(Json(commands::ack(&state.pool, &host, command_id, ack).await?))
}

//! The HTTP edge (ruling R4): one generated route for the web app and for
//! scripts.
//!
//! - `POST /api/v2/w/{workspace}/ops/{name}`: an operation in a workspace,
//!   named by its id or slug. A target in another workspace is `not_found`.
//! - `POST /api/v2/ops/{name}`: an operation whose target says where it acts
//!   (or, for a caller in one workspace, one that needs none named).
//!
//! The body is the operation's input. A session cookie (with the CSRF
//! header) makes the web edge, a bearer token the public API edge; a request
//! carrying both is refused, so a script's token never rides on a browser's
//! session or the other way round. A write's request id is the
//! `Idempotency-Key` header; `?preview=true` runs an operation that has a
//! preview and rolls it back. The answer is the envelope, with the status
//! its outcome maps to. Failing to sign in or the CSRF check answers as
//! every other route does.

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::post,
};

use super::{
    Caller, DispatchRequest, Edge, OpError,
    dispatch::{Answer, dispatch},
    registry, target,
};
use crate::{AppState, auth, error::AppError, mcp};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v2/w/{workspace}/ops/{name}",
            post(workspace_operation),
        )
        .route("/api/v2/ops/{name}", post(operation))
}

async fn workspace_operation(
    State(state): State<AppState>,
    Path((workspace, name)): Path<(String, String)>,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    call(&state, Some(&workspace), &name, &uri, &headers, body).await
}

async fn operation(
    State(state): State<AppState>,
    Path(name): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    call(&state, None, &name, &uri, &headers, body).await
}

fn answer(answer: Answer) -> Response {
    let retry_after = answer.body["details"]["retryAfterSeconds"]
        .as_u64()
        .filter(|_| answer.status == StatusCode::TOO_MANY_REQUESTS);
    let mut response = (answer.status, Json(answer.body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(seconds) = retry_after
        && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
    {
        headers.insert(header::RETRY_AFTER, value);
    }
    response
}

fn refused(error: &OpError) -> Response {
    answer(Answer::error(error))
}

/// Who is calling, and through which edge.
async fn caller(
    state: &AppState,
    headers: &HeaderMap,
    writes: bool,
) -> Result<(Caller, Edge), Response> {
    let bearer = headers.contains_key(header::AUTHORIZATION);
    if bearer && auth::has_session_cookie(&state.config, headers) {
        return Err(refused(&OpError::invalid(
            "authorization",
            "Send a session or a bearer token, not both.",
        )));
    }
    if bearer {
        let key = mcp::exact_bearer_token(headers).unwrap_or("invalid");
        state
            .rate_limits
            .check_mcp_auth(key)
            .map_err(IntoResponse::into_response)?;
        let principal = match mcp::resolve_bearer_principal(state, headers).await {
            Ok(Some(principal)) => principal,
            Ok(None) => return Err(AppError::AuthenticationRequired.into_response()),
            Err(error) => return Err(refused(&OpError::logged("authenticate", &error))),
        };
        state
            .rate_limits
            .check_mcp_request(principal.credential_id)
            .map_err(IntoResponse::into_response)?;
        let caller = super::mcp::caller_of(state, &principal)
            .await
            .map_err(|error| refused(&error))?;
        return Ok((caller, Edge::of_public_api(&principal)));
    }
    let principal = match auth::load_authenticated_actor(state, headers).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return Err(AppError::AuthenticationRequired.into_response()),
        Err(error) => return Err(error.into_response()),
    };
    if writes {
        auth::require_authenticated_write(state, headers, &principal)
    } else {
        auth::require_csrf(&state.config, headers, &principal)
    }
    .map_err(IntoResponse::into_response)?;
    let kind = super::context::actor_kind_of(&principal.actor_kind)
        .ok_or_else(|| AppError::AuthenticationRequired.into_response())?;
    Ok((
        Caller::person(principal.actor_id, kind),
        Edge::Web {
            session_id: principal.session_id,
        },
    ))
}

async fn call(
    state: &AppState,
    workspace: Option<&str>,
    name: &str,
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let Some(meta) = registry().get(name).map(|entry| entry.meta.clone()) else {
        return refused(&OpError::not_found("That operation"));
    };
    let (caller, edge) = match caller(state, headers, meta.kind.writes()).await {
        Ok(found) => found,
        Err(response) => return response,
    };
    let Ok(bytes) = to_bytes(body, meta.max_body_bytes).await else {
        return refused(&OpError::TooLarge {
            limit_bytes: meta.max_body_bytes,
        });
    };
    let input = if bytes.iter().all(u8::is_ascii_whitespace) {
        serde_json::json!({})
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(input) => input,
            Err(_) => {
                return refused(&OpError::invalid("body", "The body must be a JSON object."));
            }
        }
    };
    let workspace = match workspace {
        Some(named) => {
            let mut connection = match state.pool.acquire().await {
                Ok(connection) => connection,
                Err(error) => return refused(&OpError::logged(meta.name, &error.into())),
            };
            match target::workspace_named(&mut connection, named).await {
                Ok(id) => Some(id),
                Err(error) => return refused(&error),
            }
        }
        None => None,
    };
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let preview = uri.query().is_some_and(|query| {
        query
            .split('&')
            .any(|pair| pair == "preview=true" || pair == "preview=1")
    });
    let request = DispatchRequest {
        caller,
        edge,
        trace_id: text(crate::REQUEST_ID_HEADER.as_str()),
        workspace,
        request_id: text("idempotency-key"),
        preview,
    };
    answer(dispatch(state, request, name, input).await)
}

//! The routes a connected app calls with its own signature rather than an
//! account: trading the one-time install code Workspace sent to its install
//! address for its account's secret and the signing secrets (M1.12), and
//! sending in the events its manifest declares (M1.13). Everything else it
//! does as its account, through the ordinary API.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, error::AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/connected-modules/token", post(token))
        .route("/api/v1/modules/{module_id}/events", post(event_in))
        .route(
            "/api/v1/modules/{module_id}/frames/redeem",
            post(redeem_frame),
        )
        .route("/panel-frame/{launch_id}", get(super::frames::open_host))
        .route("/panel-frame.css", get(super::frames::host_css))
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

// ------------------------------------------------------------ events in

/// The largest event body taken.
const MAX_EVENT_BYTES: usize = 64 * 1024;
/// How far a signature's time may be from now.
const TOLERANCE_SECONDS: i64 = 5 * 60;
/// The largest `data` an event carries into the log.
const MAX_DATA_BYTES: usize = 2048;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventIn {
    /// A type the manifest's `events.emits` declares.
    #[serde(rename = "type")]
    ty: String,
    /// The thing it is about: one of the install's own things.
    #[serde(default)]
    thing: Option<ThingIn>,
    /// Small values for rules to read: ids, keys, numbers. No text bodies.
    #[serde(default)]
    data: Option<serde_json::Map<String, Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThingIn {
    id: Uuid,
}

fn answer(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Whether `data` is small and flat: scalars only, so no text body rides
/// into the log as a nested document.
fn data_fits(data: &serde_json::Map<String, Value>) -> bool {
    data.len() <= 32
        && serde_json::to_vec(data).is_ok_and(|bytes| bytes.len() <= MAX_DATA_BYTES)
        && data.values().all(|value| match value {
            Value::String(text) => text.chars().count() <= 200,
            Value::Number(_) | Value::Bool(_) | Value::Null => true,
            _ => false,
        })
}

/// `POST /api/v1/modules/{module_id}/events`: one event from a connected
/// module, signed with its inbound secret (Standard Webhooks). A signature
/// more than five minutes from now, or a `webhook-id` it already sent, is
/// refused; so is a type its manifest doesn't declare, or a thing that
/// isn't one of its own. The event goes into the log like any other, so
/// rules, webhooks and agents can follow it.
async fn event_in(
    State(state): State<AppState>,
    Path(module_id): Path<Uuid>,
    headers: HeaderMap,
    body: String,
) -> Result<Response, AppError> {
    let unsigned = || {
        answer(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            "The event isn't signed with this module's inbound secret, or the signature is too old.",
        )
    };
    if body.len() > MAX_EVENT_BYTES {
        return Ok(answer(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "An event is at most 64 KB.",
        ));
    }
    let (Some(id), Some(timestamp), Some(signature)) = (
        header_text(&headers, "webhook-id"),
        header_text(&headers, "webhook-timestamp"),
        header_text(&headers, "webhook-signature"),
    ) else {
        return Ok(unsigned());
    };
    let fresh = timestamp
        .parse::<i64>()
        .is_ok_and(|signed| (chrono::Utc::now().timestamp() - signed).abs() <= TOLERANCE_SECONDS);
    if !fresh || id.is_empty() || id.len() > 200 {
        return Ok(unsigned());
    }
    let workspace = crate::tenancy::system(
        "modules.events_in",
        crate::db::scoped_one(&state.pool, async |connection| {
            Ok::<_, AppError>(
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT workspace_id FROM module_apps WHERE module_id = $1 AND state = 'active'",
                )
                .bind(module_id)
                .fetch_optional(&mut *connection)
                .await?,
            )
        }),
    )
    .await?;
    let Some(workspace) = workspace else {
        return Ok(unsigned());
    };
    crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: None,
            workspace,
        },
        accept(&state, workspace, module_id, id, timestamp, signature, body),
    )
    .await
}

async fn accept(
    state: &AppState,
    workspace: Uuid,
    module_id: Uuid,
    webhook_id: &str,
    timestamp: &str,
    signature: &str,
    body: String,
) -> Result<Response, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(row) = super::get(&mut transaction, workspace, module_id, false).await? else {
        return Ok(answer(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            "Unknown module.",
        ));
    };
    let Some(inbound) = row.inbound_secret_id.filter(|_| row.state == "active") else {
        return Ok(answer(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            "Unknown module.",
        ));
    };
    let keys = crate::secrets::keyring(&state.config)?;
    let secret = crate::secrets::reveal(
        &mut *transaction,
        keys.as_ref(),
        inbound,
        "connected module event",
    )
    .await?;
    let signed_at = timestamp.parse::<i64>().unwrap_or_default();
    if !agent_workspace_host_protocol::verify_push(
        secret.expose(),
        webhook_id,
        signed_at,
        &body,
        signature,
        chrono::Utc::now().timestamp(),
    ) {
        return Ok(answer(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            "The event isn't signed with this module's inbound secret.",
        ));
    }
    let Ok(event) = serde_json::from_str::<EventIn>(&body) else {
        return Ok(answer(
            StatusCode::BAD_REQUEST,
            "invalid_event",
            "An event is {\"type\", \"thing\": {\"id\"}, \"data\"}.",
        ));
    };
    let manifest = row.manifest()?;
    let Some(declared) = manifest
        .events
        .emits
        .iter()
        .find(|declared| declared.event_type == event.ty)
    else {
        return Ok(answer(
            StatusCode::BAD_REQUEST,
            "undeclared_type",
            "The manifest doesn't declare that event type.",
        ));
    };
    if event.data.as_ref().is_some_and(|data| !data_fits(data)) {
        return Ok(answer(
            StatusCode::BAD_REQUEST,
            "invalid_event",
            "data is at most 32 values of ids, keys, numbers or short strings, 2 KB in all.",
        ));
    }
    // About a thing: one of this install's own, of the declared kind.
    let thing = match (&declared.kind, &event.thing) {
        (Some(kind), Some(thing)) => {
            match super::things::owned_thing(&mut transaction, workspace, module_id, kind, thing.id)
                .await?
            {
                Some(space) => Some((thing.id, kind.as_str(), space)),
                None => {
                    return Ok(answer(
                        StatusCode::BAD_REQUEST,
                        "unknown_thing",
                        "The thing isn't one this module added, of the kind the event declares.",
                    ));
                }
            }
        }
        (Some(_), None) => {
            return Ok(answer(
                StatusCode::BAD_REQUEST,
                "invalid_event",
                "This event type is about a thing: give thing.id.",
            ));
        }
        (None, Some(_)) => {
            return Ok(answer(
                StatusCode::BAD_REQUEST,
                "invalid_event",
                "This event type isn't about a thing.",
            ));
        }
        (None, None) => None,
    };
    // Once only: a receipt per webhook-id, kept past the five minutes a
    // signature is good for.
    sqlx::query(
        "DELETE FROM module_event_receipts \
         WHERE module_id = $1 AND received_at < now() - interval '15 minutes'",
    )
    .bind(module_id)
    .execute(&mut *transaction)
    .await?;
    let first = sqlx::query(
        "INSERT INTO module_event_receipts (workspace_id, module_id, webhook_id) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(workspace)
    .bind(module_id)
    .bind(webhook_id)
    .execute(&mut *transaction)
    .await?
    .rows_affected()
        == 1;
    if !first {
        return Ok(answer(
            StatusCode::CONFLICT,
            "replayed",
            "That webhook-id was already sent.",
        ));
    }
    let ty = &crate::events::catalog::MODULE_EVENT;
    let new = match thing {
        Some((id, kind, Some(space))) => crate::events::NewEvent::thing(ty, space, kind, id),
        Some((id, kind, None)) => workspace_event(ty, workspace, Some((id, kind))),
        None => workspace_event(ty, workspace, None),
    };
    let payload = json!({
        "moduleId": module_id,
        "type": event.ty,
        "data": event.data.unwrap_or_default(),
    });
    let id =
        crate::events::emit(&mut transaction, new.actor(row.account_id).payload(payload)).await?;
    transaction.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "accepted": true, "id": id })),
    )
        .into_response())
}

// ------------------------------------------------------------ frames

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeemRequest {
    token: String,
}

/// `POST /api/v1/modules/{module_id}/frames/redeem`: a connected module
/// trades the launch token its framed page received for who it is drawing
/// for (M1.15). Signed as its events are; a token redeems once, only after
/// the viewer's page opened it, and lapses five minutes after the panel
/// asked for the frame.
async fn redeem_frame(
    State(state): State<AppState>,
    Path(module_id): Path<Uuid>,
    headers: HeaderMap,
    body: String,
) -> Result<Response, AppError> {
    let unsigned = || {
        answer(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            "The request isn't signed with this module's inbound secret, or the signature is too old.",
        )
    };
    if body.len() > 4096 {
        return Ok(answer(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "A redemption is a token.",
        ));
    }
    let (Some(id), Some(timestamp), Some(signature)) = (
        header_text(&headers, "webhook-id"),
        header_text(&headers, "webhook-timestamp"),
        header_text(&headers, "webhook-signature"),
    ) else {
        return Ok(unsigned());
    };
    let signed_at = timestamp.parse::<i64>().unwrap_or_default();
    if (chrono::Utc::now().timestamp() - signed_at).abs() > TOLERANCE_SECONDS
        || id.is_empty()
        || id.len() > 200
    {
        return Ok(unsigned());
    }
    let workspace = crate::tenancy::system(
        "modules.frame_redeem",
        crate::db::scoped_one(&state.pool, async |connection| {
            Ok::<_, AppError>(
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT workspace_id FROM module_apps WHERE module_id = $1 AND state = 'active'",
                )
                .bind(module_id)
                .fetch_optional(&mut *connection)
                .await?,
            )
        }),
    )
    .await?;
    let Some(workspace) = workspace else {
        return Ok(unsigned());
    };
    crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: None,
            workspace,
        },
        async {
            let mut transaction = crate::db::scoped_tx(&state.pool).await?;
            let Some(row) = super::get(&mut transaction, workspace, module_id, false).await? else {
                return Ok(unsigned());
            };
            let Some(inbound) = row.inbound_secret_id.filter(|_| row.state == "active") else {
                return Ok(unsigned());
            };
            let keys = crate::secrets::keyring(&state.config)?;
            let secret = crate::secrets::reveal(
                &mut *transaction,
                keys.as_ref(),
                inbound,
                "connected module frame",
            )
            .await?;
            if !agent_workspace_host_protocol::verify_push(
                secret.expose(),
                id,
                signed_at,
                &body,
                signature,
                chrono::Utc::now().timestamp(),
            ) {
                return Ok(unsigned());
            }
            let Ok(request) = serde_json::from_str::<RedeemRequest>(&body) else {
                return Ok(answer(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "A redemption is {\"token\"}.",
                ));
            };
            let redeemed =
                super::frames::redeem(&mut transaction, workspace, module_id, &request.token)
                    .await?;
            transaction.commit().await?;
            Ok(match redeemed {
                Some(redeemed) => (
                    StatusCode::OK,
                    [(header::CACHE_CONTROL, "no-store")],
                    Json(redeemed),
                )
                    .into_response(),
                None => answer(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "That launch token isn't valid: it was used, it lapsed, or its page was never opened.",
                ),
            })
        },
    )
    .await
}

/// An event heard across the workspace (its thing, if any, is in no space).
fn workspace_event<'a>(
    ty: &'static crate::events::catalog::EventType,
    workspace: Uuid,
    thing: Option<(Uuid, &'a str)>,
) -> crate::events::NewEvent<'a> {
    let mut event = match thing {
        Some((id, kind)) => crate::events::NewEvent::thing(ty, workspace, kind, id),
        None => crate::events::NewEvent::thing(ty, workspace, "", Uuid::nil()),
    };
    if thing.is_none() {
        event.thing = None;
    }
    event.space_id = None;
    event
        .audience(crate::events::Audience::Workspace)
        .workspace(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_small_and_flat() {
        let mut data = serde_json::Map::new();
        data.insert("build".into(), json!("214"));
        data.insert("seconds".into(), json!(81));
        assert!(data_fits(&data));
        data.insert("log".into(), json!({ "lines": ["a"] }));
        assert!(!data_fits(&data));
        let mut long = serde_json::Map::new();
        long.insert("text".into(), json!("x".repeat(201)));
        assert!(!data_fits(&long));
    }
}

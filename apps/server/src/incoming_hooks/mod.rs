//! Incoming webhooks (K3.26): an outside service starts an automation.
//!
//! A hook is an address in one space with its own signing secret, kept in
//! the secrets store and shown once, to whoever made it
//! (`incoming_hook_create`). A sender posts to
//! `POST /api/v1/incoming-hooks/{id}` with three headers:
//!
//! - `X-Hook-Delivery`: its own id for the delivery, so a retry is
//!   recognised;
//! - `X-Hook-Timestamp`: Unix seconds, within five minutes of now;
//! - `X-Hook-Signature`: `sha256=` and the hex HMAC-SHA256, keyed by the
//!   secret, of `<timestamp>.<delivery id>.<body>`, so neither the time
//!   nor the delivery id can be changed to replay a body.
//!
//! A delivery that checks out is stored with its body and becomes an
//! `incoming_hook.received` fact in the log carrying the hook's id and the
//! delivery's, never the body (AGENTS.md rule 10); rules triggered by the
//! hook read the body live from the delivery, in their own space. The same
//! delivery id is accepted once per hook, so a retry runs nothing again. A
//! delivery that doesn't check out is refused and kept without its body,
//! with the reason, so the hook's owner can see what was turned away.
//! Deliveries are counted per hook and per workspace in the shared limits.

pub mod ops;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiPath, AppError},
    secrets::{self, NewSecret, SecretOwner, SecretValue},
};

pub const DELIVERY_HEADER: &str = "x-hook-delivery";
pub const TIMESTAMP_HEADER: &str = "x-hook-timestamp";
pub const SIGNATURE_HEADER: &str = "x-hook-signature";
/// The largest body a hook takes.
pub const MAX_BODY_BYTES: usize = 65_536;
/// How far a delivery's timestamp may be from now.
const MAX_SKEW_SECONDS: i64 = 300;
/// How much of the body `{{delivery.text}}` gives a rule.
const TEXT_CHARACTERS: usize = 4_000;
/// The most recent deliveries a hook's listing shows.
const RECENT_DELIVERIES: i64 = 10;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/incoming-hooks/{hook_id}", post(receive))
}

/// A hook, as its space's members see it. Never its secret.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub name: String,
    /// Where the sender posts.
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub last_delivery_at: Option<DateTime<Utc>>,
    /// The latest deliveries, accepted or refused, newest first.
    pub recent: Vec<HookDeliveryView>,
}

/// One delivery: when, its id, and whether it was taken. Never its body.
#[derive(Clone, Debug, Serialize, JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct HookDeliveryView {
    #[serde(skip)]
    #[schemars(skip)]
    pub hook_id: Uuid,
    /// The sender's id for it.
    #[sqlx(rename = "delivery_key")]
    pub delivery_id: String,
    /// `accepted` or `refused`.
    pub outcome: String,
    /// Why it was refused: `no_delivery_id`, `stale_timestamp`,
    /// `bad_signature` or `too_large`.
    pub reason: Option<String>,
    pub received_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct HookRow {
    id: Uuid,
    space_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    last_delivery_at: Option<DateTime<Utc>>,
}

fn secret_name(hook_id: Uuid) -> String {
    format!("incoming_hook:{hook_id}")
}

pub(crate) fn url(state: &AppState, hook_id: Uuid) -> String {
    format!(
        "{}/api/v1/incoming-hooks/{hook_id}",
        crate::events::public_origin(state)
    )
}

/// A one-line name, 1 to 120 characters.
pub(crate) fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name.chars().count() <= 120 && !name.chars().any(char::is_control))
        .then(|| name.to_owned())
}

/// Makes a hook in `space` and its secret, which is returned only here.
pub(crate) async fn create(
    connection: &mut PgConnection,
    state: &AppState,
    workspace: Uuid,
    space: Uuid,
    account: Uuid,
    name: &str,
    request_id: Option<&str>,
) -> Result<(Uuid, String), AppError> {
    use rand_core::RngCore;
    let keys = secrets::keyring(&state.config)?;
    let id = Uuid::now_v7();
    let mut bytes = [0_u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    let secret = format!(
        "awh_{}",
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
    );
    let stored = secrets::put(
        connection,
        keys.as_ref(),
        NewSecret {
            owner: SecretOwner::Project {
                organization_id: workspace,
                project_id: space,
            },
            name: &secret_name(id),
            description: Some("An incoming hook's signing secret."),
            value: &SecretValue::new(&secret),
            actor_id: Some(account),
            system_managed: true,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO incoming_hooks
            (id, workspace_id, space_id, name, secret_id, created_by_account_id, request_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(workspace)
    .bind(space)
    .bind(name)
    .bind(stored.id)
    .bind(account)
    .bind(request_id)
    .execute(&mut *connection)
    .await?;
    Ok((id, secret))
}

/// The hook `account` already made with `request_id` in `space`.
pub(crate) async fn made_by_request(
    connection: &mut PgConnection,
    space: Uuid,
    account: Uuid,
    request_id: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM incoming_hooks \
         WHERE created_by_account_id = $1 AND request_id = $2 AND space_id = $3",
    )
    .bind(account)
    .bind(request_id)
    .bind(space)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The hooks in `space` (one when `only` is given), newest first.
pub(crate) async fn list(
    connection: &mut PgConnection,
    state: &AppState,
    space: Uuid,
    only: Option<Uuid>,
) -> Result<Vec<HookView>, AppError> {
    let rows = sqlx::query_as::<_, HookRow>(
        r#"
        SELECT id, space_id, name, created_at, last_delivery_at
        FROM incoming_hooks
        WHERE space_id = $1 AND ($2::uuid IS NULL OR id = $2)
        ORDER BY created_at DESC, id DESC
        LIMIT 200
        "#,
    )
    .bind(space)
    .bind(only)
    .fetch_all(&mut *connection)
    .await?;
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let deliveries = sqlx::query_as::<_, HookDeliveryView>(
        r#"
        SELECT recent.hook_id, recent.delivery_key, recent.outcome, recent.reason,
               recent.received_at
        FROM unnest($1::uuid[]) AS hook(id)
        CROSS JOIN LATERAL (
            SELECT delivery.hook_id, delivery.delivery_key, delivery.outcome,
                   delivery.reason, delivery.received_at
            FROM incoming_hook_deliveries delivery
            WHERE delivery.hook_id = hook.id
            ORDER BY delivery.received_at DESC, delivery.id DESC
            LIMIT $2
        ) recent
        "#,
    )
    .bind(&ids)
    .bind(RECENT_DELIVERIES)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| HookView {
            url: url(state, row.id),
            recent: deliveries
                .iter()
                .filter(|delivery| delivery.hook_id == row.id)
                .cloned()
                .collect(),
            id: row.id,
            space_id: row.space_id,
            name: row.name,
            created_at: row.created_at,
            last_delivery_at: row.last_delivery_at,
        })
        .collect())
}

/// Removes a hook in `space`, its secret and its deliveries. False when
/// there is no such hook there.
pub(crate) async fn delete(
    connection: &mut PgConnection,
    space: Uuid,
    hook: Uuid,
) -> Result<bool, AppError> {
    let secret = sqlx::query_scalar::<_, Uuid>(
        "DELETE FROM incoming_hooks WHERE space_id = $1 AND id = $2 RETURNING secret_id",
    )
    .bind(space)
    .bind(hook)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(secret) = secret else {
        return Ok(false);
    };
    secrets::delete(&mut *connection, secret).await?;
    Ok(true)
}

/// Whether `hook` is in `space` (a rule naming it must be in its space).
pub(crate) async fn exists_in(pool: &PgPool, space: Uuid, hook: Uuid) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM incoming_hooks WHERE space_id = $1 AND id = $2)",
    )
    .bind(space)
    .bind(hook)
    .fetch_one(crate::db::scoped(pool))
    .await?)
}

/// What a rule in `space` may use of an accepted delivery to one of the
/// space's hooks.
pub(crate) struct RuleDelivery {
    pub hook_name: String,
    pub key: String,
    pub text: String,
}

pub(crate) async fn delivery_for_rule(
    pool: &PgPool,
    space: Uuid,
    hook: Uuid,
    delivery: Uuid,
) -> Result<Option<RuleDelivery>, AppError> {
    let row = sqlx::query_as::<_, (String, String, Option<Vec<u8>>)>(
        r#"
        SELECT hook.name, delivery.delivery_key, delivery.body
        FROM incoming_hook_deliveries delivery
        JOIN incoming_hooks hook ON hook.id = delivery.hook_id
        WHERE delivery.id = $3 AND delivery.hook_id = $2 AND delivery.space_id = $1
          AND hook.space_id = $1 AND delivery.outcome = 'accepted'
        "#,
    )
    .bind(space)
    .bind(hook)
    .bind(delivery)
    .fetch_optional(crate::db::scoped(pool))
    .await?;
    Ok(row.map(|(hook_name, key, body)| RuleDelivery {
        hook_name,
        key,
        text: String::from_utf8_lossy(body.as_deref().unwrap_or_default())
            .chars()
            .take(TEXT_CHARACTERS)
            .collect(),
    }))
}

#[derive(FromRow)]
struct Target {
    workspace_id: Uuid,
    space_id: Uuid,
    secret_id: Uuid,
}

/// Why a delivery is turned away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    NoDeliveryId,
    StaleTimestamp,
    BadSignature,
    TooLarge,
}

impl Refusal {
    fn code(self) -> &'static str {
        match self {
            Self::NoDeliveryId => "no_delivery_id",
            Self::StaleTimestamp => "stale_timestamp",
            Self::BadSignature => "bad_signature",
            Self::TooLarge => "too_large",
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Self::BadSignature => StatusCode::UNAUTHORIZED,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::NoDeliveryId | Self::StaleTimestamp => StatusCode::BAD_REQUEST,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::NoDeliveryId => "Send the delivery's own id in X-Hook-Delivery.",
            Self::StaleTimestamp => {
                "Send the time in X-Hook-Timestamp (Unix seconds, within five minutes)."
            }
            Self::BadSignature => "The signature doesn't match this hook's secret.",
            Self::TooLarge => "The body is larger than 64 KiB.",
        }
    }
}

/// The delivery's id, when it has a usable one.
fn delivery_key(headers: &HeaderMap) -> Option<String> {
    let key = headers.get(DELIVERY_HEADER)?.to_str().ok()?.trim();
    (!key.is_empty() && key.chars().count() <= 200 && !key.chars().any(char::is_control))
        .then(|| key.to_owned())
}

/// Checks the timestamp and the signature of `body` against `secret`.
pub(crate) fn check(
    secret: &[u8],
    headers: &HeaderMap,
    delivery: &str,
    body: &[u8],
    now: i64,
) -> Result<(), Refusal> {
    let timestamp = headers
        .get(TIMESTAMP_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|timestamp| (now - timestamp).abs() <= MAX_SKEW_SECONDS)
        .ok_or(Refusal::StaleTimestamp)?;
    let given = headers
        .get(SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().strip_prefix("sha256="))
        .and_then(|hex| hex::decode(hex).ok())
        .ok_or(Refusal::BadSignature)?;
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(secret).map_err(|_| Refusal::BadSignature)?;
    mac.update(signed(timestamp, delivery).as_bytes());
    mac.update(body);
    mac.verify_slice(&given).map_err(|_| Refusal::BadSignature)
}

/// What comes before the body in the signed text.
fn signed(timestamp: i64, delivery: &str) -> String {
    format!("{timestamp}.{delivery}.")
}

/// The signature a sender puts in `X-Hook-Signature` (for tests and docs).
pub fn sign(secret: &str, timestamp: i64, delivery: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key");
    mac.update(signed(timestamp, delivery).as_bytes());
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn answer(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

async fn receive(
    State(state): State<AppState>,
    ApiPath(hook_id): ApiPath<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    // A signed delivery: its hook, and so its workspace, is known only once it is read.
    crate::tenancy::system(
        "hooks.incoming",
        receive_here(State(state), ApiPath(hook_id), headers, body),
    )
    .await
}

async fn receive_here(
    State(state): State<AppState>,
    ApiPath(hook_id): ApiPath<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    // The hook is looked up first, by its key alone, so an address naming
    // no hook keeps no counter: per-hook counters exist only for real hooks.
    let target = sqlx::query_as::<_, Target>(
        "SELECT workspace_id, space_id, secret_id FROM incoming_hooks WHERE id = $1",
    )
    .bind(hook_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?;
    let Some(target) = target else {
        return Err(AppError::NotFound("incoming hook"));
    };
    state.rate_limits.check_incoming_hook(hook_id).await?;
    state
        .rate_limits
        .check_workspace_public_submission(target.workspace_id)
        .await?;
    let key = delivery_key(&headers);
    let checked = match (&key, body.len() > MAX_BODY_BYTES) {
        (None, _) => Err(Refusal::NoDeliveryId),
        (Some(_), true) => Err(Refusal::TooLarge),
        (Some(key), false) => {
            let keys = secrets::keyring(&state.config)?;
            let secret = secrets::reveal(
                crate::db::scoped(&state.pool),
                keys.as_ref(),
                target.secret_id,
                "incoming hook signature",
            )
            .await?;
            check(
                secret.expose().as_bytes(),
                &headers,
                key,
                &body,
                Utc::now().timestamp(),
            )
        }
    };
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    crate::workspace::lifecycle::ensure_open(
        &mut transaction,
        target.workspace_id,
        "incoming_hook_receive",
    )
    .await?;
    let delivery_id = Uuid::now_v7();
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(120).collect::<String>());
    if let Err(refusal) = checked {
        // Kept without its body, so the owner sees what was turned away.
        sqlx::query(
            r#"
            INSERT INTO incoming_hook_deliveries
                (id, workspace_id, space_id, hook_id, delivery_key, outcome, reason, content_type)
            VALUES ($1, $2, $3, $4, $5, 'refused', $6, $7)
            "#,
        )
        .bind(delivery_id)
        .bind(target.workspace_id)
        .bind(target.space_id)
        .bind(hook_id)
        .bind(key.as_deref().unwrap_or("(none)"))
        .bind(refusal.code())
        .bind(content_type)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        tracing::info!(%hook_id, reason = refusal.code(), "incoming hook delivery refused");
        return Ok(answer(
            refusal.status(),
            json!({ "status": "refused", "reason": refusal.code(), "message": refusal.message() }),
        ));
    }
    let key = key.unwrap_or_default();
    let accepted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO incoming_hook_deliveries
            (id, workspace_id, space_id, hook_id, delivery_key, outcome, content_type, body)
        VALUES ($1, $2, $3, $4, $5, 'accepted', $6, $7)
        ON CONFLICT (hook_id, delivery_key) WHERE outcome = 'accepted' DO NOTHING
        RETURNING id
        "#,
    )
    .bind(delivery_id)
    .bind(target.workspace_id)
    .bind(target.space_id)
    .bind(hook_id)
    .bind(&key)
    .bind(content_type)
    .bind(body.as_ref())
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(accepted) = accepted else {
        // A retry of a delivery already taken: nothing runs again.
        transaction.rollback().await?;
        return Ok(answer(
            StatusCode::OK,
            json!({ "status": "duplicate", "deliveryId": key }),
        ));
    };
    sqlx::query("UPDATE incoming_hooks SET last_delivery_at = now() WHERE id = $1")
        .bind(hook_id)
        .execute(&mut *transaction)
        .await?;
    crate::events::emit(
        &mut transaction,
        crate::events::NewEvent::thing(
            &crate::events::catalog::INCOMING_HOOK_RECEIVED,
            target.space_id,
            "incoming_hook",
            hook_id,
        )
        .workspace(target.workspace_id)
        .payload(json!({ "deliveryId": accepted })),
    )
    .await?;
    transaction.commit().await?;
    Ok(answer(
        StatusCode::ACCEPTED,
        json!({ "status": "accepted", "deliveryId": key }),
    ))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(timestamp: i64, signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(DELIVERY_HEADER, HeaderValue::from_static("d-1"));
        headers.insert(
            TIMESTAMP_HEADER,
            HeaderValue::from_str(&timestamp.to_string()).expect("header"),
        );
        headers.insert(
            SIGNATURE_HEADER,
            HeaderValue::from_str(signature).expect("header"),
        );
        headers
    }

    #[test]
    fn only_a_fresh_signature_over_the_body_checks_out() {
        let now = 1_800_000_000;
        let body = br#"{"ok":true}"#;
        let good = sign("awh_secret", now, "d-1", body);
        let ok = |secret: &[u8], signature: &str, delivery: &str, body: &[u8], at: i64| {
            check(secret, &headers(now, signature), delivery, body, at)
        };
        assert_eq!(ok(b"awh_secret", &good, "d-1", body, now), Ok(()));
        assert_eq!(
            ok(b"awh_other", &good, "d-1", body, now),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            ok(b"awh_secret", &good, "d-1", b"{}", now),
            Err(Refusal::BadSignature)
        );
        // The same body and signature under another delivery id.
        assert_eq!(
            ok(b"awh_secret", &good, "d-2", body, now),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            ok(b"awh_secret", &good, "d-1", body, now + 301),
            Err(Refusal::StaleTimestamp)
        );
        assert_eq!(
            ok(b"awh_secret", "deadbeef", "d-1", body, now),
            Err(Refusal::BadSignature)
        );
    }
}

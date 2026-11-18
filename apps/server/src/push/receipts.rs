//! C2.34: whether pushes arrive and get tapped. Each device's push carries a
//! random receipt (only its sha256 is kept); the service worker sends it
//! back when the notification shows and when it is tapped. A receipt names
//! one push to one device and says nothing else, so it needs no session:
//! knowing it is the permission, and all it can do is set those two times.
//! Each device's list in settings shows how many of its recent pushes showed
//! and were opened.

use axum::{Json, extract::State, http::HeaderMap, http::StatusCode};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::DeviceRow;
use crate::{
    AppState,
    auth::{random_token, require_allowed_origin},
    error::AppError,
};

fn receipt_hash(receipt: &str) -> Vec<u8> {
    Sha256::digest(receipt.as_bytes()).to_vec()
}

/// A receipt for one push to one device, kept before the push is sent.
pub(crate) async fn mint(
    pool: &PgPool,
    device: &DeviceRow,
    notification_id: Uuid,
) -> Result<String, AppError> {
    let receipt = random_token();
    sqlx::query(
        r#"
        INSERT INTO push_receipts (receipt_hash, workspace_id, subscription_id, notification_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(receipt_hash(&receipt))
    .bind(device.organization_id)
    .bind(device.id)
    .bind(notification_id)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(receipt)
}

/// Receipts are kept for 30 days: long enough to see how a device is doing.
pub async fn prune(pool: &PgPool) -> Result<u64, AppError> {
    Ok(crate::db::scoped_one(pool, async |connection| {
        sqlx::query("DELETE FROM push_receipts WHERE created_at < now() - interval '30 days'")
            .execute(connection)
            .await
    })
    .await?
    .rows_affected())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReceiptRequest {
    receipt: String,
    /// shown or opened.
    event: String,
}

/// What the service worker says about a push. Always 204: an unknown
/// receipt tells the sender nothing.
pub(crate) async fn receipt_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ReceiptRequest>,
) -> Result<StatusCode, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    let opened = match request.event.as_str() {
        "shown" => false,
        "opened" => true,
        _ => return Err(AppError::Validation("event")),
    };
    if request.receipt.is_empty()
        || request.receipt.len() > 128
        || URL_SAFE_NO_PAD.decode(&request.receipt).is_err()
    {
        return Ok(StatusCode::NO_CONTENT);
    }
    let hash = receipt_hash(&request.receipt);
    crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query(
            r#"
            UPDATE push_receipts
            SET shown_at = COALESCE(shown_at, now()),
                opened_at = CASE WHEN $2 THEN COALESCE(opened_at, now()) ELSE opened_at END
            WHERE receipt_hash = $1
            "#,
        )
        .bind(&hash)
        .bind(opened)
        .execute(connection)
        .await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

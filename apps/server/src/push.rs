//! N3: Web Push to phones and desktops, as a channel on top of W6's
//! notifications. Each person subscribes each device once; every
//! notification they get (and want by push, outside quiet hours) is
//! encrypted for each device and sent by the `push.deliver` job.
//!
//! Keys: the VAPID signing key is generated on first use and kept as an
//! encrypted workspace secret (S1); a device's endpoint and keys are a
//! capability, so they're an encrypted secret too, and never logged or
//! returned to the browser. Endpoints must be https on a known push
//! service, so a subscription can't make the server call anywhere else.
//!
//! The job recheck what the person can see when it sends (principle 9):
//! it reads the notification as them, so a lost membership sends nothing.

pub mod web_push;

use std::time::Duration as StdDuration;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    jobs::{self, JobError, JobRegistry, NewJob},
    notification_settings::{self, Group},
    notifications::{self, NotificationView},
    secrets::{self, NewSecret, SecretOwner, SecretValue},
};

pub const DELIVER_KIND: &str = "push.deliver";
const VAPID_SECRET: &str = "push.vapid_private_key";
const MAX_DEVICES_PER_PERSON: i64 = 20;
/// A device that fails this many sends in a row is dropped.
const MAX_CONSECUTIVE_FAILURES: i32 = 10;
/// How long a push service keeps an undelivered message.
const TTL_SECONDS: u32 = 24 * 60 * 60;
const BODY_CHARACTERS: usize = 240;

/// Push services browsers use. An endpoint on any other host is refused.
const PUSH_SERVICE_SUFFIXES: &[&str] = &[
    "fcm.googleapis.com",
    "android.googleapis.com",
    "push.services.mozilla.com",
    "push.apple.com",
    "notify.windows.com",
];

const PRUNE_KIND: &str = "push.prune_deliveries";

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(PRUNE_KIND, |context| async move {
        prune_deliveries(&context.state.pool).await?;
        Ok(())
    });
    registry.every(PRUNE_KIND, StdDuration::from_secs(6 * 60 * 60));
    registry.register_with_timeout(
        DELIVER_KIND,
        StdDuration::from_secs(120),
        |context| async move {
            let job: DeliverJob = context.job.payload_as()?;
            deliver_job(&context.state, &job).await
        },
    );
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliverJob {
    notification_id: Uuid,
    recipient: Uuid,
    /// One occurrence: the same on every attempt.
    delivery_key: String,
}

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// Queue a push for a notification occurrence just written, if the
/// recipient has a device and wants this kind by push. Runs inside the
/// transaction that wrote it.
pub(crate) async fn enqueue_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    notification_id: Uuid,
    recipient: Uuid,
    dedupe_key: &str,
) -> Result<(), AppError> {
    let has_device = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM push_subscriptions WHERE actor_id = $1)",
    )
    .bind(recipient)
    .fetch_one(&mut **transaction)
    .await?;
    if !has_device {
        return Ok(());
    }
    let delivery_key = hash(&format!("{recipient}:{dedupe_key}"));
    jobs::enqueue(
        &mut **transaction,
        NewJob::new(DELIVER_KIND)
            .workspace(organization_id)
            .payload(DeliverJob {
                notification_id,
                recipient,
                delivery_key: delivery_key.clone(),
            })
            .unique(format!("{DELIVER_KIND}:{delivery_key}"))
            .max_attempts(6)
            .priority(10),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Keys

fn keyring(state: &AppState) -> Result<std::sync::Arc<dyn secrets::KeyEncryption>, AppError> {
    secrets::keyring(&state.config).map_err(|_| AppError::SecretsNotConfigured)
}

/// The workspace's VAPID key (PKCS#8), made on first use.
async fn vapid_key(
    state: &AppState,
    organization_id: Uuid,
) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let keys = keyring(state)?;
    let owner = SecretOwner::Workspace { organization_id };
    if let Some(value) = secrets::reveal_named(
        &state.pool,
        keys.as_ref(),
        owner,
        VAPID_SECRET,
        "push.vapid",
    )
    .await?
    {
        return decode_vapid(&value);
    }
    let mut transaction = state.pool.begin().await?;
    // Two first uses at once must not make two keys: devices subscribe to
    // exactly one.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("push-vapid:{organization_id}"))
        .execute(&mut *transaction)
        .await?;
    if let Some(id) = secrets::find(&mut *transaction, owner, VAPID_SECRET).await? {
        let value = secrets::reveal(&mut *transaction, keys.as_ref(), id, "push.vapid").await?;
        transaction.commit().await?;
        return decode_vapid(&value);
    }
    let generated = web_push::generate_vapid_key().map_err(|_| AppError::Internal("vapid key"))?;
    let value = SecretValue::new(STANDARD.encode(generated.as_slice()));
    secrets::put(
        &mut transaction,
        keys.as_ref(),
        NewSecret {
            owner,
            name: VAPID_SECRET,
            description: Some("Signs this workspace's Web Push notifications."),
            value: &value,
            actor_id: None,
            system_managed: true,
        },
    )
    .await?;
    transaction.commit().await?;
    Ok(generated)
}

fn decode_vapid(value: &SecretValue) -> Result<Zeroizing<Vec<u8>>, AppError> {
    STANDARD
        .decode(value.expose())
        .map(Zeroizing::new)
        .map_err(|_| AppError::Internal("vapid key"))
}

fn contact(state: &AppState) -> String {
    if let Some(contact) = &state.config.push_contact {
        return contact.clone();
    }
    match state.config.public_origin.as_deref() {
        Some(origin) if origin.starts_with("https://") => origin.to_owned(),
        Some(origin) => {
            let host = url::Url::parse(origin)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_else(|| "localhost".to_owned());
            format!("mailto:push@{host}")
        }
        None => "mailto:push@localhost".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Subscriptions

fn push_service_host(endpoint: &str) -> Option<url::Url> {
    let url = url::Url::parse(endpoint).ok()?;
    if url.scheme() != "https"
        || url.port().is_some_and(|port| port != 443)
        || !url.username().is_empty()
        || url.password().is_some()
        || endpoint.len() > 2048
    {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    PUSH_SERVICE_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        .then_some(url)
}

#[derive(Deserialize, Serialize)]
struct StoredSubscription {
    endpoint: String,
    p256dh: String,
    auth: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: Uuid,
    pub label: String,
    /// The push service's host, such as `fcm.googleapis.com`.
    pub service_host: String,
    /// sha256 of the endpoint, so a browser can tell which device it is.
    pub endpoint_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_sent_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub failure_count: i32,
}

pub async fn devices(pool: &PgPool, actor_id: Uuid) -> Result<Vec<DeviceView>, AppError> {
    Ok(sqlx::query_as::<_, DeviceView>(
        r#"
        SELECT id, label, service_host, endpoint_hash, created_at, last_sent_at,
               last_failure_at, failure_count
        FROM push_subscriptions WHERE actor_id = $1
        ORDER BY created_at, id
        "#,
    )
    .bind(actor_id)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceKeys {
    p256dh: String,
    auth: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubscribeRequest {
    endpoint: String,
    keys: DeviceKeys,
    label: Option<String>,
    /// The workspace whose key the browser subscribed with.
    organization_id: Uuid,
    #[serde(default)]
    expiration_time: Option<Value>,
}

async fn subscribe(
    state: &AppState,
    actor_id: Uuid,
    request: SubscribeRequest,
) -> Result<DeviceView, AppError> {
    let organization_id =
        notification_settings::home_workspace(&state.pool, actor_id, Some(request.organization_id))
            .await?;
    let endpoint = push_service_host(&request.endpoint).ok_or(AppError::Validation("endpoint"))?;
    let p256dh = web_push::decode_key(&request.keys.p256dh).ok_or(AppError::Validation("keys"))?;
    let auth = web_push::decode_key(&request.keys.auth).ok_or(AppError::Validation("keys"))?;
    if !web_push::valid_device_keys(&p256dh, &auth) {
        return Err(AppError::Validation("keys"));
    }
    let label = request
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty() && !label.chars().any(char::is_control))
        .map(|label| label.chars().take(80).collect::<String>())
        .unwrap_or_else(|| "This device".to_owned());
    let endpoint_hash = hash(&request.endpoint);
    let keys = keyring(state)?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("push-devices:{actor_id}"))
        .execute(&mut *transaction)
        .await?;
    let existing = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT id, secret_id FROM push_subscriptions
        WHERE organization_id = $1 AND actor_id = $2 AND endpoint_hash = $3
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(&endpoint_hash)
    .fetch_optional(&mut *transaction)
    .await?;
    if existing.is_none() {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM push_subscriptions WHERE actor_id = $1",
        )
        .bind(actor_id)
        .fetch_one(&mut *transaction)
        .await?;
        if count >= MAX_DEVICES_PER_PERSON {
            return Err(AppError::Conflict("push_subscription", "too many devices"));
        }
    }
    let id = existing.map_or_else(Uuid::now_v7, |(id, _)| id);
    let stored = Zeroizing::new(
        serde_json::to_string(&StoredSubscription {
            endpoint: request.endpoint.clone(),
            p256dh: request.keys.p256dh.clone(),
            auth: request.keys.auth.clone(),
        })
        .map_err(|_| AppError::Internal("push subscription"))?,
    );
    let secret = secrets::put(
        &mut transaction,
        keys.as_ref(),
        NewSecret {
            owner: SecretOwner::Actor {
                organization_id,
                actor_id,
            },
            name: &format!("push:{}", id.simple()),
            description: None,
            value: &SecretValue::new(stored.as_str()),
            actor_id: Some(actor_id),
            system_managed: true,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO push_subscriptions (
            id, organization_id, actor_id, secret_id, endpoint_hash, service_host, label
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (organization_id, actor_id, endpoint_hash) DO UPDATE SET
            secret_id = EXCLUDED.secret_id, label = EXCLUDED.label,
            failure_count = 0, last_failure_at = NULL
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(actor_id)
    .bind(secret.id)
    .bind(&endpoint_hash)
    .bind(endpoint.host_str().unwrap_or_default())
    .bind(&label)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    devices(&state.pool, actor_id)
        .await?
        .into_iter()
        .find(|device| device.id == id)
        .ok_or(AppError::Internal("push subscription"))
}

/// Remove a device and its stored endpoint.
async fn remove_device(pool: &PgPool, actor_id: Uuid, id: Uuid) -> Result<bool, AppError> {
    let mut transaction = pool.begin().await?;
    let Some(secret_id) = sqlx::query_scalar::<_, Uuid>(
        "DELETE FROM push_subscriptions WHERE id = $1 AND actor_id = $2 RETURNING secret_id",
    )
    .bind(id)
    .bind(actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    else {
        return Ok(false);
    };
    secrets::delete(&mut *transaction, secret_id).await?;
    transaction.commit().await?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Sending

#[derive(FromRow)]
struct DeviceRow {
    id: Uuid,
    organization_id: Uuid,
    secret_id: Uuid,
}

/// What one send did.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SendResult {
    Sent,
    /// The push service says the device is gone; it was removed.
    Gone,
    /// Worth trying again later (rate limited, service down).
    Retry,
    Failed,
}

/// The JSON the service worker reads (`sw.js`).
pub fn payload_for(notification: &NotificationView) -> Value {
    let body = notification
        .preview
        .as_deref()
        .map(|text| text.chars().take(BODY_CHARACTERS).collect::<String>());
    let mut actions = vec![json!({"action": "open", "title": "Open"})];
    let launch = notification
        .launch_href
        .as_deref()
        .filter(|href| href.starts_with('/'));
    if let Some(href) = launch {
        actions.push(json!({"action": "launch", "title": "Launch", "url": href}));
    }
    json!({
        "id": notification.id,
        "kind": notification.kind,
        "title": notification.title.chars().take(160).collect::<String>(),
        "body": body,
        "place": notification.project_name,
        "url": notification.href,
        "tag": notification.subject.id,
        "actions": actions,
    })
}

async fn send_one(
    state: &AppState,
    client: &reqwest::Client,
    device: &DeviceRow,
    payload: &[u8],
    topic: &str,
    urgency: &str,
) -> Result<SendResult, AppError> {
    let keys = keyring(state)?;
    let stored = secrets::reveal(&state.pool, keys.as_ref(), device.secret_id, "push.send").await?;
    let stored: StoredSubscription = serde_json::from_str(stored.expose())
        .map_err(|_| AppError::Internal("push subscription"))?;
    let Some(endpoint) = push_service_host(&stored.endpoint) else {
        return Ok(SendResult::Failed);
    };
    let (Some(p256dh), Some(auth)) = (
        web_push::decode_key(&stored.p256dh),
        web_push::decode_key(&stored.auth),
    ) else {
        return Ok(SendResult::Failed);
    };
    let body = web_push::encrypt(
        &web_push::Subscription {
            endpoint: &stored.endpoint,
            p256dh: &p256dh,
            auth: &auth,
        },
        payload,
    )
    .map_err(|_| AppError::Validation("push payload"))?;
    let vapid = vapid_key(state, device.organization_id).await?;
    let authorization =
        web_push::vapid_authorization(&vapid, &endpoint, &contact(state), Utc::now().timestamp())
            .map_err(|_| AppError::Internal("vapid key"))?;
    let response = client
        .post(endpoint)
        .header("Authorization", authorization)
        .header("TTL", TTL_SECONDS.to_string())
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("Urgency", urgency)
        .header("Topic", topic)
        .body(body)
        .send()
        .await;
    let status = match response {
        Ok(response) => response.status(),
        Err(_) => return Ok(SendResult::Retry),
    };
    Ok(match status.as_u16() {
        200..=299 => SendResult::Sent,
        404 | 410 => SendResult::Gone,
        429 | 500..=599 => SendResult::Retry,
        _ => SendResult::Failed,
    })
}

async fn record(pool: &PgPool, device: &DeviceRow, result: SendResult) -> Result<(), AppError> {
    match result {
        SendResult::Sent => {
            sqlx::query(
                "UPDATE push_subscriptions SET last_sent_at = now(), failure_count = 0 WHERE id = $1",
            )
            .bind(device.id)
            .execute(pool)
            .await?;
        }
        SendResult::Gone => {
            let mut transaction = pool.begin().await?;
            sqlx::query("DELETE FROM push_subscriptions WHERE id = $1")
                .bind(device.id)
                .execute(&mut *transaction)
                .await?;
            secrets::delete(&mut *transaction, device.secret_id).await?;
            transaction.commit().await?;
        }
        SendResult::Retry | SendResult::Failed => {
            let failures = sqlx::query_scalar::<_, i32>(
                r#"
                UPDATE push_subscriptions
                SET failure_count = failure_count + 1, last_failure_at = now()
                WHERE id = $1 RETURNING failure_count
                "#,
            )
            .bind(device.id)
            .fetch_optional(pool)
            .await?;
            if failures.is_some_and(|count| count >= MAX_CONSECUTIVE_FAILURES) {
                Box::pin(record(pool, device, SendResult::Gone)).await?;
            }
        }
    }
    Ok(())
}

fn http_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(StdDuration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AppError::Internal("push client"))
}

async fn recipient_devices(pool: &PgPool, actor_id: Uuid) -> Result<Vec<DeviceRow>, AppError> {
    Ok(sqlx::query_as::<_, DeviceRow>(
        "SELECT id, organization_id, secret_id FROM push_subscriptions WHERE actor_id = $1 ORDER BY id",
    )
    .bind(actor_id)
    .fetch_all(pool)
    .await?)
}

async fn deliver_job(state: &AppState, job: &DeliverJob) -> Result<(), JobError> {
    // Read as the recipient: anything they can no longer see sends nothing.
    let Some(notification) =
        notifications::get_many(&state.pool, job.recipient, &[job.notification_id])
            .await?
            .into_iter()
            .next()
    else {
        return Ok(());
    };
    if notification.read {
        return Ok(());
    }
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(notification.project_id)
            .fetch_one(&state.pool)
            .await?;
    {
        let mut connection = state.pool.acquire().await.map_err(AppError::from)?;
        let decision = notification_settings::decide(
            &mut connection,
            job.recipient,
            notification.project_id,
            &notification.kind,
        )
        .await?;
        if !decision.push {
            return Ok(());
        }
        let (quiet, _) =
            notification_settings::quiet_hours(&mut connection, organization_id, job.recipient)
                .await?;
        if let Some(ends) = quiet.ends_after(Utc::now()) {
            jobs::enqueue(
                &state.pool,
                NewJob::new(DELIVER_KIND)
                    .workspace(organization_id)
                    .payload(DeliverJob {
                        notification_id: job.notification_id,
                        recipient: job.recipient,
                        delivery_key: job.delivery_key.clone(),
                    })
                    .unique(format!("{DELIVER_KIND}:{}:after-quiet", job.delivery_key))
                    .run_at(ends)
                    .max_attempts(6),
            )
            .await?;
            return Ok(());
        }
    }
    let payload = serde_json::to_vec(&payload_for(&notification))
        .map_err(|_| JobError::permanent("push payload"))?;
    let topic = notification.id.simple().to_string();
    let urgency = if Group::of(&notification.kind) == Group::Usage
        || notification.kind == notifications::kind::APPROVAL_REQUESTED
    {
        "high"
    } else {
        "normal"
    };
    let client = http_client()?;
    let mut retry = false;
    for device in recipient_devices(&state.pool, job.recipient).await? {
        let sent = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM push_deliveries WHERE subscription_id = $1 AND delivery_key = $2)",
        )
        .bind(device.id)
        .bind(&job.delivery_key)
        .fetch_one(&state.pool)
        .await?;
        if sent {
            continue;
        }
        let result = send_one(state, &client, &device, &payload, &topic, urgency).await?;
        if result == SendResult::Sent {
            sqlx::query(
                "INSERT INTO push_deliveries (subscription_id, delivery_key) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(device.id)
            .bind(&job.delivery_key)
            .execute(&state.pool)
            .await?;
        }
        retry |= result == SendResult::Retry;
        record(&state.pool, &device, result).await?;
    }
    if retry {
        return Err(JobError::retry("push service asked to retry"));
    }
    Ok(())
}

/// Delivery records older than a week aren't needed to dedupe retries.
pub async fn prune_deliveries(pool: &PgPool) -> Result<u64, AppError> {
    Ok(
        sqlx::query("DELETE FROM push_deliveries WHERE sent_at < now() - interval '7 days'")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/push/config", get(config_http))
        .route(
            "/api/v1/push/subscriptions",
            get(list_http).post(subscribe_http),
        )
        .route("/api/v1/push/subscriptions/{id}", delete(remove_http))
        .route("/api/v1/push/subscriptions/{id}/test", post(test_http))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceQuery {
    organization_id: Option<Uuid>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigResponse {
    organization_id: Uuid,
    /// The application server key to subscribe with (base64url).
    public_key: String,
}

async fn config_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<WorkspaceQuery>,
) -> Result<Json<ConfigResponse>, AppError> {
    let organization_id = notification_settings::home_workspace(
        &state.pool,
        principal.actor_id,
        query.organization_id,
    )
    .await?;
    let key = vapid_key(&state, organization_id).await?;
    Ok(Json(ConfigResponse {
        organization_id,
        public_key: web_push::vapid_public_key(&key)
            .map_err(|_| AppError::Internal("vapid key"))?,
    }))
}

#[derive(Serialize)]
struct DevicesResponse {
    devices: Vec<DeviceView>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<DevicesResponse>, AppError> {
    Ok(Json(DevicesResponse {
        devices: devices(&state.pool, principal.actor_id).await?,
    }))
}

async fn subscribe_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<SubscribeRequest>,
) -> Result<(StatusCode, Json<DeviceView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let _ = &request.expiration_time;
    Ok((
        StatusCode::CREATED,
        Json(subscribe(&state, principal.actor_id, request).await?),
    ))
}

async fn remove_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if remove_device(&state.pool, principal.actor_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound("push_subscription"))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Serialize)]
struct TestResponse {
    result: SendResult,
}

/// Send a test notification to one device now.
async fn test_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<TestResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let device = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, organization_id, secret_id FROM push_subscriptions WHERE id = $1 AND actor_id = $2",
    )
    .bind(id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("push_subscription"))?;
    let payload = serde_json::to_vec(&json!({
        "id": Uuid::now_v7(),
        "kind": "test",
        "title": "Notifications work on this device",
        "body": "You'll get mentions, approvals, runs and capacity alerts here.",
        "url": "/settings/account/notifications",
        "tag": "test",
        "actions": [{"action": "open", "title": "Open"}],
    }))
    .map_err(|_| AppError::Internal("push payload"))?;
    let client = http_client()?;
    let result = send_one(&state, &client, &device, &payload, "test", "normal").await?;
    record(&state.pool, &device, result).await?;
    Ok(Json(TestResponse { result }))
}

#[cfg(test)]
mod tests {
    use super::push_service_host;

    #[test]
    fn only_https_push_services_are_accepted() {
        for good in [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://web.push.apple.com/QGx",
            "https://wns2-par02p.notify.windows.com/w/?token=x",
        ] {
            assert!(push_service_host(good).is_some(), "{good}");
        }
        for bad in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://fcm.googleapis.com:8443/x",
            "https://evil.example.com/push",
            "https://fcm.googleapis.com.evil.example/x",
            "https://127.0.0.1/x",
            "https://user:pw@fcm.googleapis.com/x",
            "not a url",
        ] {
            assert!(push_service_host(bad).is_none(), "{bad}");
        }
    }
}

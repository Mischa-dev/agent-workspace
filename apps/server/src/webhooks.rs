//! X4: outgoing webhooks.
//!
//! A subscription belongs to the person (or the connected app acting as
//! them) that made it, and hears one workspace's events: every project in
//! it they can see, or one project. Events come from [`crate::events`]; each
//! one a subscription wants becomes a delivery row and a `webhooks.deliver`
//! job. A delivery is checked again against its owner's access when it is
//! sent (principle 9): a message only goes out if the owner can see its
//! conversation now (`conversation_visible_to`), a pull request only if
//! `pull_request_visible_to`, anything only if they're still in the
//! project. A subscription whose owner left its workspace or project, or
//! whose app was disconnected, turns itself off with the reason.
//!
//! Each POST is signed like agent wakeups: `X-Workspace-Signature:
//! sha256=<HMAC-SHA256 of "{timestamp}.{body}">` with the subscription's
//! secret, which is stored encrypted (S1) and shown once. Every attempt of
//! one delivery carries the same `X-Workspace-Delivery` id. Targets pass
//! PR #128's address checks (`replies::validate_webhook_url`), and the
//! client pins the checked address. Failed deliveries retry with the job
//! queue's backoff; twenty failed deliveries in a row turn it off.

use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgExecutor, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    events::{self, Caller, Event, Subject},
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    policy::ActorKind,
    secrets::{self, NewSecret, SecretOwner, SecretValue},
};

pub const DELIVER_KIND: &str = "webhooks.deliver";
const PRUNE_KIND: &str = "webhooks.prune";
/// A person's subscriptions per workspace, at most.
const MAX_PER_OWNER: i64 = 25;
/// Failed deliveries in a row that turn a subscription off.
const MAX_CONSECUTIVE_FAILURES: i32 = 20;
const MAX_LOG_PAGE: i64 = 100;
const MESSAGE_TEXT_CHARACTERS: usize = 8_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/webhooks", get(list_route).post(create_route))
        .route(
            "/api/v1/webhooks/{webhook_id}",
            axum::routing::patch(update_route).delete(delete_route),
        )
        .route(
            "/api/v1/webhooks/{webhook_id}/rotate-secret",
            post(rotate_route),
        )
        .route("/api/v1/webhooks/{webhook_id}/test", post(test_route))
        .route(
            "/api/v1/webhooks/{webhook_id}/deliveries",
            get(deliveries_route),
        )
        .route(
            "/api/v1/webhooks/{webhook_id}/deliveries/{delivery_id}/redeliver",
            post(redeliver_route),
        )
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(DELIVER_KIND, |context: JobContext| async move {
        deliver(&context).await
    });
    registry.register(PRUNE_KIND, |context: JobContext| async move {
        sqlx::query(
            "DELETE FROM webhook_deliveries WHERE state <> 'pending' AND created_at < now() - interval '30 days'",
        )
        .execute(&context.state.pool)
        .await?;
        Ok(())
    });
    registry.every(PRUNE_KIND, Duration::from_secs(6 * 60 * 60));
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, FromRow)]
struct SubscriptionRow {
    id: Uuid,
    organization_id: Uuid,
    project_id: Option<Uuid>,
    project_name: Option<String>,
    owner_actor_id: Uuid,
    via_oauth_grant_id: Option<Uuid>,
    made_through_app: bool,
    name: String,
    url: String,
    event_types: Vec<String>,
    secret_id: Option<Uuid>,
    state: String,
    disabled_reason: Option<String>,
    consecutive_failures: i32,
    last_delivery_at: Option<DateTime<Utc>>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const SUBSCRIPTION_SELECT: &str = r#"
    SELECT subscription.id, subscription.organization_id, subscription.project_id,
           project.name AS project_name, subscription.owner_actor_id,
           subscription.via_oauth_grant_id, subscription.made_through_app,
           subscription.name, subscription.url,
           subscription.event_types, subscription.secret_id, subscription.state,
           subscription.disabled_reason, subscription.consecutive_failures,
           subscription.last_delivery_at, subscription.version,
           subscription.created_at, subscription.updated_at
    FROM webhook_subscriptions subscription
    LEFT JOIN projects project ON project.id = subscription.project_id
"#;

/// What anyone may see about a subscription: never its secret.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookView {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub project_id: Option<Uuid>,
    /// The project it's limited to, or null for the whole workspace.
    pub project_name: Option<String>,
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    /// active, paused or disabled (turned off by the server; see reason).
    pub state: String,
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub last_delivery_at: Option<DateTime<Utc>>,
    pub has_secret: bool,
    pub via_app: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<SubscriptionRow> for WebhookView {
    fn from(row: SubscriptionRow) -> Self {
        Self {
            id: row.id,
            organization_id: row.organization_id,
            project_id: row.project_id,
            project_name: row.project_name,
            name: row.name,
            url: row.url,
            event_types: row.event_types,
            state: row.state,
            disabled_reason: row.disabled_reason,
            consecutive_failures: row.consecutive_failures,
            last_delivery_at: row.last_delivery_at,
            has_secret: row.secret_id.is_some(),
            via_app: row.via_oauth_grant_id.is_some(),
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryView {
    /// The id the receiver sees in X-Workspace-Delivery, the same on every
    /// retry.
    pub id: Uuid,
    pub event_type: String,
    pub project_id: Option<Uuid>,
    pub subject: Value,
    pub state: String,
    pub attempts: i32,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    pub duration_ms: Option<i32>,
    pub occurred_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTypeView {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookProject {
    pub id: Uuid,
    pub name: String,
    pub organization_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookList {
    pub webhooks: Vec<WebhookView>,
    pub event_types: Vec<EventTypeView>,
    /// Projects a subscription can be limited to.
    pub projects: Vec<WebhookProject>,
    /// How receivers check a delivery.
    pub signing: &'static str,
}

pub const SIGNING_HELP: &str = "Each delivery is a POST with JSON. Check it with your secret: the X-Workspace-Signature header is sha256= followed by the hex HMAC-SHA256 of the X-Workspace-Timestamp header, a dot, and the raw body. Drop a delivery whose X-Workspace-Delivery id you've already handled; retries carry the same id.";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedWebhook {
    pub webhook: WebhookView,
    /// The signing secret. Shown once; store it now.
    pub secret: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryPage {
    pub deliveries: Vec<DeliveryView>,
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Core, shared by the browser and MCP

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateWebhook {
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    /// Limit it to one project; left out, the whole workspace.
    #[serde(default)]
    pub project_id: Option<Uuid>,
    /// The workspace, when no project is given; left out, the caller's
    /// default.
    #[serde(default)]
    pub organization_id: Option<Uuid>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWebhook {
    pub expected_version: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub event_types: Option<Vec<String>>,
    /// active or paused. Turning a disabled one back on is "active".
    #[serde(default)]
    pub state: Option<String>,
}

fn normalize_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("name"));
    }
    Ok(name.to_owned())
}

fn normalize_events(values: &[String]) -> Result<Vec<String>, AppError> {
    let mut events = values
        .iter()
        .map(|value| value.trim().to_owned())
        .collect::<Vec<_>>();
    events.sort();
    events.dedup();
    if events.is_empty()
        || events.len() > 20
        || !events.iter().all(|event| events::is_event_type(event))
    {
        return Err(AppError::Validation("eventTypes"));
    }
    Ok(events)
}

fn check_url(state: &AppState, value: &str) -> Result<String, AppError> {
    let value = value.trim();
    crate::replies::validate_webhook_url(value, &state.config.webhook_trusted_origins)?;
    Ok(value.to_owned())
}

fn require_person(caller: &Caller) -> Result<(), AppError> {
    // Webhooks deliver what a person can see; agents hear events through
    // their own connection instead.
    if caller.actor_kind != ActorKind::Human {
        return Err(AppError::Forbidden);
    }
    if !caller.can_write {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn new_secret() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("whsec_{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn secret_name(webhook_id: Uuid) -> String {
    format!("outgoing_webhook:{webhook_id}")
}

async fn store_secret(
    state: &AppState,
    connection: &mut sqlx::PgConnection,
    organization_id: Uuid,
    owner: Uuid,
    webhook_id: Uuid,
) -> Result<(Uuid, String), AppError> {
    let keys = secrets::keyring(&state.config)?;
    let value = new_secret();
    let secret = SecretValue::new(value.clone());
    let stored = secrets::put(
        connection,
        keys.as_ref(),
        NewSecret {
            owner: SecretOwner::Actor {
                organization_id,
                actor_id: owner,
            },
            name: &secret_name(webhook_id),
            description: Some("Signs this webhook's deliveries."),
            value: &secret,
            actor_id: Some(owner),
            system_managed: true,
        },
    )
    .await?;
    Ok((stored.id, value))
}

async fn fetch(
    executor: impl PgExecutor<'_>,
    webhook_id: Uuid,
) -> Result<SubscriptionRow, AppError> {
    sqlx::query_as::<_, SubscriptionRow>(&format!(
        "{SUBSCRIPTION_SELECT} WHERE subscription.id = $1"
    ))
    .bind(webhook_id)
    .fetch_optional(executor)
    .await?
    .ok_or(AppError::NotFound("webhook"))
}

/// The caller's own subscription, or not found.
async fn owned(
    executor: impl PgExecutor<'_>,
    caller: &Caller,
    webhook_id: Uuid,
) -> Result<SubscriptionRow, AppError> {
    let row = fetch(executor, webhook_id).await?;
    if row.owner_actor_id != caller.actor_id {
        return Err(AppError::NotFound("webhook"));
    }
    if let Some(project_id) = row.project_id
        && !caller.reaches(project_id)
    {
        return Err(AppError::NotFound("webhook"));
    }
    if row.project_id.is_none() && caller.token_project_id.is_some() {
        return Err(AppError::NotFound("webhook"));
    }
    Ok(row)
}

pub(crate) async fn list(state: &AppState, caller: &Caller) -> Result<WebhookList, AppError> {
    let rows = sqlx::query_as::<_, SubscriptionRow>(&format!(
        r#"{SUBSCRIPTION_SELECT}
        WHERE subscription.owner_actor_id = $1
          AND ($2::uuid IS NULL OR subscription.project_id = $2)
          AND EXISTS (
              SELECT 1 FROM organization_memberships membership
              WHERE membership.organization_id = subscription.organization_id
                AND membership.actor_id = $1 AND membership.enabled
          )
        ORDER BY subscription.created_at DESC, subscription.id DESC
        LIMIT 200"#
    ))
    .bind(caller.actor_id)
    .bind(caller.token_project_id)
    .fetch_all(&state.pool)
    .await?;
    let projects = sqlx::query_as::<_, WebhookProject>(
        r#"
        SELECT project.id, project.name, project.organization_id
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id AND project.archived_at IS NULL
        WHERE membership.actor_id = $1 AND membership.enabled
          AND ($2::uuid IS NULL OR project.id = $2)
          AND actor_can_access_project($1, NULL, project.id)
        ORDER BY (project.kind = 'direct'), lower(project.name), project.id
        LIMIT 300
        "#,
    )
    .bind(caller.actor_id)
    .bind(caller.token_project_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(WebhookList {
        webhooks: rows.into_iter().map(WebhookView::from).collect(),
        event_types: events::EVENT_TYPES
            .iter()
            .map(|(name, description)| EventTypeView { name, description })
            .collect(),
        projects,
        signing: SIGNING_HELP,
    })
}

pub(crate) async fn create(
    state: &AppState,
    caller: &Caller,
    request: CreateWebhook,
) -> Result<CreatedWebhook, AppError> {
    require_person(caller)?;
    let name = normalize_name(&request.name)?;
    let url = check_url(state, &request.url)?;
    let event_types = normalize_events(&request.event_types)?;
    let organization_id = match request.project_id {
        Some(project_id) => {
            if !caller.reaches(project_id)
                || !sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
                    .bind(caller.actor_id)
                    .bind(project_id)
                    .fetch_one(&state.pool)
                    .await?
            {
                return Err(AppError::NotFound("project"));
            }
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_one(&state.pool)
                .await?
        }
        None => {
            if caller.token_project_id.is_some() {
                // A token for one project can't hear the whole workspace.
                return Err(AppError::Validation("projectId"));
            }
            let chosen = sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT membership.organization_id
                FROM organization_memberships membership
                JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
                WHERE membership.actor_id = $1 AND membership.enabled
                  AND ($2::uuid IS NULL OR membership.organization_id = $2)
                ORDER BY membership.created_at, membership.organization_id
                LIMIT 1
                "#,
            )
            .bind(caller.actor_id)
            .bind(request.organization_id)
            .fetch_optional(&state.pool)
            .await?;
            chosen.ok_or(AppError::NotFound("workspace"))?
        }
    };
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("webhooks:{organization_id}:{}", caller.actor_id))
        .execute(&mut *transaction)
        .await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM webhook_subscriptions WHERE organization_id = $1 AND owner_actor_id = $2",
    )
    .bind(organization_id)
    .bind(caller.actor_id)
    .fetch_one(&mut *transaction)
    .await?;
    if count >= MAX_PER_OWNER {
        return Err(AppError::Conflict(
            "webhook",
            "you have 25 webhooks in this workspace; delete one first",
        ));
    }
    let id = Uuid::now_v7();
    let (secret_id, secret) = store_secret(
        state,
        &mut transaction,
        organization_id,
        caller.actor_id,
        id,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO webhook_subscriptions (
            id, organization_id, project_id, owner_actor_id, via_oauth_grant_id,
            name, url, event_types, secret_id, made_through_app
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(request.project_id)
    .bind(caller.actor_id)
    .bind(caller.via_grant_id)
    .bind(&name)
    .bind(&url)
    .bind(&event_types)
    .bind(secret_id)
    .bind(through_app(caller))
    .execute(&mut *transaction)
    .await?;
    events::audit(
        &mut *transaction,
        caller,
        organization_id,
        request.project_id,
        "webhook.created",
        "webhook",
        id,
        json!({ "eventTypes": event_types }),
    )
    .await?;
    transaction.commit().await?;
    Ok(CreatedWebhook {
        webhook: fetch(&state.pool, id).await?.into(),
        secret,
    })
}

pub(crate) async fn update(
    state: &AppState,
    caller: &Caller,
    webhook_id: Uuid,
    request: UpdateWebhook,
) -> Result<WebhookView, AppError> {
    require_person(caller)?;
    let name = request.name.as_deref().map(normalize_name).transpose()?;
    let url = request
        .url
        .as_deref()
        .map(|url| check_url(state, url))
        .transpose()?;
    let event_types = request
        .event_types
        .as_deref()
        .map(normalize_events)
        .transpose()?;
    let new_state = match request.state.as_deref() {
        None => None,
        Some(value @ ("active" | "paused")) => Some(value.to_owned()),
        Some(_) => return Err(AppError::Validation("state")),
    };
    let mut transaction = state.pool.begin().await?;
    let current = owned(&mut *transaction, caller, webhook_id).await?;
    if current.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "webhook",
            current_version: current.version,
        });
    }
    // Turning it back on checks the owner may still use what it covers.
    if new_state.as_deref() == Some("active")
        && let Some(project_id) = current.project_id
        && !sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
            .bind(caller.actor_id)
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?
    {
        return Err(AppError::Forbidden);
    }
    sqlx::query(
        r#"
        UPDATE webhook_subscriptions
        SET name = COALESCE($2, name),
            url = COALESCE($3, url),
            event_types = COALESCE($4, event_types),
            state = COALESCE($5, state),
            disabled_reason = CASE WHEN $5 IS NULL THEN disabled_reason ELSE NULL END,
            consecutive_failures = CASE WHEN $5 = 'active' THEN 0 ELSE consecutive_failures END,
            made_through_app = made_through_app OR $6,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(webhook_id)
    .bind(&name)
    .bind(&url)
    .bind(&event_types)
    .bind(&new_state)
    .bind(through_app(caller))
    .execute(&mut *transaction)
    .await?;
    events::audit(
        &mut *transaction,
        caller,
        current.organization_id,
        current.project_id,
        "webhook.updated",
        "webhook",
        webhook_id,
        json!({ "state": new_state, "urlChanged": url.is_some() }),
    )
    .await?;
    transaction.commit().await?;
    Ok(fetch(&state.pool, webhook_id).await?.into())
}

pub(crate) async fn delete(
    state: &AppState,
    caller: &Caller,
    webhook_id: Uuid,
) -> Result<(), AppError> {
    require_person(caller)?;
    let mut transaction = state.pool.begin().await?;
    let current = owned(&mut *transaction, caller, webhook_id).await?;
    sqlx::query("DELETE FROM webhook_subscriptions WHERE id = $1")
        .bind(webhook_id)
        .execute(&mut *transaction)
        .await?;
    if let Some(secret_id) = current.secret_id {
        secrets::delete(&mut *transaction, secret_id).await?;
    }
    events::audit(
        &mut *transaction,
        caller,
        current.organization_id,
        current.project_id,
        "webhook.deleted",
        "webhook",
        webhook_id,
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn rotate_secret(
    state: &AppState,
    caller: &Caller,
    webhook_id: Uuid,
) -> Result<CreatedWebhook, AppError> {
    require_person(caller)?;
    let mut transaction = state.pool.begin().await?;
    let current = owned(&mut *transaction, caller, webhook_id).await?;
    let (secret_id, secret) = store_secret(
        state,
        &mut transaction,
        current.organization_id,
        current.owner_actor_id,
        webhook_id,
    )
    .await?;
    sqlx::query(
        "UPDATE webhook_subscriptions SET secret_id = $2, version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(webhook_id)
    .bind(secret_id)
    .execute(&mut *transaction)
    .await?;
    events::audit(
        &mut *transaction,
        caller,
        current.organization_id,
        current.project_id,
        "webhook.secret_rotated",
        "webhook",
        webhook_id,
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(CreatedWebhook {
        webhook: fetch(&state.pool, webhook_id).await?.into(),
        secret,
    })
}

/// Sends a `ping` delivery now, through the queue like any other.
pub(crate) async fn send_test(
    state: &AppState,
    caller: &Caller,
    webhook_id: Uuid,
) -> Result<DeliveryView, AppError> {
    require_person(caller)?;
    let mut transaction = state.pool.begin().await?;
    let current = owned(&mut *transaction, caller, webhook_id).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO webhook_deliveries (
            id, subscription_id, organization_id, project_id, event_type, event_key, subject
        ) VALUES ($1, $2, $3, $4, 'ping', $5, '{}'::jsonb)
        "#,
    )
    .bind(id)
    .bind(webhook_id)
    .bind(current.organization_id)
    .bind(current.project_id)
    .bind(format!("ping:{id}"))
    .execute(&mut *transaction)
    .await?;
    queue_delivery(&mut *transaction, current.organization_id, id).await?;
    transaction.commit().await?;
    delivery(&state.pool, webhook_id, id).await
}

/// Sends a delivery again under the same id.
pub(crate) async fn redeliver(
    state: &AppState,
    caller: &Caller,
    webhook_id: Uuid,
    delivery_id: Uuid,
) -> Result<DeliveryView, AppError> {
    require_person(caller)?;
    let mut transaction = state.pool.begin().await?;
    let current = owned(&mut *transaction, caller, webhook_id).await?;
    // Through an app, the app's limit decides as well, whatever the webhook
    // was made through: it may not resend a task without "see tasks", or a
    // pull request without "see code", and the delivery remembers that an
    // app asked, so sending it checks that limit again.
    let app = through_app(caller);
    if app {
        let Some((project_id, subject)) = sqlx::query_as::<_, (Option<Uuid>, Value)>(
            "SELECT project_id, subject FROM webhook_deliveries WHERE id = $1 AND subscription_id = $2",
        )
        .bind(delivery_id)
        .bind(webhook_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Err(AppError::NotFound("delivery"));
        };
        let subject: Subject = serde_json::from_value(subject).unwrap_or_default();
        if let Some(project_id) = project_id
            && app_limit_hides(&state.pool, &current, project_id, &subject, true).await?
        {
            return Err(AppError::NotFound("delivery"));
        }
    }
    let updated = sqlx::query(
        r#"
        UPDATE webhook_deliveries
        SET state = 'pending', last_error = NULL, updated_at = now(),
            made_through_app = made_through_app OR $3
        WHERE id = $1 AND subscription_id = $2 AND state <> 'pending'
        "#,
    )
    .bind(delivery_id)
    .bind(webhook_id)
    .bind(app)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() == 1 {
        queue_delivery(&mut *transaction, current.organization_id, delivery_id).await?;
    }
    transaction.commit().await?;
    delivery(&state.pool, webhook_id, delivery_id).await
}

const DELIVERY_COLUMNS: &str = r#"
    id, event_type, project_id, subject, state, attempts, last_status_code,
    last_error, duration_ms, occurred_at, created_at, delivered_at
"#;

async fn delivery(
    pool: &PgPool,
    webhook_id: Uuid,
    delivery_id: Uuid,
) -> Result<DeliveryView, AppError> {
    sqlx::query_as::<_, DeliveryView>(&format!(
        "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries WHERE id = $1 AND subscription_id = $2"
    ))
    .bind(delivery_id)
    .bind(webhook_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("delivery"))
}

/// The delivery log, newest first, by cursor.
pub(crate) async fn deliveries(
    state: &AppState,
    caller: &Caller,
    webhook_id: Uuid,
    cursor: Option<&str>,
    limit: Option<i64>,
) -> Result<DeliveryPage, AppError> {
    owned(&state.pool, caller, webhook_id).await?;
    let limit = limit.unwrap_or(50).clamp(1, MAX_LOG_PAGE);
    let after = cursor
        .map(|cursor| Uuid::parse_str(cursor).map_err(|_| AppError::Validation("cursor")))
        .transpose()?;
    let mut rows = sqlx::query_as::<_, DeliveryView>(&format!(
        r#"SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries
        WHERE subscription_id = $1 AND ($2::uuid IS NULL OR id < $2)
        ORDER BY id DESC LIMIT $3"#
    ))
    .bind(webhook_id)
    .bind(after)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let next_cursor = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|row| row.id.to_string())
    } else {
        None
    };
    Ok(DeliveryPage {
        deliveries: rows,
        next_cursor,
    })
}

// ---------------------------------------------------------------------------
// Fan-out

async fn queue_delivery(
    executor: impl PgExecutor<'_>,
    organization_id: Uuid,
    delivery_id: Uuid,
) -> Result<(), AppError> {
    jobs::enqueue(
        executor,
        NewJob::new(DELIVER_KIND)
            .workspace(organization_id)
            .payload(json!({ "deliveryId": delivery_id }))
            .unique(format!("webhook-delivery:{delivery_id}")),
    )
    .await?;
    Ok(())
}

/// Records a delivery for each active subscription that wants this event
/// and whose owner can see it now, and queues it. A subscription sees an
/// event once, however often it's routed.
pub(crate) async fn fan_out(state: &AppState, event: &Event) -> Result<usize, AppError> {
    let candidates = sqlx::query_as::<_, SubscriptionRow>(&format!(
        r#"{SUBSCRIPTION_SELECT}
        WHERE subscription.organization_id = $1
          AND subscription.state = 'active'
          AND (subscription.project_id IS NULL OR subscription.project_id = $2)
          AND $3 = ANY(subscription.event_types)"#
    ))
    .bind(event.organization_id)
    .bind(event.project_id)
    .bind(&event.event_type)
    .fetch_all(&state.pool)
    .await?;
    let mut queued = 0;
    for subscription in candidates {
        let subscription_id = subscription.id;
        let owner = subscription.owner_actor_id;
        // An owner who left turns it off now rather than at the next delivery.
        if let Some(reason) = owner_refusal(&state.pool, &subscription).await? {
            turn_off(&state.pool, subscription_id, reason).await?;
            continue;
        }
        if !visible_to(&state.pool, owner, event.project_id, &event.subject).await?
            || app_limit_hides(
                &state.pool,
                &subscription,
                event.project_id,
                &event.subject,
                false,
            )
            .await?
        {
            continue;
        }
        let mut transaction = state.pool.begin().await?;
        let inserted = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO webhook_deliveries (
                id, subscription_id, organization_id, project_id, event_type, event_key,
                subject, occurred_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (subscription_id, event_key) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(subscription_id)
        .bind(event.organization_id)
        .bind(event.project_id)
        .bind(&event.event_type)
        .bind(&event.key)
        .bind(serde_json::to_value(&event.subject).unwrap_or_else(|_| json!({})))
        .bind(event.occurred_at)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(delivery_id) = inserted {
            queue_delivery(&mut *transaction, event.organization_id, delivery_id).await?;
            queued += 1;
        }
        transaction.commit().await?;
    }
    Ok(queued)
}

/// Whether a person is making or changing a webhook through an app or script
/// (MCP or the public API), whose agent limit its deliveries then keep.
fn through_app(caller: &Caller) -> bool {
    caller.actor_kind == ActorKind::Human && caller.channel == "mcp"
}

/// Whether the agent limit of the app a webhook was made through (or, with
/// `through_app`, an app that asked for this delivery), as it is now, keeps
/// it from what the event is about: a task without "see tasks", or a pull
/// request without "see code", in the event's project.
async fn app_limit_hides(
    pool: &PgPool,
    subscription: &SubscriptionRow,
    project_id: Uuid,
    subject: &Subject,
    through_app: bool,
) -> Result<bool, AppError> {
    use crate::policy::{Permission, PermissionSet};
    if !(subscription.made_through_app || through_app)
        || (subject.work_item_id.is_none() && subject.pull_request_id.is_none())
    {
        return Ok(false);
    }
    let mut connection = pool.acquire().await?;
    let limits = crate::roles::app_access(&mut connection, subscription.owner_actor_id)
        .await?
        .limits;
    let allowed = limits
        .projects
        .get(&project_id)
        .copied()
        .unwrap_or(PermissionSet::EMPTY);
    Ok(
        (subject.work_item_id.is_some() && !allowed.contains(Permission::TasksView))
            || (subject.pull_request_id.is_some() && !allowed.contains(Permission::CodeView)),
    )
}

/// Whether `owner` can see what the event is about right now.
pub(crate) async fn visible_to(
    executor: impl PgExecutor<'_>,
    owner: Uuid,
    project_id: Uuid,
    subject: &Subject,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT actor_can_access_project($1, NULL, $2)
           AND ($3::uuid IS NULL OR conversation_visible_to($3, $1))
           AND ($4::uuid IS NULL OR pull_request_visible_to($4, $1))
           AND ($5::uuid IS NULL OR EXISTS (
               SELECT 1 FROM work_items item WHERE item.id = $5 AND item.project_id = $2
           ))
           -- A task's or pull request's events only while the owner's role
           -- there still lets them see tasks, or code.
           AND ($5::uuid IS NULL OR EXISTS (
               SELECT 1 FROM project_memberships sees_task
               WHERE sees_task.project_id = $2 AND sees_task.actor_id = $1
                 AND sees_task.enabled AND custom_role_allows(sees_task.role_id, 'tasks.view')
           ))
           AND ($4::uuid IS NULL OR EXISTS (
               SELECT 1 FROM project_memberships sees_code
               WHERE sees_code.project_id = $2 AND sees_code.actor_id = $1
                 AND sees_code.enabled AND custom_role_allows(sees_code.role_id, 'code.view')
           ))
        "#,
    )
    .bind(owner)
    .bind(project_id)
    .bind(subject.conversation_id)
    .bind(subject.pull_request_id)
    .bind(subject.work_item_id)
    .fetch_one(executor)
    .await?)
}

// ---------------------------------------------------------------------------
// Delivery

#[derive(Debug, FromRow)]
struct DueDelivery {
    id: Uuid,
    subscription_id: Uuid,
    organization_id: Uuid,
    project_id: Option<Uuid>,
    event_type: String,
    subject: Value,
    occurred_at: DateTime<Utc>,
    state: String,
    attempts: i32,
    /// An app asked to send it again: its limit is checked as well.
    made_through_app: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeliverPayload {
    delivery_id: Uuid,
}

async fn finish_delivery(
    pool: &PgPool,
    delivery_id: Uuid,
    state: &str,
    error: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE webhook_deliveries
        SET state = $2, last_error = $3, updated_at = now(),
            delivered_at = CASE WHEN $2 = 'delivered' THEN now() ELSE delivered_at END
        WHERE id = $1
        "#,
    )
    .bind(delivery_id)
    .bind(state)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn turn_off(pool: &PgPool, subscription_id: Uuid, reason: &str) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE webhook_subscriptions
        SET state = 'disabled', disabled_reason = $2, version = version + 1, updated_at = now()
        WHERE id = $1 AND state <> 'disabled'
        "#,
    )
    .bind(subscription_id)
    .bind(reason)
    .execute(pool)
    .await?;
    tracing::info!(webhook_id = %subscription_id, "webhook turned off");
    Ok(())
}

/// Why a subscription's owner can no longer have it, if they can't: they
/// left the workspace or its project, or the app that made it is gone.
async fn owner_refusal(
    pool: &PgPool,
    subscription: &SubscriptionRow,
) -> Result<Option<&'static str>, AppError> {
    let (in_workspace, in_project, app_gone) = sqlx::query_as::<_, (bool, bool, bool)>(
        r#"
        SELECT EXISTS (
                   SELECT 1 FROM organization_memberships membership
                   JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
                   JOIN users person ON person.actor_id = actor.id AND person.status = 'active'
                   WHERE membership.organization_id = $2 AND membership.actor_id = $1
                     AND membership.enabled
               ),
               $3::uuid IS NULL OR actor_can_access_project($1, NULL, $3),
               $4::uuid IS NOT NULL AND NOT EXISTS (
                   SELECT 1 FROM mcp_oauth_grants grant_row
                   WHERE grant_row.id = $4 AND grant_row.revoked_at IS NULL
               )
        "#,
    )
    .bind(subscription.owner_actor_id)
    .bind(subscription.organization_id)
    .bind(subscription.project_id)
    .bind(subscription.via_oauth_grant_id)
    .fetch_one(pool)
    .await?;
    Ok(if !in_workspace {
        Some("Turned off because its owner is no longer in this workspace.")
    } else if !in_project {
        Some("Turned off because its owner can no longer see its project.")
    } else if app_gone {
        Some("Turned off because the app that made it was disconnected.")
    } else {
        None
    })
}

async fn deliver(context: &JobContext) -> Result<(), JobError> {
    let payload: DeliverPayload = context.job.payload_as()?;
    let state = &context.state;
    let pool = &state.pool;
    let Some(due) = sqlx::query_as::<_, DueDelivery>(
        r#"
        SELECT id, subscription_id, organization_id, project_id, event_type, subject,
               occurred_at, state, attempts, made_through_app
        FROM webhook_deliveries WHERE id = $1
        "#,
    )
    .bind(payload.delivery_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(());
    };
    if due.state != "pending" {
        return Ok(());
    }
    let subscription = fetch(pool, due.subscription_id).await?;
    if subscription.state != "active" {
        finish_delivery(
            pool,
            due.id,
            "skipped",
            Some("The webhook is paused or off."),
        )
        .await?;
        return Ok(());
    }
    // Principle 9: the owner's access now, not when they subscribed.
    if let Some(reason) = owner_refusal(pool, &subscription).await? {
        turn_off(pool, subscription.id, reason).await?;
        finish_delivery(pool, due.id, "skipped", Some(reason)).await?;
        return Ok(());
    }
    let subject: Subject = serde_json::from_value(due.subject.clone()).unwrap_or_default();
    if let Some(project_id) = due.project_id
        && due.event_type != "ping"
        && (!visible_to(pool, subscription.owner_actor_id, project_id, &subject).await?
            || app_limit_hides(
                pool,
                &subscription,
                project_id,
                &subject,
                due.made_through_app,
            )
            .await?)
    {
        finish_delivery(
            pool,
            due.id,
            "skipped",
            Some("Its owner can't see what this event is about."),
        )
        .await?;
        return Ok(());
    }
    let Some(body) = build_body(state, &due, &subject).await? else {
        finish_delivery(pool, due.id, "skipped", Some("What it was about is gone.")).await?;
        return Ok(());
    };
    let Some(secret_id) = subscription.secret_id else {
        finish_delivery(
            pool,
            due.id,
            "failed",
            Some("The webhook has no signing secret."),
        )
        .await?;
        return Ok(());
    };
    let keys = secrets::keyring(&state.config)
        .map_err(|_| JobError::retry("the secrets keyring isn't configured"))?;
    let secret = secrets::reveal(pool, keys.as_ref(), secret_id, "outgoing webhook signature")
        .await
        .map_err(|error| JobError::retry(error.log_category()))?;
    let started = Instant::now();
    let sent = send_post(
        &subscription.url,
        secret.expose(),
        &due,
        &body,
        &state.config.webhook_trusted_origins,
    )
    .await;
    let elapsed = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
    let attempts = due.attempts + 1;
    match sent {
        Ok(status) => {
            sqlx::query(
                r#"
                UPDATE webhook_deliveries
                SET state = 'delivered', attempts = $2, last_status_code = $3, last_error = NULL,
                    duration_ms = $4, delivered_at = now(), updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(due.id)
            .bind(attempts)
            .bind(i32::from(status))
            .bind(elapsed)
            .execute(pool)
            .await?;
            sqlx::query(
                "UPDATE webhook_subscriptions SET consecutive_failures = 0, last_delivery_at = now() WHERE id = $1",
            )
            .bind(subscription.id)
            .execute(pool)
            .await?;
            Ok(())
        }
        Err((status, error)) => {
            let last = context.job.attempts >= context.job.max_attempts;
            sqlx::query(
                r#"
                UPDATE webhook_deliveries
                SET attempts = $2, last_status_code = $3, last_error = $4, duration_ms = $5,
                    state = CASE WHEN $6 THEN 'failed' ELSE state END, updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(due.id)
            .bind(attempts)
            .bind(status.map(i32::from))
            .bind(error)
            .bind(elapsed)
            .bind(last)
            .execute(pool)
            .await?;
            if last {
                let failures = sqlx::query_scalar::<_, i32>(
                    r#"
                    UPDATE webhook_subscriptions
                    SET consecutive_failures = consecutive_failures + 1, last_delivery_at = now()
                    WHERE id = $1 RETURNING consecutive_failures
                    "#,
                )
                .bind(subscription.id)
                .fetch_one(pool)
                .await?;
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    turn_off(
                        pool,
                        subscription.id,
                        "Turned off after 20 deliveries in a row failed. Fix the receiver, then turn it on again.",
                    )
                    .await?;
                }
                return Err(JobError::permanent(error));
            }
            Err(JobError::retry(error))
        }
    }
}

async fn send_post(
    url: &str,
    secret: &str,
    due: &DueDelivery,
    body: &Value,
    trusted_origins: &[String],
) -> Result<u16, (Option<u16>, &'static str)> {
    let (url, client) = crate::replies::webhook_client(url, trusted_origins)
        .await
        .map_err(|error| (None, error))?;
    let body = body.to_string();
    let timestamp = Utc::now().timestamp().to_string();
    let signature = crate::replies::sign(secret, &timestamp, &body)
        .ok_or((None, "could not sign the delivery"))?;
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("user-agent", "AgentWorkspace-Webhooks/1")
        .header("x-workspace-delivery", due.id.to_string())
        .header("x-workspace-event", &due.event_type)
        .header("x-workspace-timestamp", &timestamp)
        .header("x-workspace-signature", format!("sha256={signature}"))
        .body(body)
        .send()
        .await
        .map_err(|_| (None, "the receiver did not respond"))?;
    let status = response.status().as_u16();
    if response.status().is_success() {
        Ok(status)
    } else {
        Err((Some(status), "the receiver returned an error status"))
    }
}

#[derive(FromRow)]
struct TaskData {
    id: Uuid,
    key: String,
    title: String,
    priority: String,
    status: String,
    status_category: String,
    assignee_actor_id: Option<Uuid>,
    assignee_name: Option<String>,
    archived: bool,
    version: i64,
    updated_at: DateTime<Utc>,
}

pub(crate) async fn task_data(
    state: &AppState,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<Value>, AppError> {
    let Some(task) = sqlx::query_as::<_, TaskData>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key, item.title,
               item.priority, status.name AS status, status.semantic_category AS status_category,
               item.assignee_actor_id,
               (SELECT display_name FROM users WHERE actor_id = item.assignee_actor_id)
                   AS assignee_name,
               item.archived_at IS NOT NULL AS archived, item.version, item.updated_at
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(None);
    };
    let labels = sqlx::query_scalar::<_, String>(
        "SELECT name FROM work_item_active_labels WHERE project_id = $1 AND work_item_id = $2 ORDER BY lower(name)",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&state.pool)
    .await?;
    let origin = events::public_origin(state);
    Ok(Some(json!({
        "id": task.id,
        "key": task.key,
        "title": task.title,
        "status": task.status,
        "statusCategory": task.status_category,
        "priority": task.priority,
        "assignee": task.assignee_actor_id.map(|id| json!({ "id": id, "name": task.assignee_name })),
        "labels": labels,
        "archived": task.archived,
        "version": task.version,
        "updatedAt": task.updated_at,
        "url": format!("{origin}/projects/{project_id}/work-items/{}", task.id),
    })))
}

async fn build_body(
    state: &AppState,
    due: &DueDelivery,
    subject: &Subject,
) -> Result<Option<Value>, AppError> {
    let pool = &state.pool;
    let origin = events::public_origin(state);
    let project = match due.project_id {
        Some(project_id) => sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, project_key, name FROM projects WHERE id = $1",
        )
        .bind(project_id)
        .fetch_optional(pool)
        .await?
        .map(|(id, key, name)| json!({ "id": id, "key": key, "name": name })),
        None => None,
    };
    let data = match due.event_type.as_str() {
        "ping" => json!({ "message": "This is a test delivery from Agent Workspace." }),
        "task.created" | "task.updated" => {
            let (Some(project_id), Some(work_item_id)) = (due.project_id, subject.work_item_id)
            else {
                return Ok(None);
            };
            let Some(mut task) = task_data(state, project_id, work_item_id).await? else {
                return Ok(None);
            };
            if !subject.changed_fields.is_empty() {
                task["changedFields"] = json!(subject.changed_fields);
            }
            json!({ "task": task })
        }
        "run.updated" | "run.finished" => {
            let Some(run_id) = subject.agent_run_id else {
                return Ok(None);
            };
            let Some(run) = sqlx::query_as::<
                _,
                (
                    Uuid,
                    Uuid,
                    String,
                    Option<String>,
                    Option<String>,
                    DateTime<Utc>,
                    Option<DateTime<Utc>>,
                ),
            >(
                r#"
                SELECT run.id, run.conversation_id, run.state, run.config->>'harness',
                       run.failure_reason, run.created_at, run.finished_at
                FROM agent_runs run WHERE run.id = $1
                "#,
            )
            .bind(run_id)
            .fetch_optional(pool)
            .await?
            else {
                return Ok(None);
            };
            let task = match (due.project_id, subject.work_item_id) {
                (Some(project_id), Some(work_item_id)) => {
                    task_data(state, project_id, work_item_id).await?
                }
                _ => None,
            };
            let project_id = due.project_id.unwrap_or_default();
            json!({
                "run": {
                    "id": run.0,
                    "conversationId": run.1,
                    "state": subject.state.clone().unwrap_or_else(|| run.2.clone()),
                    "currentState": run.2,
                    "harness": run.3,
                    "failure": run.4,
                    "createdAt": run.5,
                    "finishedAt": run.6,
                    "url": format!("{origin}/projects/{project_id}/threads/{}", run.1),
                },
                "task": task,
            })
        }
        event if event.starts_with("pull_request.") => {
            let Some(pull_request_id) = subject.pull_request_id else {
                return Ok(None);
            };
            let Some(pr) =
                sqlx::query_as::<_, (Uuid, i64, String, String, String, String, String, String)>(
                    r#"
                SELECT pr.id, pr.pull_request_number, pr.title, pr.html_url, pr.state,
                       pr.checks_state, pr.head_sha, pr.head_ref
                FROM github_pull_requests pr WHERE pr.id = $1
                "#,
                )
                .bind(pull_request_id)
                .fetch_optional(pool)
                .await?
            else {
                return Ok(None);
            };
            json!({
                "pullRequest": {
                    "id": pr.0,
                    "number": pr.1,
                    "title": pr.2,
                    "url": pr.3,
                    "state": pr.4,
                    "checksState": pr.5,
                    "headSha": pr.6,
                    "headRef": pr.7,
                }
            })
        }
        "message.created" => {
            let (Some(conversation_id), Some(message_id)) =
                (subject.conversation_id, subject.message_id)
            else {
                return Ok(None);
            };
            let Some(message) = sqlx::query_as::<_, (Uuid, Uuid, Option<String>, String, bool, DateTime<Utc>)>(
                r#"
                SELECT message.id, message.author_actor_id,
                       COALESCE(
                           (SELECT display_name FROM users WHERE actor_id = message.author_actor_id),
                           (SELECT membership.display_name FROM project_memberships membership
                            JOIN conversations conversation ON conversation.id = message.conversation_id
                            WHERE membership.project_id = conversation.project_id
                              AND membership.actor_id = message.author_actor_id)
                       ),
                       message.plain_text, message.removed_at IS NOT NULL, message.created_at
                FROM messages message
                WHERE message.id = $1 AND message.conversation_id = $2
                "#,
            )
            .bind(message_id)
            .bind(conversation_id)
            .fetch_optional(pool)
            .await?
            else {
                return Ok(None);
            };
            if message.4 {
                // Removed since: its text must not leave.
                return Ok(None);
            }
            let project_id = due.project_id.unwrap_or_default();
            json!({
                "message": {
                    "id": message.0,
                    "conversationId": conversation_id,
                    "author": { "id": message.1, "name": message.2 },
                    "text": message.3.chars().take(MESSAGE_TEXT_CHARACTERS).collect::<String>(),
                    "createdAt": message.5,
                    "url": format!("{origin}/projects/{project_id}/conversations/{conversation_id}#message-{}", message.0),
                }
            })
        }
        _ => return Ok(None),
    };
    Ok(Some(json!({
        "id": due.id,
        "type": due.event_type,
        "occurredAt": due.occurred_at,
        "workspaceId": due.organization_id,
        "project": project,
        "data": data,
    })))
}

// ---------------------------------------------------------------------------
// HTTP

async fn list_route(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<WebhookList>, AppError> {
    let caller = Caller::browser(&principal)?;
    Ok(Json(list(&state, &caller).await?))
}

async fn create_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<CreateWebhook>,
) -> Result<(StatusCode, Json<CreatedWebhook>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok((
        StatusCode::CREATED,
        Json(create(&state, &caller, request).await?),
    ))
}

async fn update_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(webhook_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateWebhook>,
) -> Result<Json<WebhookView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok(Json(update(&state, &caller, webhook_id, request).await?))
}

async fn delete_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(webhook_id): ApiPath<Uuid>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    delete(&state, &caller, webhook_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rotate_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(webhook_id): ApiPath<Uuid>,
) -> Result<Json<CreatedWebhook>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok(Json(rotate_secret(&state, &caller, webhook_id).await?))
}

async fn test_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(webhook_id): ApiPath<Uuid>,
) -> Result<(StatusCode, Json<DeliveryView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(send_test(&state, &caller, webhook_id).await?),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeliveriesQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

async fn deliveries_route(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(webhook_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<DeliveriesQuery>,
) -> Result<Json<DeliveryPage>, AppError> {
    let caller = Caller::browser(&principal)?;
    Ok(Json(
        deliveries(
            &state,
            &caller,
            webhook_id,
            query.cursor.as_deref(),
            query.limit,
        )
        .await?,
    ))
}

async fn redeliver_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((webhook_id, delivery_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<DeliveryView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(redeliver(&state, &caller, webhook_id, delivery_id).await?),
    ))
}

#[cfg(test)]
mod tests {
    use super::{new_secret, normalize_events, normalize_name};

    #[test]
    fn events_must_be_known_and_names_short() {
        assert_eq!(
            normalize_events(&["run.finished".to_owned(), "run.finished".to_owned()])
                .expect("events"),
            vec!["run.finished".to_owned()]
        );
        assert!(normalize_events(&[]).is_err());
        assert!(normalize_events(&["run.exploded".to_owned()]).is_err());
        assert!(normalize_name("  CI bridge ").is_ok());
        assert!(normalize_name("").is_err());
        assert!(normalize_name(&"x".repeat(81)).is_err());
    }

    #[test]
    fn secrets_are_long_and_distinct() {
        let first = new_secret();
        assert!(first.starts_with("whsec_") && first.len() > 40);
        assert_ne!(first, new_secret());
    }
}

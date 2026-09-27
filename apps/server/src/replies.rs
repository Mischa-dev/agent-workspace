//! Reply requests: which agents are asked to answer, how they claim the work,
//! and how they are woken up.
//!
//! The rules come from the earlier Workspace app, where they were live-tested:
//! - A human DM to an agent asks that agent for a required reply.
//! - A human message in a group asks every agent member for an optional reply.
//!   Agents may pass, and a pass posts nothing.
//! - Agent messages in a group ask the other agents only while the
//!   conversation's turn allowance lasts. Each human message resets it.
//! - A newer message supersedes an agent's still-pending request in the same
//!   conversation, so nobody answers a stale question.
//! - Claiming starts a lease. A lapsed lease becomes "expired" and waits for a
//!   person to retry it, rather than being handed out again silently.

use std::time::Duration;

use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{ActorView, enqueue_event, lock_conversation_access},
    error::{ApiPath, AppError},
    external_events::enqueue_external_event,
    outbox::{OutboxEventPayload, ReplyRequestsUpdatedPayload},
    policy::Action,
};

/// Agent messages allowed in a group before a person has to say Continue.
pub(crate) const TURN_ALLOWANCE: i32 = 6;
pub(crate) const LEASE_MINUTES: i32 = 15;
const WORKER_INTERVAL: Duration = Duration::from_secs(3);
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(8);
const WEBHOOK_MAX_ATTEMPTS: i32 = 8;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/conversations/{id}/continue",
            post(continue_conversation),
        )
        .route("/api/v1/reply-requests/{id}/retry", post(retry_request))
}

/// A message that was just written, as seen by the reply rules.
pub(crate) struct NewMessage<'a> {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub conversation_kind: &'a str,
    pub message_id: Uuid,
    pub author_actor_id: Uuid,
    pub author_kind: &'a str,
    pub mentioned_actor_ids: &'a [Uuid],
}

/// Ask the right agents to reply. Runs in the message's own transaction.
pub(crate) async fn ask_for_replies(
    transaction: &mut Transaction<'_, Postgres>,
    message: NewMessage<'_>,
) -> Result<(), AppError> {
    if matches!(message.conversation_kind, "channel" | "thread") {
        // Project discussions ask only agents explicitly mentioned by a human.
        // DMs and groups retain their existing delivery and turn rules below.
        return if message.author_kind == "human" && !message.mentioned_actor_ids.is_empty() {
            ask(transaction, &message, true).await
        } else {
            Ok(())
        };
    }
    if !matches!(message.conversation_kind, "dm" | "group") {
        return Ok(());
    }
    let from_agent = message.author_kind == "external_agent";
    if from_agent {
        if message.conversation_kind == "dm" {
            return Ok(());
        }
        let remaining = sqlx::query_scalar::<_, i32>(
            r#"
            UPDATE conversations
            SET agent_turns_remaining = GREATEST(agent_turns_remaining - 1, 0)
            WHERE id = $1
            RETURNING agent_turns_remaining
            "#,
        )
        .bind(message.conversation_id)
        .fetch_one(&mut **transaction)
        .await?;
        if remaining == 0 {
            // Allowance used up: the conversation waits for a person.
            return notify(transaction, message.project_id, message.conversation_id).await;
        }
    } else {
        sqlx::query("UPDATE conversations SET agent_turns_remaining = $2 WHERE id = $1")
            .bind(message.conversation_id)
            .bind(TURN_ALLOWANCE)
            .execute(&mut **transaction)
            .await?;
    }
    let required = !from_agent && message.conversation_kind == "dm";
    ask(transaction, &message, required).await
}

async fn ask(
    transaction: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
    required: bool,
) -> Result<(), AppError> {
    let agents = if matches!(message.conversation_kind, "channel" | "thread") {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT membership.actor_id FROM project_memberships membership JOIN actors actor ON actor.id=membership.actor_id WHERE membership.project_id=$1 AND membership.enabled AND actor.kind='external_agent' AND actor.status='active' AND actor.id=ANY($2) AND conversation_visible_to($3,actor.id) ORDER BY actor.id"
        ).bind(message.project_id).bind(message.mentioned_actor_ids).bind(message.conversation_id).fetch_all(&mut **transaction).await?
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"
        SELECT member.actor_id
        FROM conversation_members member
        JOIN actors actor ON actor.id = member.actor_id
        JOIN project_memberships membership
          ON membership.project_id = member.project_id
         AND membership.actor_id = member.actor_id
        WHERE member.conversation_id = $1
          AND member.actor_id <> $2
          AND actor.kind = 'external_agent'
          AND actor.status = 'active'
          AND membership.enabled
        ORDER BY member.actor_id
        "#,
        )
        .bind(message.conversation_id)
        .bind(message.author_actor_id)
        .fetch_all(&mut **transaction)
        .await?
    };
    if agents.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE reply_requests
        SET status = 'superseded', updated_at = now()
        WHERE conversation_id = $1
          AND status = 'pending'
          AND agent_actor_id = ANY($2)
        "#,
    )
    .bind(message.conversation_id)
    .bind(&agents)
    .execute(&mut **transaction)
    .await?;
    for agent in agents {
        // An agent already asked about this message (possible after Continue)
        // keeps its existing request.
        let inserted = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO reply_requests (
                id, project_id, conversation_id, message_id, agent_actor_id,
                required, mentioned
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (message_id, agent_actor_id) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(message.project_id)
        .bind(message.conversation_id)
        .bind(message.message_id)
        .bind(agent)
        .bind(required)
        .bind(message.mentioned_actor_ids.contains(&agent))
        .fetch_optional(&mut **transaction)
        .await?;
        if let Some(request_id) = inserted {
            wake(
                transaction,
                message.project_id,
                message.conversation_id,
                agent,
                request_id,
            )
            .await?;
        }
    }
    notify(transaction, message.project_id, message.conversation_id).await
}

/// Tell each of the agent's active connections that work is waiting: an inbox
/// event for pollers, plus a webhook delivery if the connection has one.
async fn wake(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    agent_actor_id: Uuid,
    request_id: Uuid,
) -> Result<(), AppError> {
    let connections = sqlx::query_as::<_, (Uuid, bool)>(
        r#"
        SELECT id, webhook_url IS NOT NULL
        FROM external_agent_connections
        WHERE agent_connection_can_access(id, $1) AND actor_id = $2 AND status = 'active'
        "#,
    )
    .bind(project_id)
    .bind(agent_actor_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (connection_id, has_webhook) in connections {
        enqueue_external_event(
            transaction,
            connection_id,
            project_id,
            "reply.requested",
            "You were asked to reply",
            "reply_request",
            request_id,
            &format!("/messages/{conversation_id}"),
        )
        .await?;
        if has_webhook {
            sqlx::query(
                r#"
                INSERT INTO agent_webhook_deliveries (id, connection_id, reply_request_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (connection_id, reply_request_id) DO UPDATE
                SET attempts = 0, next_attempt_at = now(), delivered_at = NULL,
                    last_error = NULL
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(connection_id)
            .bind(request_id)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(())
}

/// Push a refresh to browsers watching this conversation.
pub(crate) async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    enqueue_event(
        transaction,
        project_id,
        OutboxEventPayload::ReplyRequestsUpdated(ReplyRequestsUpdatedPayload { conversation_id }),
    )
    .await
}

/// Mark the agent's claimed request answered by `reply_message_id`. Fails when
/// the claim is gone (expired, superseded, or already answered), so the
/// caller's transaction rolls the message back.
pub(crate) async fn complete_with_reply(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
    generation: i32,
    agent_actor_id: Uuid,
    conversation_id: Uuid,
    reply_message_id: Uuid,
) -> Result<(), AppError> {
    let updated = sqlx::query(
        r#"
        UPDATE reply_requests
        SET status = 'replied', reply_message_id = $5, lease_expires_at = NULL,
            updated_at = now()
        WHERE id = $1 AND generation = $2 AND agent_actor_id = $3
          AND conversation_id = $4 AND status = 'claimed'
        "#,
    )
    .bind(request_id)
    .bind(generation)
    .bind(agent_actor_id)
    .bind(conversation_id)
    .bind(reply_message_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "reply request",
            "not claimed by this agent any more",
        ))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyRequestView {
    pub id: Uuid,
    pub message_id: Uuid,
    pub agent: ActorView,
    pub required: bool,
    pub mentioned: bool,
    pub status: String,
    pub reason: Option<String>,
    pub updated_at: String,
    /// When the agent last used any of its connections.
    pub agent_last_seen_at: Option<String>,
}

#[derive(Debug, FromRow)]
struct ReplyRequestRow {
    id: Uuid,
    message_id: Uuid,
    agent_actor_id: Uuid,
    agent_name: String,
    agent_handle: String,
    required: bool,
    mentioned: bool,
    status: String,
    reason: Option<String>,
    updated_at: String,
    agent_last_seen_at: Option<String>,
}

/// Recent requests for a conversation's timeline, newest first.
pub(crate) async fn timeline_requests(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<(Vec<ReplyRequestView>, i32), AppError> {
    let rows = sqlx::query_as::<_, ReplyRequestRow>(
        r#"
        SELECT request.id, request.message_id, request.agent_actor_id,
               membership.display_name AS agent_name,
               membership.handle AS agent_handle,
               request.required, request.mentioned, request.status, request.reason,
               request.updated_at::text AS updated_at,
               (
                   SELECT max(connection.last_used_at)::text
                   FROM external_agent_connections connection
                   WHERE agent_connection_can_access(connection.id, request.project_id)
                     AND connection.actor_id = request.agent_actor_id
               ) AS agent_last_seen_at
        FROM reply_requests request
        JOIN project_memberships membership
          ON membership.project_id = request.project_id
         AND membership.actor_id = request.agent_actor_id
        WHERE request.conversation_id = $1
          AND request.status <> 'superseded'
        ORDER BY request.created_at DESC, request.id
        LIMIT 60
        "#,
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;
    let remaining = sqlx::query_scalar::<_, i32>(
        "SELECT agent_turns_remaining FROM conversations WHERE id = $1",
    )
    .bind(conversation_id)
    .fetch_one(pool)
    .await?;
    let views = rows
        .into_iter()
        .map(|row| ReplyRequestView {
            id: row.id,
            message_id: row.message_id,
            agent: ActorView {
                id: row.agent_actor_id,
                kind: "external_agent".to_owned(),
                display_name: row.agent_name,
                handle: row.agent_handle,
                avatar_url: None,
            },
            required: row.required,
            mentioned: row.mentioned,
            status: row.status,
            reason: row.reason,
            updated_at: row.updated_at,
            agent_last_seen_at: row.agent_last_seen_at,
        })
        .collect();
    Ok((views, remaining))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContinueResponse {
    agent_turns_remaining: i32,
}

/// Give the agents another round after the allowance ran out, and ask the other
/// agents to respond to the latest agent message.
async fn continue_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
) -> Result<Json<ContinueResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let conversation = lock_conversation_access(
        &mut transaction,
        &principal,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await?;
    if conversation.kind != "group" {
        return Err(AppError::Conflict("conversation", "only groups have turns"));
    }
    sqlx::query("UPDATE conversations SET agent_turns_remaining = $2 WHERE id = $1")
        .bind(conversation_id)
        .bind(TURN_ALLOWANCE)
        .execute(&mut *transaction)
        .await?;
    let latest = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        r#"
        SELECT message.id, message.author_actor_id, actor.kind
        FROM conversation_entries entry
        JOIN messages message ON message.id = entry.message_id
        JOIN actors actor ON actor.id = message.author_actor_id
        WHERE entry.conversation_id = $1
        ORDER BY entry.sequence DESC
        LIMIT 1
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some((message_id, author_actor_id, author_kind)) = latest
        && author_kind == "external_agent"
    {
        ask(
            &mut transaction,
            &NewMessage {
                project_id: conversation.project_id,
                conversation_id,
                conversation_kind: "group",
                message_id,
                author_actor_id,
                author_kind: &author_kind,
                mentioned_actor_ids: &[],
            },
            false,
        )
        .await?;
    }
    notify(&mut transaction, conversation.project_id, conversation_id).await?;
    transaction.commit().await?;
    Ok(Json(ContinueResponse {
        agent_turns_remaining: TURN_ALLOWANCE,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RetryResponse {
    id: Uuid,
    status: &'static str,
}

/// Put an expired or released request back in the agent's inbox.
async fn retry_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(request_id): ApiPath<Uuid>,
) -> Result<Json<RetryResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let (conversation_id, agent_actor_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT conversation_id, agent_actor_id FROM reply_requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("reply request"))?;
    let conversation = lock_conversation_access(
        &mut transaction,
        &principal,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE reply_requests
        SET status = 'pending', reason = NULL, updated_at = now()
        WHERE id = $1 AND status IN ('expired', 'released')
        "#,
    )
    .bind(request_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "reply request",
            "only expired or released requests can be retried",
        ));
    }
    wake(
        &mut transaction,
        conversation.project_id,
        conversation_id,
        agent_actor_id,
        request_id,
    )
    .await?;
    notify(&mut transaction, conversation.project_id, conversation_id).await?;
    transaction.commit().await?;
    Ok(Json(RetryResponse {
        id: request_id,
        status: "pending",
    }))
}

/// Background loop: expire lapsed claims and deliver webhook wakeups.
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let client = match reqwest::Client::builder()
        .timeout(WEBHOOK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "reply worker could not build its HTTP client");
            return;
        }
    };
    loop {
        if let Err(error) = expire_leases(&state.pool).await {
            tracing::warn!(category = error.log_category(), "reply lease sweep failed");
        }
        if let Err(error) = deliver_webhooks(&state.pool, &client).await {
            tracing::warn!(category = error.log_category(), "webhook delivery failed");
        }
        tokio::select! {
            _ = tokio::time::sleep(WORKER_INTERVAL) => {}
            _ = shutdown.changed() => return,
        }
        if *shutdown.borrow() {
            return;
        }
    }
}

async fn expire_leases(pool: &PgPool) -> Result<(), AppError> {
    let mut transaction = pool.begin().await?;
    let expired = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        UPDATE reply_requests
        SET status = 'expired', lease_expires_at = NULL, updated_at = now(),
            reason = 'The agent stopped checking in before replying.'
        WHERE status = 'claimed' AND lease_expires_at < now()
        RETURNING project_id, conversation_id
        "#,
    )
    .fetch_all(&mut *transaction)
    .await?;
    let mut seen = std::collections::HashSet::new();
    for (project_id, conversation_id) in expired {
        if seen.insert(conversation_id) {
            notify(&mut transaction, project_id, conversation_id).await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

#[derive(Debug, FromRow)]
struct DueDelivery {
    id: Uuid,
    connection_id: Uuid,
    reply_request_id: Uuid,
    attempts: i32,
    agent_actor_id: Uuid,
    request_pending: bool,
    webhook_url: Option<String>,
    webhook_secret: Option<String>,
}

async fn deliver_webhooks(pool: &PgPool, client: &reqwest::Client) -> Result<(), AppError> {
    let mut transaction = pool.begin().await?;
    let due = sqlx::query_as::<_, DueDelivery>(
        r#"
        SELECT delivery.id, delivery.connection_id, delivery.reply_request_id,
               delivery.attempts, request.agent_actor_id,
               (request.status = 'pending' AND agent_connection_can_access(connection.id, request.project_id) AND conversation_visible_to(request.conversation_id, request.agent_actor_id)) AS request_pending,
               connection.webhook_url, connection.webhook_secret
        FROM agent_webhook_deliveries delivery
        JOIN reply_requests request ON request.id = delivery.reply_request_id
        JOIN external_agent_connections connection ON connection.id = delivery.connection_id
        WHERE delivery.delivered_at IS NULL
          AND delivery.next_attempt_at <= now()
          AND delivery.attempts < $1
        ORDER BY delivery.next_attempt_at
        LIMIT 10
        FOR UPDATE OF delivery SKIP LOCKED
        "#,
    )
    .bind(WEBHOOK_MAX_ATTEMPTS)
    .fetch_all(&mut *transaction)
    .await?;
    for delivery in due {
        let (Some(url), Some(secret), true) = (
            delivery.webhook_url.as_deref(),
            delivery.webhook_secret.as_deref(),
            delivery.request_pending,
        ) else {
            // Answered, superseded, or the webhook was removed: nothing to wake.
            sqlx::query("UPDATE agent_webhook_deliveries SET delivered_at = now() WHERE id = $1")
                .bind(delivery.id)
                .execute(&mut *transaction)
                .await?;
            continue;
        };
        let error = post_wakeup(client, url, secret, &delivery).await.err();
        let attempts = delivery.attempts + 1;
        let backoff_seconds = (5_i64 << attempts.min(6)).min(300);
        sqlx::query(
            r#"
            UPDATE agent_webhook_deliveries
            SET attempts = $2,
                next_attempt_at = now() + make_interval(secs => $3),
                delivered_at = CASE WHEN $4::text IS NULL THEN now() END,
                last_error = $4
            WHERE id = $1
            "#,
        )
        .bind(delivery.id)
        .bind(attempts)
        .bind(backoff_seconds as f64)
        .bind(error)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE external_agent_connections
            SET last_wakeup_at = now(), last_wakeup_error = $2
            WHERE id = $1
            "#,
        )
        .bind(delivery.connection_id)
        .bind(error)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn post_wakeup(
    client: &reqwest::Client,
    url: &str,
    secret: &str,
    delivery: &DueDelivery,
) -> Result<(), &'static str> {
    let body = serde_json::json!({
        "event": "inbox.available",
        "deliveryId": delivery.id,
        "agentId": delivery.agent_actor_id,
        "replyRequestId": delivery.reply_request_id,
    })
    .to_string();
    let timestamp = unix_timestamp();
    let signature = sign(secret, &timestamp, &body).ok_or("could not sign the wakeup")?;
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-workspace-delivery", delivery.id.to_string())
        .header("x-workspace-timestamp", &timestamp)
        .header("x-workspace-signature", format!("sha256={signature}"))
        .body(body)
        .send()
        .await
        .map_err(|_| "the webhook did not respond")?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err("the webhook returned an error status")
    }
}

/// HMAC-SHA256 over "{timestamp}.{body}", hex encoded.
fn sign(secret: &str, timestamp: &str, body: &str) -> Option<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body.as_bytes());
    Some(
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().to_string())
        .unwrap_or_default()
}

/// A new random webhook signing secret, shown to the owner once.
pub(crate) fn new_webhook_secret() -> String {
    use rand_core::{OsRng, RngCore};
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("whsec_{}", URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_cover_timestamp_and_body() {
        let signature = sign("secret", "1700000000", "{}").expect("sign");
        assert_eq!(signature.len(), 64);
        assert_ne!(Some(signature.clone()), sign("secret", "1700000001", "{}"));
        assert_ne!(Some(signature), sign("other", "1700000000", "{}"));
    }
}

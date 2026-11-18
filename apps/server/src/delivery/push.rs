//! Webhook pushes (A2 3.5), on the job queue so no transaction is held
//! across a post.
//!
//! The `delivery.push` consumer reads the log after the sequencer and, for
//! each batch, queues one `delivery.push` job per webhook connection that
//! could hear any of its events. The job reads the connection's events with
//! the one read path from its `push_cursor`, records a `deliveries` row for
//! each, moves the cursor and commits; then claims what is due, posts (at
//! most four at once, nothing open in the database), and records each
//! result in a short transaction of its own. Failures are retried over
//! about a day (0 s, 5 s, 5 min, 30 min, 2 h, 5 h, 10 h, 10 h), then marked
//! failed; a failed push is still in the log for the next poll or stream.
//! After 48 hours and 20 failures in a row, pushes stop and the account
//! hears `connection.push_disabled` on its other transports.
//!
//! A sweep every 30 seconds queues what a running job may have missed and
//! the retries that came due, and retires signing secrets a rotation left.
//!
//! Formats: `standard` posts the envelope with Standard Webhooks headers
//! and the older `x-workspace-*` headers as aliases. `typed_v1` and
//! `inbox_v1` post the older reply wakeup bodies, for reply events only,
//! with the older headers only.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    connections::{self, Connection},
    envelope::{self, Lookups},
    read::{self, Delivered, Scope},
    reason::Reason,
    sign,
};
use crate::{
    AppState,
    error::AppError,
    events::{ConsumerError, LoggedEvent},
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    tenancy::{self, Tenancy},
};

pub const PUSH_KIND: &str = "delivery.push";
pub const SWEEP_KIND: &str = "delivery.sweep";
pub const PING_KIND: &str = "delivery.ping";
/// Attempts before a delivery is marked failed.
pub const MAX_ATTEMPTS: i32 = 8;
/// Seconds to wait after each failed attempt, by attempts made so far.
const RETRY_AFTER: [i64; 7] = [5, 300, 1_800, 7_200, 18_000, 36_000, 36_000];
/// Posts in flight for one connection.
const IN_FLIGHT: i64 = 4;
/// Events queued per read.
const QUEUE_BATCH: i64 = 100;
/// How long a claimed delivery is left to its poster.
const CLAIM_SECONDS: f64 = 60.0;
const DISABLE_AFTER_FAILURES: i32 = 20;
const DISABLE_AFTER_SECONDS: f64 = 48.0 * 3600.0;
/// Rounds one job does before leaving the rest to the next.
const ROUNDS: usize = 20;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PushJob {
    connection_id: Uuid,
    account_id: Uuid,
}

fn push_job(connection_id: Uuid, account_id: Uuid, workspace_id: Uuid) -> NewJob {
    NewJob::new(PUSH_KIND)
        .payload(PushJob {
            connection_id,
            account_id,
        })
        .workspace(workspace_id)
        .unique(format!("{PUSH_KIND}:{connection_id}"))
        .max_attempts(3)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(PUSH_KIND, |context: JobContext| async move {
        let job: PushJob = context.job.payload_as()?;
        let state = context.state.clone();
        tenancy::scoped(
            Tenancy::Member {
                account: job.account_id,
                workspaces: None,
            },
            async move { push(&state, job.connection_id).await },
        )
        .await
        .map_err(|error| JobError::retry(error.log_category()))
    });
    registry.register(PING_KIND, |context: JobContext| async move {
        let job: PushJob = context.job.payload_as()?;
        let state = context.state.clone();
        tenancy::scoped(
            Tenancy::Member {
                account: job.account_id,
                workspaces: None,
            },
            async move { ping(&state, job.connection_id).await },
        )
        .await
        .map_err(|error| JobError::retry(error.log_category()))
    });
    registry.register(SWEEP_KIND, |context: JobContext| async move {
        sweep(&context.state.pool)
            .await
            .map_err(|error| JobError::retry(error.log_category()))
    });
    registry.every(SWEEP_KIND, Duration::from_secs(30));
}

/// The log consumer that wakes the pusher.
pub struct Pusher;

#[async_trait]
impl crate::events::Consumer for Pusher {
    fn name(&self) -> &'static str {
        PUSH_KIND
    }

    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        batch: &[LoggedEvent],
    ) -> Result<(), ConsumerError> {
        let (workspaces, audiences): (Vec<Uuid>, Vec<Option<Uuid>>) = batch
            .iter()
            .filter(|event| {
                event.class == "fact" && super::catalogue::get(&event.event_type).is_some()
            })
            .map(|event| (event.workspace_id, event.audience_account_id))
            .unzip();
        if workspaces.is_empty() {
            return Ok(());
        }
        let woken = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
            r#"
            SELECT DISTINCT c.id, c.account_id, c.home_workspace_id
            FROM agent_connections c
            JOIN unnest($1::uuid[], $2::uuid[]) AS e (workspace_id, audience) ON true
            WHERE c.state = 'active' AND c.webhook_url IS NOT NULL
              AND (e.audience = c.account_id
                   OR (e.audience IS NULL
                       AND (c.reaches_all OR c.home_workspace_id = e.workspace_id)
                       AND EXISTS (
                           SELECT 1 FROM organization_memberships m
                           WHERE m.organization_id = e.workspace_id
                             AND m.actor_id = c.account_id AND m.enabled)))
            "#,
        )
        .bind(&workspaces)
        .bind(&audiences)
        .fetch_all(&mut **transaction)
        .await?;
        for (connection_id, account_id, workspace_id) in woken {
            crate::jobs::enqueue(
                &mut **transaction,
                push_job(connection_id, account_id, workspace_id),
            )
            .await?;
        }
        Ok(())
    }
}

/// Queue what a running job may have missed and the retries now due, and
/// retire signing secrets whose day of overlap is over.
pub async fn sweep(pool: &sqlx::PgPool) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let due = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        r#"
        SELECT c.id, c.account_id, c.home_workspace_id
        FROM agent_connections c
        CROSS JOIN LATERAL (
            SELECT EXISTS (
                SELECT 1 FROM deliveries d
                WHERE d.connection_id = c.id AND d.state = 'pending'
                  AND d.next_attempt_at <= now()) AS retry_due
        ) due
        WHERE c.state = 'active' AND c.webhook_url IS NOT NULL
          AND (due.retry_due
               OR c.push_cursor < (SELECT next_position - 1 FROM event_position_counter))
        -- Retries first, then the connections furthest behind.
        ORDER BY due.retry_due DESC, c.push_cursor, c.id
        LIMIT 1000
        "#,
    )
    .fetch_all(&mut *transaction)
    .await?;
    for (connection_id, account_id, workspace_id) in due {
        crate::jobs::enqueue(
            &mut *transaction,
            push_job(connection_id, account_id, workspace_id),
        )
        .await?;
    }
    let retired = sqlx::query_scalar::<_, Uuid>(
        "DELETE FROM connection_signing_secrets \
         WHERE state = 'retiring' AND retire_at <= now() RETURNING secret_id",
    )
    .fetch_all(&mut *transaction)
    .await?;
    for secret_id in retired {
        crate::secrets::delete(&mut *transaction, secret_id).await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Push to a connection now: after a redelivery is asked for.
pub(crate) async fn queue_push(
    connection: &mut PgConnection,
    delivery: &Connection,
) -> Result<(), AppError> {
    crate::jobs::enqueue(
        &mut *connection,
        push_job(delivery.id, delivery.account_id, delivery.home_workspace_id),
    )
    .await?;
    Ok(())
}

/// Send a connection's webhook a test ping, on the job queue; the answer is
/// kept on the connection (`last_test_*`).
pub(crate) async fn queue_ping(
    connection: &mut PgConnection,
    delivery: &Connection,
) -> Result<(), AppError> {
    crate::jobs::enqueue(
        &mut *connection,
        NewJob::new(PING_KIND)
            .payload(PushJob {
                connection_id: delivery.id,
                account_id: delivery.account_id,
            })
            .workspace(delivery.home_workspace_id)
            .unique(format!("{PING_KIND}:{}", delivery.id))
            .max_attempts(1),
    )
    .await?;
    Ok(())
}

/// Pings a connection's webhook, holding nothing open while it waits, and
/// keeps what it answered.
async fn ping(state: &AppState, connection_id: Uuid) -> Result<(), AppError> {
    let (delivery, secrets) = {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let Some(delivery) = connections::load_any(&mut transaction, connection_id).await? else {
            return Ok(());
        };
        if delivery.state == "removed" || delivery.webhook_url.is_none() {
            return Ok(());
        }
        let secrets = signing_secrets(&mut transaction, state, delivery.id).await?;
        transaction.commit().await?;
        (delivery, secrets)
    };
    let url = delivery.webhook_url.clone().unwrap_or_default();
    let answered = send_test(
        &delivery,
        &url,
        &secrets,
        &state.config.webhook_trusted_origins,
    )
    .await;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    sqlx::query(
        "UPDATE agent_connections SET last_test_at = now(), last_test_status = $2, \
         last_test_error = $3 WHERE id = $1",
    )
    .bind(delivery.id)
    .bind(answered.status.map(i32::from))
    .bind(
        answered
            .error
            .map(|error| error.chars().take(200).collect::<String>()),
    )
    .execute(&mut *transaction)
    .await?;
    // The account's open pages show the answer.
    tell_account(
        &mut transaction,
        &delivery,
        &crate::events::catalog::CONNECTION_CHANGED,
        Some((delivery.id, "connection")),
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// One connection's pushes until nothing is left to queue or due.
pub async fn push(state: &AppState, connection_id: Uuid) -> Result<(), AppError> {
    for _ in 0..ROUNDS {
        let Some(queued) = queue(state, connection_id).await? else {
            return Ok(());
        };
        let claimed = claim(state, connection_id).await?;
        if claimed.posts.is_empty() && !queued.has_more {
            return Ok(());
        }
        let trusted = &state.config.webhook_trusted_origins;
        let results = futures_util::future::join_all(
            claimed
                .posts
                .iter()
                .map(|post| send(post, &claimed.url, &claimed.secrets, trusted)),
        )
        .await;
        record(state, &claimed, results).await?;
    }
    Ok(())
}

/// What queueing found.
struct Queued {
    has_more: bool,
}

/// Records a delivery for each event the connection hears after its push
/// cursor and moves the cursor. `None` when another job holds the
/// connection, or it no longer pushes.
async fn queue(state: &AppState, connection_id: Uuid) -> Result<Option<Queued>, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(delivery) = sqlx::query_as::<_, Connection>(&format!(
        "SELECT {} FROM agent_connections WHERE id = $1 FOR UPDATE SKIP LOCKED",
        connections::COLUMNS
    ))
    .bind(connection_id)
    .fetch_optional(&mut *transaction)
    .await?
    else {
        return Ok(None);
    };
    if delivery.state != "active" || delivery.webhook_url.is_none() {
        return Ok(None);
    }
    // Pushed for the account itself, unless a person's app set it up.
    let limits = if delivery.made_through_app {
        match app_limit(&mut transaction, &delivery).await? {
            Some(limits) => Some(limits),
            None => {
                transaction.commit().await?;
                return Ok(None);
            }
        }
    } else {
        None
    };
    let scope = Scope {
        recipient: delivery.account_id,
        workspaces: connections::reach(&mut transaction, &delivery).await?,
        space: None,
        limits,
    };
    let subscriptions = connections::subscriptions(&mut transaction, &delivery).await?;
    let batch = read::read_batch(
        &mut transaction,
        &subscriptions,
        &scope,
        delivery.push_cursor,
        QUEUE_BATCH,
    )
    .await?;
    for item in &batch.delivered {
        sqlx::query(
            r#"
            INSERT INTO deliveries (
                connection_id, event_position, event_id, workspace_id, event_type, reason
            ) VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (connection_id, event_position) DO NOTHING
            "#,
        )
        .bind(delivery.id)
        .bind(item.event.position)
        .bind(item.event.id)
        .bind(item.event.workspace_id)
        .bind(&item.event.event_type)
        .bind(item.reason.key())
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        "UPDATE agent_connections SET push_cursor = GREATEST(push_cursor, $2) WHERE id = $1",
    )
    .bind(delivery.id)
    // A2.10: while the account is paused, the cursor waits before what is
    // held; what passed is queued again on the next sweep only once (a
    // delivery is kept once per position).
    .bind(batch.held.unwrap_or(batch.next))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(Queued {
        has_more: batch.has_more && batch.held.is_none(),
    }))
}

/// One post to make.
struct Post {
    position: i64,
    attempts: i32,
    message_id: String,
    event_type: String,
    body: String,
    /// The Standard Webhooks headers, or only the older ones.
    standard: bool,
}

struct Claimed {
    connection: Connection,
    url: String,
    /// Active first, then any retiring one.
    secrets: Vec<String>,
    posts: Vec<Post>,
}

#[derive(FromRow)]
struct DueRow {
    #[sqlx(flatten)]
    event: LoggedEvent,
    attempts: i32,
    reason: String,
    deliverable: bool,
    session_ref: Option<String>,
    session_harness: Option<String>,
}

/// Claims up to four due deliveries: what is no longer visible or true is
/// skipped, the rest are pushed a minute ahead so no other job posts them.
async fn claim(state: &AppState, connection_id: Uuid) -> Result<Claimed, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let delivery = connections::load_any(&mut transaction, connection_id)
        .await?
        .ok_or(AppError::NotFound("connection"))?;
    let columns = crate::events::LOGGED_EVENT_COLUMNS
        .split(", ")
        .map(|column| format!("e.{}", column.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let due = sqlx::query_as::<_, DueRow>(&format!(
        r#"
        SELECT {columns}, d.attempts, d.reason,
               (delivery_visible($2, e) AND delivery_still_relevant($2, e)) AS deliverable,
               session.external_ref AS session_ref, session.harness AS session_harness
        FROM deliveries d
        JOIN events e ON e.position = d.event_position
        LEFT JOIN reply_requests reply
          ON e.type LIKE 'reply.%' AND reply.id = e.thing_id
        LEFT JOIN agent_sessions session
          ON session.agent_actor_id = $2
         AND session.conversation_id = COALESCE(reply.thread_id, reply.conversation_id)
         AND session.state IN ('active', 'idle')
        WHERE d.connection_id = $1 AND d.state = 'pending' AND d.next_attempt_at <= now()
        ORDER BY d.event_position
        LIMIT $3
        FOR UPDATE OF d SKIP LOCKED
        "#
    ))
    .bind(delivery.id)
    .bind(delivery.account_id)
    .bind(IN_FLIGHT)
    .fetch_all(&mut *transaction)
    .await?;
    // A person's app set it up: what is sent again (a retry, a redelivery)
    // keeps to that app's limit as it is now.
    let limits = if delivery.made_through_app && !due.is_empty() {
        Some(app_limit(&mut transaction, &delivery).await?)
    } else {
        None
    };
    let mut posts = Vec::new();
    let mut items = Vec::new();
    for mut row in due {
        if let Some(limits) = &limits {
            row.deliverable = row.deliverable
                && limits
                    .as_ref()
                    .is_some_and(|limits| read::within_limits(limits, &row.event));
        }
        let body = if row.deliverable {
            legacy_body(&delivery, &row)
        } else {
            None
        };
        let skip = !row.deliverable || (delivery.webhook_format != "standard" && body.is_none());
        // A2.14: over a hard budget on deliveries, a push waits for the
        // next period instead of failing (counted once, so a retry after a
        // failed post doesn't count again).
        if !skip
            && let Some(until) = crate::budgets::admit_delivery(
                &mut transaction,
                row.event.workspace_id,
                delivery.account_id,
                &format!("delivery:{}:{}", delivery.id, row.event.position),
            )
            .await?
        {
            sqlx::query(
                "UPDATE deliveries SET next_attempt_at = $3 \
                 WHERE connection_id = $1 AND event_position = $2",
            )
            .bind(delivery.id)
            .bind(row.event.position)
            .bind(until)
            .execute(&mut *transaction)
            .await?;
            continue;
        }
        let next = if skip { None } else { Some(CLAIM_SECONDS) };
        sqlx::query(
            r#"
            UPDATE deliveries
            SET state = CASE WHEN $3::float8 IS NULL THEN 'skipped' ELSE state END,
                next_attempt_at = now() + make_interval(secs => COALESCE($3, 0))
            WHERE connection_id = $1 AND event_position = $2
            "#,
        )
        .bind(delivery.id)
        .bind(row.event.position)
        .bind(next)
        .execute(&mut *transaction)
        .await?;
        if skip {
            continue;
        }
        let reason = Reason::parse(&row.reason).unwrap_or(Reason::Visible);
        match body {
            Some(body) => posts.push(Post {
                position: row.event.position,
                attempts: row.attempts,
                message_id: row.event.id.to_string(),
                event_type: body["event"].as_str().unwrap_or_default().to_owned(),
                body: body.to_string(),
                standard: false,
            }),
            None => items.push((
                row.attempts,
                Delivered {
                    event: row.event,
                    reason,
                },
            )),
        }
    }
    if !items.is_empty() {
        let delivered = items
            .iter()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>();
        let lookups = Lookups::for_batch(&mut transaction, &delivered).await?;
        for (attempts, item) in items {
            posts.push(Post {
                position: item.event.position,
                attempts,
                message_id: envelope::event_id(item.event.id),
                event_type: item.event.event_type.clone(),
                body: envelope::envelope(&item, &delivery, &lookups, None).to_string(),
                standard: true,
            });
        }
        posts.sort_by_key(|post| post.position);
    }
    let secrets = if posts.is_empty() {
        Vec::new()
    } else {
        signing_secrets(&mut transaction, state, delivery.id).await?
    };
    transaction.commit().await?;
    Ok(Claimed {
        url: delivery.webhook_url.clone().unwrap_or_default(),
        connection: delivery,
        secrets,
        posts,
    })
}

/// The older reply wakeup bodies, for `typed_v1` and `inbox_v1`
/// connections; `None` for a standard one, and for what those formats
/// never carried (skipped).
fn legacy_body(delivery: &Connection, row: &DueRow) -> Option<Value> {
    let event = &row.event;
    let payload = &event.payload;
    match delivery.webhook_format.as_str() {
        "inbox_v1"
            if matches!(
                event.event_type.as_str(),
                "reply.requested" | "reply.followup"
            ) =>
        {
            Some(json!({
                "event": "inbox.available",
                "deliveryId": event.id,
                "agentId": delivery.account_id,
                "replyRequestId": event.thing_id,
            }))
        }
        "typed_v1" if event.event_type.starts_with("reply.") => Some(json!({
            "event": event.event_type,
            "deliveryId": event.id,
            "agentId": delivery.account_id,
            "projectId": event.space_id,
            "replyRequestId": event.thing_id,
            "conversationId": payload.get("conversationId"),
            "threadId": payload.get("threadId"),
            "messageId": payload.get("messageId"),
            "generation": payload.get("generation"),
            "followsRequestId": payload.get("followsRequestId"),
            // The agent's own session in the conversation, to resume.
            "sessionRef": row.session_ref,
            "sessionHarness": row.session_harness,
        })),
        _ => None,
    }
}

/// The connection's live signing secrets, the active one first.
pub async fn signing_secrets(
    connection: &mut PgConnection,
    state: &AppState,
    connection_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT secret_id FROM connection_signing_secrets WHERE connection_id = $1 \
         ORDER BY state = 'active' DESC, created_at DESC",
    )
    .bind(connection_id)
    .fetch_all(&mut *connection)
    .await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let Ok(keys) = crate::secrets::keyring(&state.config) else {
        return Ok(Vec::new());
    };
    let mut secrets = Vec::new();
    for id in ids {
        if let Ok(value) =
            crate::secrets::reveal(&mut *connection, keys.as_ref(), id, "agent_webhook.sign").await
        {
            secrets.push(value.expose().to_owned());
        }
    }
    Ok(secrets)
}

/// What one post came to.
struct Sent {
    status: Option<u16>,
    error: Option<&'static str>,
    elapsed_ms: i32,
}

async fn send(post: &Post, url: &str, secrets: &[String], trusted: &[String]) -> Sent {
    let started = std::time::Instant::now();
    let outcome = async {
        let Some(active) = secrets.first() else {
            return Err((None, "the webhook's signing secret couldn't be read"));
        };
        let (url, client) = crate::outbound::webhook_client(url, trusted)
            .await
            .map_err(|error| (None, error))?;
        let timestamp = sign::unix_timestamp();
        let legacy = sign::legacy(active, &timestamp, &post.body)
            .ok_or((None, "could not sign the webhook"))?;
        let mut request = client
            .post(url)
            .header("content-type", "application/json")
            .header("x-workspace-delivery", &post.message_id)
            .header("x-workspace-event", &post.event_type)
            .header("x-workspace-timestamp", &timestamp)
            .header("x-workspace-signature", format!("sha256={legacy}"));
        if post.standard {
            let signatures = secrets
                .iter()
                .filter_map(|secret| {
                    sign::standard(secret, &post.message_id, &timestamp, &post.body)
                })
                .collect::<Vec<_>>()
                .join(" ");
            request = request
                .header("webhook-id", &post.message_id)
                .header("webhook-timestamp", &timestamp)
                .header("webhook-signature", signatures);
        }
        let response = request
            .body(post.body.clone())
            .send()
            .await
            .map_err(|_| (None, "the webhook did not respond"))?;
        let status = response.status();
        if status.is_success() {
            Ok(status.as_u16())
        } else {
            Err((
                Some(status.as_u16()),
                "the webhook returned an error status",
            ))
        }
    }
    .await;
    let elapsed_ms = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
    match outcome {
        Ok(status) => Sent {
            status: Some(status),
            error: None,
            elapsed_ms,
        },
        Err((status, error)) => Sent {
            status,
            error: Some(error),
            elapsed_ms,
        },
    }
}

/// What a receiver answered to a test ping.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestPing {
    pub ok: bool,
    /// The HTTP status the receiver answered with, if it answered.
    pub status: Option<u16>,
    /// Why it didn't work, in words a person can act on.
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub delivery_id: Uuid,
}

/// Sends a signed `ping` to a connection's webhook now, signed and sent
/// exactly like a real delivery, and reports what it answered. It names
/// nothing to act on; nothing is queued or retried.
pub async fn send_test(
    connection: &Connection,
    url: &str,
    secrets: &[String],
    trusted: &[String],
) -> TestPing {
    let delivery_id = Uuid::now_v7();
    let message_id = super::envelope::event_id(delivery_id);
    let body = json!({
        "type": "ping",
        "event": "ping",
        "id": message_id,
        "deliveryId": delivery_id,
        "connectionId": connection.id,
        "agentId": connection.account_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "message": "A test from the workspace. Verify the signature and answer 2xx; there is nothing to do.",
    })
    .to_string();
    let post = Post {
        position: 0,
        attempts: 0,
        message_id: delivery_id.to_string(),
        event_type: "ping".to_owned(),
        body,
        standard: connection.webhook_format == "standard",
    };
    let sent = send(&post, url, secrets, trusted).await;
    let elapsed_ms = u64::try_from(sent.elapsed_ms).unwrap_or_default();
    let error = sent.error.map(|error| match sent.status {
        Some(401 | 403) => {
            "the receiver refused the signature: check it uses this connection's current secret"
                .to_owned()
        }
        Some(status) => format!("the receiver answered {status}"),
        None => error.to_owned(),
    });
    TestPing {
        ok: error.is_none(),
        status: sent.status,
        error,
        elapsed_ms,
        delivery_id,
    }
}

/// Records each post's result, the connection's health, and turns pushes
/// off when a receiver has failed for long enough.
async fn record(state: &AppState, claimed: &Claimed, results: Vec<Sent>) -> Result<(), AppError> {
    if claimed.posts.is_empty() {
        return Ok(());
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    for (post, sent) in claimed.posts.iter().zip(results) {
        let attempts = post.attempts + 1;
        let retry_after = usize::try_from(attempts - 1)
            .ok()
            .and_then(|index| RETRY_AFTER.get(index))
            .copied()
            .unwrap_or(36_000);
        sqlx::query(
            r#"
            UPDATE deliveries
            SET attempts = $3,
                state = CASE WHEN $5::text IS NULL THEN 'delivered'
                             WHEN $3 >= $7 THEN 'failed' ELSE 'pending' END,
                delivered_at = CASE WHEN $5::text IS NULL THEN now() END,
                next_attempt_at = now() + make_interval(secs => $6),
                last_status = $4, last_error = $5, elapsed_ms = $8
            WHERE connection_id = $1 AND event_position = $2
            "#,
        )
        .bind(claimed.connection.id)
        .bind(post.position)
        .bind(attempts)
        .bind(sent.status.map(i32::from))
        .bind(sent.error)
        .bind(retry_after as f64)
        .bind(MAX_ATTEMPTS)
        .bind(sent.elapsed_ms)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE agent_connections
            SET consecutive_failures = CASE WHEN $2::text IS NULL THEN 0
                                            ELSE consecutive_failures + 1 END,
                first_failure_at = CASE WHEN $2::text IS NULL THEN NULL
                                        ELSE COALESCE(first_failure_at, now()) END,
                last_delivered_at = CASE WHEN $2::text IS NULL THEN now()
                                         ELSE last_delivered_at END,
                last_error = $2
            WHERE id = $1
            "#,
        )
        .bind(claimed.connection.id)
        .bind(sent.error)
        .execute(&mut *transaction)
        .await?;
    }
    let disabled = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE agent_connections
        SET state = 'push_disabled', version = version + 1, updated_at = now(),
            disabled_reason = 'No webhook post succeeded for 48 hours. Fix the receiver, then turn pushes back on.'
        WHERE id = $1 AND state = 'active'
          AND consecutive_failures >= $2
          AND first_failure_at <= now() - make_interval(secs => $3)
        RETURNING id
        "#,
    )
    .bind(claimed.connection.id)
    .bind(DISABLE_AFTER_FAILURES)
    .bind(DISABLE_AFTER_SECONDS)
    .fetch_optional(&mut *transaction)
    .await?;
    if disabled.is_some() {
        pushes_stopped(&mut transaction, &claimed.connection).await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Tells the account, on its other transports, that a connection's pushes
/// stopped.
async fn pushes_stopped(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    connection: &Connection,
) -> Result<(), AppError> {
    tell_account(
        transaction,
        connection,
        &crate::events::catalog::CONNECTION_PUSH_DISABLED,
        None,
    )
    .await
}

/// An event about the connection, addressed to its account.
async fn tell_account(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    connection: &Connection,
    ty: &'static crate::events::catalog::EventType,
    thing: Option<(Uuid, &'static str)>,
) -> Result<(), AppError> {
    crate::events::emit(
        transaction,
        crate::events::NewEvent {
            id: Uuid::now_v7(),
            ty,
            workspace_id: Some(connection.home_workspace_id),
            space_id: None,
            thing,
            access_thing: None,
            version: None,
            changes: &[],
            audience: crate::events::Audience::Account(connection.account_id),
            payload: json!({ "connectionId": connection.id }),
            actor: None,
            legacy_type: None,
        },
    )
    .await?;
    Ok(())
}

/// For a connection a person's app set up: the app's delegation limit as it
/// is now, or `None` when the app was disconnected (its pushes stop then).
async fn app_limit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    connection: &Connection,
) -> Result<Option<crate::policy::AgentLimits>, AppError> {
    let connected = match connection.via_grant_id {
        Some(grant) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM mcp_oauth_grants WHERE id = $1 AND revoked_at IS NULL)",
        )
        .bind(grant)
        .fetch_one(&mut **transaction)
        .await?,
        None => true,
    };
    if !connected {
        let stopped = sqlx::query_scalar::<_, Uuid>(
            "UPDATE agent_connections SET state = 'push_disabled', version = version + 1, \
             updated_at = now(), disabled_reason = 'The app that set it up was disconnected.' \
             WHERE id = $1 AND state = 'active' RETURNING id",
        )
        .bind(connection.id)
        .fetch_optional(&mut **transaction)
        .await?;
        if stopped.is_some() {
            pushes_stopped(transaction, connection).await?;
        }
        return Ok(None);
    }
    Ok(Some(
        crate::roles::app_access(transaction, connection.account_id)
            .await?
            .limits,
    ))
}

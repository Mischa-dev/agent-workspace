//! W21: scheduled send. Write a message now, have it sent later as you.
//!
//! Scheduling checks the author can post in the conversation and that the
//! content is valid there. Sending is a job on the queue (at least once,
//! keyed by the message and its time) that runs the ordinary send command
//! as the author, so their access, the conversation's state, mentions and
//! reply rules are all checked as if they pressed Send then (principle 9).
//! An agent's scheduled message sends with a current credential of the
//! connection it was scheduled through, which must still allow it. Each
//! attempt reuses one idempotency key, so a retried job can't post twice.
//! If the send is refused, the message is kept as failed, with why, and the
//! author is told.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        ActorView, MessageCommandActor, MessageMcpCredential, append_message_command,
        fetch_actor_views, lock_message_conversation, message_content_error,
        require_active_conversation, resolve_message_content,
    },
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::IdempotencyKey,
    jobs::{self, JobError, JobRegistry, NewJob},
    later::{message_href, timestamp},
    message_content::MessageContent,
    notifications::{self, NewNotification, kind},
    policy::Action,
};

pub const SEND_KIND: &str = "messages.scheduled_send";
const MAX_AHEAD_DAYS: i64 = 366;
/// Waiting to send, per author.
const MAX_SCHEDULED: i64 = 100;
const LIST_LIMIT: i64 = 200;
/// Failed ones stay listed this long, to copy or reschedule.
const FAILED_SHOWN_DAYS: i32 = 14;

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        SEND_KIND,
        std::time::Duration::from_secs(120),
        |context| async move {
            let job: SendJob = context.job.payload_as()?;
            let last_attempt = context.job.attempts >= context.job.max_attempts;
            send_due(&context.state, &job, last_attempt).await
        },
    );
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledMessageView {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub place: String,
    pub project_id: Uuid,
    pub project_name: String,
    /// The conversation, in the web app.
    pub href: String,
    pub content: serde_json::Value,
    pub plain_text: String,
    pub mentioned_actors: serde_json::Value,
    pub referenced_work_items: serde_json::Value,
    pub send_at: String,
    /// scheduled, sent, failed or canceled
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_href: Option<String>,
    pub created_at: String,
}

#[derive(FromRow)]
struct ScheduledRow {
    id: Uuid,
    conversation_id: Uuid,
    conversation_kind: String,
    conversation_name: String,
    parent_conversation_id: Option<Uuid>,
    project_id: Uuid,
    project_name: String,
    direct_space: bool,
    content: serde_json::Value,
    plain_text: String,
    mentioned_actors: serde_json::Value,
    referenced_work_items: serde_json::Value,
    send_at: DateTime<Utc>,
    status: String,
    failure: Option<String>,
    message_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

/// `$1`'s scheduled messages in conversations they can still see.
fn select_sql(extra: &str) -> String {
    format!(
        r#"
        SELECT scheduled.id, scheduled.conversation_id,
               conversation.kind AS conversation_kind, conversation.name AS conversation_name,
               conversation.parent_channel_id AS parent_conversation_id,
               scheduled.project_id,
               CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END
                   AS project_name,
               project.kind = 'direct' AS direct_space,
               scheduled.content, scheduled.plain_text,
               scheduled.mentioned_actors, scheduled.referenced_work_items,
               scheduled.send_at, scheduled.status, scheduled.failure, scheduled.message_id,
               scheduled.created_at
        FROM scheduled_messages scheduled
        JOIN conversations conversation ON conversation.id = scheduled.conversation_id
        JOIN projects project ON project.id = scheduled.project_id
        WHERE scheduled.author_actor_id = $1
          AND actor_can_access_project($1, scheduled.external_agent_connection_id, scheduled.project_id)
          AND conversation_visible_to(scheduled.conversation_id, $1)
          {extra}
        "#
    )
}

fn place(kind: &str, name: &str) -> String {
    match kind {
        "dm" => "a DM".to_owned(),
        "group" if !name.is_empty() => name.to_owned(),
        "group" => "a group".to_owned(),
        "channel" => format!("#{name}"),
        _ if !name.is_empty() => name.to_owned(),
        _ => "a conversation".to_owned(),
    }
}

fn conversation_href(row: &ScheduledRow) -> String {
    if row.direct_space {
        let id = row.parent_conversation_id.unwrap_or(row.conversation_id);
        return format!("/messages/{id}");
    }
    let segment = if row.conversation_kind == "thread" {
        "threads"
    } else {
        "channels"
    };
    format!(
        "/projects/{}/{segment}/{}",
        row.project_id, row.conversation_id
    )
}

fn view(row: ScheduledRow) -> ScheduledMessageView {
    ScheduledMessageView {
        id: row.id,
        conversation_id: row.conversation_id,
        place: place(&row.conversation_kind, &row.conversation_name),
        project_id: row.project_id,
        project_name: row.project_name.clone(),
        href: conversation_href(&row),
        message_href: row.message_id.map(|message_id| {
            message_href(
                row.project_id,
                row.direct_space,
                row.conversation_id,
                Some(&row.conversation_kind),
                row.parent_conversation_id,
                message_id,
            )
        }),
        content: row.content,
        plain_text: row.plain_text,
        mentioned_actors: row.mentioned_actors,
        referenced_work_items: row.referenced_work_items,
        send_at: timestamp(row.send_at),
        status: row.status,
        failure: row.failure,
        created_at: timestamp(row.created_at),
    }
}

// ---------------------------------------------------------------------------
// Reading

/// Waiting to send, soonest first, then recent failures. Only in
/// `conversation_id` when given, and within `within` when given.
pub async fn list(
    pool: &PgPool,
    viewer: Uuid,
    conversation_id: Option<Uuid>,
    within: Option<&[Uuid]>,
) -> Result<Vec<ScheduledMessageView>, AppError> {
    let sql = select_sql(&format!(
        r#"
        AND ($2::uuid IS NULL OR scheduled.conversation_id = $2)
        AND ($3::uuid[] IS NULL OR scheduled.project_id = ANY($3))
        AND (scheduled.status = 'scheduled'
             OR (scheduled.status = 'failed'
                 AND scheduled.updated_at > now() - interval '{FAILED_SHOWN_DAYS} days'))
        ORDER BY scheduled.status DESC, scheduled.send_at ASC, scheduled.id
        LIMIT $4
        "#
    ));
    Ok(sqlx::query_as::<_, ScheduledRow>(&sql)
        .bind(viewer)
        .bind(conversation_id)
        .bind(within)
        .bind(LIST_LIMIT)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(view)
        .collect())
}

pub async fn get_scheduled(
    pool: &PgPool,
    viewer: Uuid,
    id: Uuid,
) -> Result<ScheduledMessageView, AppError> {
    sqlx::query_as::<_, ScheduledRow>(&select_sql("AND scheduled.id = $2"))
        .bind(viewer)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(view)
        .ok_or(AppError::NotFound("scheduled_message"))
}

// ---------------------------------------------------------------------------
// Scheduling

/// A send time: a little ahead at least, at most a year ahead.
pub fn parse_send_at(value: &str) -> Result<DateTime<Utc>, AppError> {
    let at = DateTime::parse_from_rfc3339(value)
        .map_err(|_| AppError::Validation("sendAt"))?
        .with_timezone(&Utc);
    let now = Utc::now();
    if at < now - Duration::minutes(1) || at > now + Duration::days(MAX_AHEAD_DAYS) {
        return Err(AppError::Validation("sendAt"));
    }
    Ok(at.max(now))
}

struct Checked {
    organization_id: Uuid,
    project_id: Uuid,
    content: serde_json::Value,
    plain_text: String,
    mentioned_actors: Vec<ActorView>,
    referenced_work_items: serde_json::Value,
}

/// The author can post here now, and the content is valid here.
async fn check(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MessageCommandActor,
    conversation_id: Uuid,
    content: serde_json::Value,
) -> Result<Checked, AppError> {
    let content = MessageContent::from_json(content).map_err(message_content_error)?;
    let conversation =
        lock_message_conversation(transaction, actor, conversation_id, Action::MessageSend).await?;
    require_active_conversation(&conversation)?;
    let resolved = resolve_message_content(transaction, conversation.project_id, content).await?;
    let mentioned_actors = fetch_actor_views(
        transaction,
        conversation.project_id,
        &resolved.mentioned_actor_ids,
    )
    .await?;
    let referenced_work_items = serde_json::Value::Array(
        resolved
            .work_item_rows
            .iter()
            .map(|(id, key, title, status_category, archived_at)| {
                serde_json::json!({
                    "id": id,
                    "key": key,
                    "title": title,
                    "statusCategory": status_category,
                    "archivedAt": archived_at,
                    "href": format!("/projects/{}/work-items/{id}", conversation.project_id),
                })
            })
            .collect(),
    );
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(conversation.project_id)
            .fetch_one(&mut **transaction)
            .await?;
    Ok(Checked {
        organization_id,
        project_id: conversation.project_id,
        content: resolved.content_json,
        plain_text: resolved.plain_text,
        mentioned_actors,
        referenced_work_items,
    })
}

/// Schedule `content` to be sent into `conversation_id` at `send_at`, as
/// `actor`.
pub(crate) async fn schedule(
    state: &AppState,
    actor: &MessageCommandActor,
    conversation_id: Uuid,
    content: serde_json::Value,
    send_at: &str,
) -> Result<ScheduledMessageView, AppError> {
    let send_at = parse_send_at(send_at)?;
    let mut transaction = state.pool.begin().await?;
    let waiting = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM scheduled_messages WHERE author_actor_id = $1 AND status = 'scheduled'",
    )
    .bind(actor.actor_id)
    .fetch_one(&mut *transaction)
    .await?;
    if waiting >= MAX_SCHEDULED {
        return Err(AppError::Conflict(
            "scheduled_message",
            "you have too many messages waiting to send",
        ));
    }
    let checked = check(&mut transaction, actor, conversation_id, content).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO scheduled_messages (
            id, organization_id, project_id, conversation_id, author_actor_id,
            external_agent_connection_id, via_oauth_grant_id, content, plain_text,
            mentioned_actors, referenced_work_items, send_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(id)
    .bind(checked.organization_id)
    .bind(checked.project_id)
    .bind(conversation_id)
    .bind(actor.actor_id)
    .bind(actor.mcp_credential.map(|proof| proof.connection_id))
    .bind(actor.via_grant_id)
    .bind(&checked.content)
    .bind(&checked.plain_text)
    .bind(
        serde_json::to_value(&checked.mentioned_actors)
            .map_err(|_| AppError::Internal("mentions"))?,
    )
    .bind(&checked.referenced_work_items)
    .bind(send_at)
    .execute(&mut *transaction)
    .await?;
    enqueue(&mut *transaction, checked.organization_id, id, send_at).await?;
    transaction.commit().await?;
    get_scheduled(&state.pool, actor.actor_id, id).await
}

#[derive(FromRow)]
struct LockedScheduled {
    organization_id: Uuid,
    conversation_id: Uuid,
    send_at: DateTime<Utc>,
    status: String,
}

async fn lock_own(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    id: Uuid,
) -> Result<LockedScheduled, AppError> {
    sqlx::query_as::<_, LockedScheduled>(
        r#"
        SELECT organization_id, conversation_id, send_at, status
        FROM scheduled_messages
        WHERE id = $1 AND author_actor_id = $2
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("scheduled_message"))
}

/// Change the text or the time of one still waiting to send, or of a failed
/// one (which schedules it again).
pub(crate) async fn update(
    state: &AppState,
    actor: &MessageCommandActor,
    id: Uuid,
    content: Option<serde_json::Value>,
    send_at: Option<&str>,
) -> Result<ScheduledMessageView, AppError> {
    get_scheduled(&state.pool, actor.actor_id, id).await?;
    let send_at = send_at.map(parse_send_at).transpose()?;
    let mut transaction = state.pool.begin().await?;
    let locked = lock_own(&mut transaction, actor.actor_id, id).await?;
    if !matches!(locked.status.as_str(), "scheduled" | "failed") {
        return Err(AppError::Conflict(
            "scheduled_message",
            "this message was already sent or canceled",
        ));
    }
    let content = match content {
        Some(content) => content,
        None => {
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT content FROM scheduled_messages WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut *transaction)
            .await?
        }
    };
    // Checked again, as if it were new.
    let checked = check(&mut transaction, actor, locked.conversation_id, content).await?;
    // A failed one is scheduled again only with a new time; fixing just
    // its text keeps it failed rather than sending it at once.
    if locked.status == "failed" && send_at.is_none() {
        sqlx::query(
            r#"
            UPDATE scheduled_messages
            SET content = $2, plain_text = $3, mentioned_actors = $4, referenced_work_items = $5,
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&checked.content)
        .bind(&checked.plain_text)
        .bind(
            serde_json::to_value(&checked.mentioned_actors)
                .map_err(|_| AppError::Internal("mentions"))?,
        )
        .bind(&checked.referenced_work_items)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return get_scheduled(&state.pool, actor.actor_id, id).await;
    }
    let next = send_at.unwrap_or(locked.send_at).max(Utc::now());
    sqlx::query(
        r#"
        UPDATE scheduled_messages
        SET content = $2, plain_text = $3, mentioned_actors = $4, referenced_work_items = $5,
            send_at = $6, status = 'scheduled', failure = NULL, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(&checked.content)
    .bind(&checked.plain_text)
    .bind(
        serde_json::to_value(&checked.mentioned_actors)
            .map_err(|_| AppError::Internal("mentions"))?,
    )
    .bind(&checked.referenced_work_items)
    .bind(next)
    .execute(&mut *transaction)
    .await?;
    if next != locked.send_at || locked.status != "scheduled" {
        dequeue(&mut *transaction, id, locked.send_at).await?;
        enqueue(&mut *transaction, locked.organization_id, id, next).await?;
    }
    transaction.commit().await?;
    get_scheduled(&state.pool, actor.actor_id, id).await
}

/// Send one waiting message now.
pub async fn send_now(
    state: &AppState,
    actor_id: Uuid,
    id: Uuid,
) -> Result<ScheduledMessageView, AppError> {
    get_scheduled(&state.pool, actor_id, id).await?;
    let mut transaction = state.pool.begin().await?;
    let locked = lock_own(&mut transaction, actor_id, id).await?;
    if locked.status != "scheduled" {
        return Err(AppError::Conflict(
            "scheduled_message",
            "this message isn't waiting to send",
        ));
    }
    let now = Utc::now();
    sqlx::query("UPDATE scheduled_messages SET send_at = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    dequeue(&mut *transaction, id, locked.send_at).await?;
    enqueue(&mut *transaction, locked.organization_id, id, now).await?;
    transaction.commit().await?;
    // Sent straight away rather than when a worker next looks.
    let _ = send_due(
        state,
        &SendJob {
            scheduled_message_id: id,
            at_micros: now.timestamp_micros(),
        },
        false,
    )
    .await;
    get_scheduled(&state.pool, actor_id, id).await
}

/// Cancel one waiting (or failed) message. It isn't sent.
pub async fn cancel(pool: &PgPool, actor_id: Uuid, id: Uuid) -> Result<(), AppError> {
    let mut transaction = pool.begin().await?;
    let locked = lock_own(&mut transaction, actor_id, id).await?;
    if !matches!(locked.status.as_str(), "scheduled" | "failed") {
        return Err(AppError::Conflict(
            "scheduled_message",
            "this message was already sent",
        ));
    }
    sqlx::query(
        "UPDATE scheduled_messages SET status = 'canceled', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    dequeue(&mut *transaction, id, locked.send_at).await?;
    transaction.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sending

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendJob {
    scheduled_message_id: Uuid,
    at_micros: i64,
}

fn job_key(id: Uuid, at: DateTime<Utc>) -> String {
    format!("{SEND_KIND}:{id}:{}", at.timestamp_micros())
}

async fn enqueue<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    id: Uuid,
    at: DateTime<Utc>,
) -> Result<(), AppError> {
    jobs::enqueue(
        executor,
        NewJob::new(SEND_KIND)
            .workspace(organization_id)
            .payload(SendJob {
                scheduled_message_id: id,
                at_micros: at.timestamp_micros(),
            })
            .unique(job_key(id, at))
            .run_at(at)
            .priority(10)
            .max_attempts(6),
    )
    .await?;
    Ok(())
}

async fn dequeue<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    id: Uuid,
    at: DateTime<Utc>,
) -> Result<(), AppError> {
    jobs::cancel_queued(executor, &job_key(id, at)).await?;
    Ok(())
}

#[derive(FromRow)]
struct DueRow {
    id: Uuid,
    organization_id: Uuid,
    conversation_id: Uuid,
    author_actor_id: Uuid,
    author_kind: String,
    external_agent_connection_id: Option<Uuid>,
    via_oauth_grant_id: Option<Uuid>,
    content: serde_json::Value,
    send_at: DateTime<Utc>,
    status: String,
}

/// Why a refused send failed, in words for its author; `None` for errors
/// worth retrying.
fn failure_reason(error: &AppError) -> Option<&'static str> {
    match error {
        AppError::NotFound(_) | AppError::Forbidden => {
            Some("You can no longer post in that conversation.")
        }
        AppError::Conflict(..) | AppError::ProjectArchived => {
            Some("That conversation is archived.")
        }
        AppError::Validation(_) | AppError::Invalid(_) => {
            Some("Someone it mentions or a task it links is no longer there.")
        }
        _ => None,
    }
}

async fn send_due(state: &AppState, job: &SendJob, last_attempt: bool) -> Result<(), JobError> {
    let mut transaction = state.pool.begin().await?;
    let Some(row) = sqlx::query_as::<_, DueRow>(
        r#"
        SELECT scheduled.id, scheduled.organization_id, scheduled.conversation_id,
               scheduled.author_actor_id, actor.kind AS author_kind,
               scheduled.external_agent_connection_id, scheduled.via_oauth_grant_id,
               scheduled.content, scheduled.send_at, scheduled.status
        FROM scheduled_messages scheduled
        JOIN actors actor ON actor.id = scheduled.author_actor_id
        WHERE scheduled.id = $1
        FOR UPDATE OF scheduled
        "#,
    )
    .bind(job.scheduled_message_id)
    .fetch_optional(&mut *transaction)
    .await?
    else {
        return Ok(());
    };
    // Moved, canceled or already sent: another job (or none) is for it.
    if row.status != "scheduled" || row.send_at.timestamp_micros() != job.at_micros {
        return Ok(());
    }
    let actor = match sender(&mut transaction, &row).await? {
        Ok(actor) => actor,
        Err(reason) => {
            fail(&mut transaction, &row, reason).await?;
            transaction.commit().await?;
            return Ok(());
        }
    };
    let key = IdempotencyKey::parse(&format!("scheduled-message-{}", row.id))?;
    // The ordinary send, as the author now. The row stays locked meanwhile,
    // so a cancel or edit waits for the outcome.
    match append_message_command(
        state,
        actor,
        row.conversation_id,
        None,
        key,
        row.content.clone(),
        None,
    )
    .await
    {
        Ok(outcome) => {
            sqlx::query(
                r#"
                UPDATE scheduled_messages
                SET status = 'sent', message_id = $2, sent_at = now(), updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(row.id)
            .bind(outcome.body.message.id)
            .execute(&mut *transaction)
            .await?;
        }
        Err(error) => match failure_reason(&error) {
            Some(reason) => fail(&mut transaction, &row, reason).await?,
            // Out of retries: say so, rather than leaving it "scheduled"
            // with its time long past.
            None if last_attempt => {
                fail(
                    &mut transaction,
                    &row,
                    "It couldn't be sent. Try sending it again.",
                )
                .await?
            }
            None => return Err(error.into()),
        },
    }
    transaction.commit().await?;
    Ok(())
}

/// Who sends: the author as they are now. An agent needs a current
/// credential of the connection it scheduled through.
async fn sender(
    transaction: &mut Transaction<'_, Postgres>,
    row: &DueRow,
) -> Result<Result<MessageCommandActor, &'static str>, AppError> {
    match row.author_kind.as_str() {
        "human" => {
            let via = match row.via_oauth_grant_id {
                Some(grant_id) => {
                    let live = sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM mcp_oauth_grants WHERE id = $1 AND revoked_at IS NULL",
                    )
                    .bind(grant_id)
                    .fetch_optional(&mut **transaction)
                    .await?;
                    // Disconnecting the app stops what it scheduled, as it
                    // stops an agent's: it doesn't go out as typed by hand.
                    if live.is_none() {
                        return Ok(Err("The app it was scheduled through was disconnected."));
                    }
                    live
                }
                None => None,
            };
            Ok(Ok(MessageCommandActor {
                actor_id: row.author_actor_id,
                actor_kind: "human".to_owned(),
                mcp_credential: None,
                via_grant_id: via,
            }))
        }
        "external_agent" => {
            let Some(connection_id) = row.external_agent_connection_id else {
                return Ok(Err("The agent's connection can't send messages."));
            };
            let credential_id = sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT credential.id FROM mcp_client_credentials credential
                WHERE credential.external_agent_connection_id = $1
                  AND credential.actor_id = $2
                  AND credential.status = 'active'
                  AND credential.revoked_at IS NULL
                  AND credential.expires_at > now()
                  AND credential.scopes @> ARRAY['write']::text[]
                ORDER BY credential.created_at DESC
                LIMIT 1
                "#,
            )
            .bind(connection_id)
            .bind(row.author_actor_id)
            .fetch_optional(&mut **transaction)
            .await?;
            Ok(match credential_id {
                Some(credential_id) => Ok(MessageCommandActor {
                    actor_id: row.author_actor_id,
                    actor_kind: "external_agent".to_owned(),
                    mcp_credential: Some(MessageMcpCredential {
                        credential_id,
                        connection_id,
                    }),
                    via_grant_id: None,
                }),
                None => Err("The agent's connection can no longer send messages."),
            })
        }
        _ => Ok(Err("This account can't send messages.")),
    }
}

async fn fail(
    transaction: &mut Transaction<'_, Postgres>,
    row: &DueRow,
    reason: &'static str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE scheduled_messages SET status = 'failed', failure = $2, updated_at = now() WHERE id = $1",
    )
    .bind(row.id)
    .bind(reason)
    .execute(&mut **transaction)
    .await?;
    if row.author_kind != "human" {
        return Ok(());
    }
    // Told in their workspace's own space: the conversation may be one
    // they can no longer see.
    let Some(space) =
        crate::direct::space_for_member(transaction, row.organization_id, row.author_actor_id)
            .await?
    else {
        return Ok(());
    };
    let notification = NewNotification {
        project_id: space,
        recipient_actor_id: row.author_actor_id,
        kind: kind::SCHEDULED_SEND_FAILED,
        subject_kind: "scheduled_message",
        subject_id: row.id,
        actor_id: None,
        conversation_id: None,
        message_id: None,
        work_item_id: None,
        comment_id: None,
        agent_run_id: None,
        approval_id: None,
        pull_request_id: None,
        detail: serde_json::json!({
            "title": "Your scheduled message wasn't sent",
            "preview": reason,
            "href": "/later?view=scheduled",
        }),
        group_key: format!("scheduled_failed:{}", row.id),
        dedupe_key: format!(
            "scheduled_failed:{}:{}",
            row.id,
            row.send_at.timestamp_micros()
        ),
    };
    if notifications::deliver(transaction, &notification).await? {
        notifications::announce(transaction, space, row.author_actor_id).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/scheduled-messages", get(list_http))
        .route(
            "/api/v1/conversations/{id}/scheduled-messages",
            post(schedule_http),
        )
        .route(
            "/api/v1/scheduled-messages/{id}",
            patch(update_http).delete(cancel_http),
        )
        .route(
            "/api/v1/scheduled-messages/{id}/send-now",
            post(send_now_http),
        )
}

fn browser_actor(principal: &AuthenticatedActor) -> MessageCommandActor {
    MessageCommandActor {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind.clone(),
        mcp_credential: None,
        via_grant_id: None,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    conversation_id: Option<Uuid>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    scheduled_messages: Vec<ScheduledMessageView>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<ListResponse>, AppError> {
    Ok(Json(ListResponse {
        scheduled_messages: list(&state.pool, principal.actor_id, query.conversation_id, None)
            .await?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleRequest {
    content: serde_json::Value,
    send_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemResponse {
    scheduled_message: ScheduledMessageView,
}

async fn schedule_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ScheduleRequest>,
) -> Result<(StatusCode, Json<ItemResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let scheduled_message = schedule(
        &state,
        &browser_actor(&principal),
        conversation_id,
        request.content,
        &request.send_at,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ItemResponse { scheduled_message }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    content: Option<serde_json::Value>,
    send_at: Option<String>,
}

async fn update_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<Json<ItemResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let scheduled_message = update(
        &state,
        &browser_actor(&principal),
        id,
        request.content,
        request.send_at.as_deref(),
    )
    .await?;
    Ok(Json(ItemResponse { scheduled_message }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

async fn send_now_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<ItemResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let scheduled_message = send_now(&state, principal.actor_id, id).await?;
    Ok(Json(ItemResponse { scheduled_message }))
}

async fn cancel_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    cancel(&state.pool, principal.actor_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_fail_the_message_and_outages_retry() {
        assert!(failure_reason(&AppError::NotFound("conversation")).is_some());
        assert!(failure_reason(&AppError::Validation("Message content")).is_some());
        assert!(failure_reason(&AppError::Internal("boom")).is_none());
    }

    #[test]
    fn send_times_must_be_ahead_and_within_a_year() {
        let soon = (Utc::now() + Duration::hours(1)).to_rfc3339();
        assert!(parse_send_at(&soon).is_ok());
        let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
        assert!(parse_send_at(&past).is_err());
        let far = (Utc::now() + Duration::days(400)).to_rfc3339();
        assert!(parse_send_at(&far).is_err());
    }
}

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

use std::{collections::BTreeSet, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{get, post},
};
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
    conversations::{
        ActorView, enqueue_event, insert_audit_event, insert_audit_event_for,
        lock_conversation_for, parse_actor_kind,
    },
    error::{ApiPath, AppError},
    external_events::enqueue_external_event,
    outbox::{OutboxEventPayload, ReplyRequestsUpdatedPayload},
    policy::{Action, ActorKind},
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
        .route("/api/v1/reply-requests/{id}/stop", post(stop_request))
        .route("/agent-runner.md", get(runner_guide))
}

/// The runner contract (docs/agent-runner.md), for the people and agents
/// setting a runner up.
async fn runner_guide(State(state): State<AppState>) -> impl IntoResponse {
    let origin = state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr));
    (
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../../../docs/agent-runner.md").replace("{origin}", &origin),
    )
}

/// A message that was just written, as seen by the reply rules.
pub(crate) struct NewMessage<'a> {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    /// Where the member list lives: the conversation itself, or the DM or
    /// group a thread belongs to.
    pub members_conversation_id: Uuid,
    /// The kind whose rules apply. A thread under a DM or group follows the
    /// DM or group rules.
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
        // Project discussions ask agents a person mentions, and in a thread
        // also the agents already working there (follow-ups). DMs and groups
        // retain their existing delivery and turn rules below.
        if message.author_kind != "human" {
            return Ok(());
        }
        if !message.mentioned_actor_ids.is_empty() {
            ask(transaction, &message, true).await?;
            // Tagging an agent backed by a harness launches a run (A8).
            crate::launch::queue_tag_launches(
                transaction,
                message.project_id,
                message.conversation_id,
                message.message_id,
                message.mentioned_actor_ids,
            )
            .await?;
        }
        if message.conversation_kind == "thread" {
            let followers = thread_followers(transaction, &message).await?;
            insert_requests(transaction, &message, &followers, false, true).await?;
        }
        return Ok(());
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

/// Ask agents a person newly @mentioned by editing their message, in a
/// channel or a thread under one. `message.mentioned_actor_ids` holds only the
/// newly mentioned (in no earlier revision). Nothing is superseded and turns
/// don't change. In DMs and groups (and threads under them) every agent member
/// was already asked when the message was sent, and agents' own edits ask
/// nobody, matching send.
pub(crate) async fn ask_after_edit(
    transaction: &mut Transaction<'_, Postgres>,
    message: NewMessage<'_>,
) -> Result<(), AppError> {
    if !matches!(message.conversation_kind, "channel" | "thread")
        || message.author_kind != "human"
        || message.mentioned_actor_ids.is_empty()
    {
        return Ok(());
    }
    let agents = agents_to_ask(transaction, &message).await?;
    insert_requests(transaction, &message, &agents, true, false).await?;
    Ok(())
}

/// Ask the agents the reply rules pick, superseding their pending requests
/// about earlier messages here.
async fn ask(
    transaction: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
    required: bool,
) -> Result<(), AppError> {
    let agents = agents_to_ask(transaction, message).await?;
    insert_requests(transaction, message, &agents, required, true).await?;
    Ok(())
}

/// The agents the reply rules ask about a message. In a channel or thread
/// under one: the active agents a person mentioned who can see it. In a DM or
/// group (or a thread under one): every other active agent member.
async fn agents_to_ask(
    transaction: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
) -> Result<Vec<Uuid>, AppError> {
    if matches!(message.conversation_kind, "channel" | "thread") {
        return Ok(sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT membership.actor_id
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.project_id = $1
              AND membership.enabled
              AND actor.kind = 'external_agent'
              AND actor.status = 'active'
              AND actor.id = ANY($2)
              AND conversation_visible_to($3, actor.id)
            ORDER BY actor.id
            "#,
        )
        .bind(message.project_id)
        .bind(message.mentioned_actor_ids)
        .bind(message.conversation_id)
        .fetch_all(&mut **transaction)
        .await?);
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
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
    .bind(message.members_conversation_id)
    .bind(message.author_actor_id)
    .fetch_all(&mut **transaction)
    .await?)
}

/// How many of a thread's latest messages count when deciding which agents are
/// still part of it.
const FOLLOW_UP_WINDOW: i64 = 20;

/// The agents a person's message in a project thread follows up with, without
/// a new mention: the active agents who can see the thread and either have an
/// open request there (including a channel tag answered in this thread) or
/// wrote one of its latest messages. Agents the message mentions are asked
/// separately, as a mention.
async fn thread_followers(
    transaction: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.actor_id
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.enabled
          AND actor.kind = 'external_agent'
          AND actor.status = 'active'
          AND actor.id <> $3
          AND NOT (actor.id = ANY($4))
          AND conversation_visible_to($2, actor.id)
          AND (
              EXISTS (
                  SELECT 1 FROM reply_requests request
                  WHERE request.agent_actor_id = actor.id
                    AND (request.conversation_id = $2 OR request.thread_id = $2)
                    AND request.status IN ('pending', 'claimed')
              )
              OR actor.id IN (
                  SELECT recent.author_actor_id
                  FROM (
                      SELECT message.author_actor_id
                      FROM conversation_entries entry
                      JOIN messages message ON message.id = entry.message_id
                      WHERE entry.conversation_id = $2
                        AND message.id <> $5
                        AND message.removed_at IS NULL
                      ORDER BY entry.sequence DESC
                      LIMIT $6
                  ) recent
              )
          )
        ORDER BY actor.id
        "#,
    )
    .bind(message.project_id)
    .bind(message.conversation_id)
    .bind(message.author_actor_id)
    .bind(message.mentioned_actor_ids)
    .bind(message.message_id)
    .bind(FOLLOW_UP_WINDOW)
    .fetch_all(&mut **transaction)
    .await?)
}

/// Ask each of `agents` about `message`, waking the ones newly asked, and
/// return how many were. An agent already asked about this message keeps the
/// request it has, whatever its status.
///
/// With `supersede_others`, the agents' still-pending requests about other
/// messages in this conversation are superseded first, so nobody answers a
/// stale question. The request about this message itself is never
/// superseded: asking about the same message twice (a second Continue, or a
/// retried one) leaves each agent exactly one open request, rather than
/// superseding the first and inserting nothing.
async fn insert_requests(
    transaction: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
    agents: &[Uuid],
    required: bool,
    supersede_others: bool,
) -> Result<usize, AppError> {
    if agents.is_empty() {
        return Ok(0);
    }
    if supersede_others {
        sqlx::query(
            r#"
            UPDATE reply_requests
            SET status = 'superseded', updated_at = now()
            WHERE conversation_id = $1
              AND status = 'pending'
              AND agent_actor_id = ANY($2)
              AND message_id <> $3
            "#,
        )
        .bind(message.conversation_id)
        .bind(agents)
        .bind(message.message_id)
        .execute(&mut **transaction)
        .await?;
    }
    let mut asked = 0;
    for &agent in agents {
        // When the agent is still working on a claim here, the new request
        // queues behind it and says which claim it follows.
        let inserted = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            r#"
            INSERT INTO reply_requests (
                id, project_id, conversation_id, message_id, agent_actor_id,
                required, mentioned, follows_request_id
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7,
                (
                    SELECT claimed.id FROM reply_requests claimed
                    WHERE claimed.agent_actor_id = $5
                      AND claimed.status = 'claimed'
                      AND (claimed.conversation_id = $3 OR claimed.thread_id = $3)
                    ORDER BY claimed.created_at DESC
                    LIMIT 1
                )
            )
            ON CONFLICT (message_id, agent_actor_id) DO NOTHING
            RETURNING id, follows_request_id
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
        if let Some((request_id, follows)) = inserted {
            asked += 1;
            wake(
                transaction,
                message.project_id,
                message.conversation_id,
                agent,
                request_id,
                if follows.is_some() {
                    Wakeup::FollowUp
                } else {
                    Wakeup::Requested
                },
            )
            .await?;
        }
    }
    notify(transaction, message.project_id, message.conversation_id).await?;
    Ok(asked)
}

/// Which open reply requests `withdraw_requests` takes back.
pub(crate) enum Withdraw<'a> {
    /// Every request about one message, for example because it was removed.
    Message {
        conversation_id: Uuid,
        message_id: Uuid,
    },
    /// These agents' requests in a group and the threads under it that
    /// `editor_actor_id` can see, for example because they left the group.
    AgentsInGroup {
        group_id: Uuid,
        agent_actor_ids: &'a [Uuid],
        editor_actor_id: Uuid,
    },
}

/// The only way reply requests are withdrawn. Every open request the filter
/// matches (pending, claimed, expired or released) becomes superseded, with
/// `reason` in plain words, so it can't be answered (`complete_with_reply`
/// needs a claim), handed out, or retried (`retry_request` takes only
/// expired or released ones). Answered requests keep 'replied'. Browsers
/// watching each affected conversation are told once. Returns how many were
/// withdrawn.
///
/// The caller holds the conversation's lock, taken through a helper that
/// checks `conversation_visible_to` (the lock order is the conversation, then
/// these rows).
pub(crate) async fn withdraw_requests(
    transaction: &mut Transaction<'_, Postgres>,
    filter: Withdraw<'_>,
    reason: &str,
) -> Result<i64, AppError> {
    let withdrawn = match filter {
        Withdraw::Message {
            conversation_id,
            message_id,
        } => {
            sqlx::query_as::<_, (Uuid, Uuid)>(
                r#"
                UPDATE reply_requests
                SET status = 'superseded', lease_expires_at = NULL, reason = $3,
                    updated_at = now()
                WHERE conversation_id = $1
                  AND message_id = $2
                  AND status IN ('pending', 'claimed', 'expired', 'released')
                RETURNING project_id, conversation_id
                "#,
            )
            .bind(conversation_id)
            .bind(message_id)
            .bind(reason)
            .fetch_all(&mut **transaction)
            .await?
        }
        Withdraw::AgentsInGroup {
            group_id,
            agent_actor_ids,
            editor_actor_id,
        } => {
            sqlx::query_as::<_, (Uuid, Uuid)>(
                r#"
                UPDATE reply_requests request
                SET status = 'superseded', lease_expires_at = NULL, reason = $4,
                    updated_at = now()
                FROM conversations conversation
                WHERE conversation.id = request.conversation_id
                  AND (conversation.id = $1
                       OR (conversation.parent_channel_id = $1 AND conversation.kind = 'thread'))
                  AND conversation_visible_to(conversation.id, $3)
                  AND request.agent_actor_id = ANY($2)
                  AND request.status IN ('pending', 'claimed', 'expired', 'released')
                RETURNING request.project_id, request.conversation_id
                "#,
            )
            .bind(group_id)
            .bind(agent_actor_ids)
            .bind(editor_actor_id)
            .bind(reason)
            .fetch_all(&mut **transaction)
            .await?
        }
    };
    let count = i64::try_from(withdrawn.len()).unwrap_or(i64::MAX);
    let conversations = withdrawn.into_iter().collect::<BTreeSet<_>>();
    for (project_id, conversation_id) in conversations {
        notify(transaction, project_id, conversation_id).await?;
    }
    Ok(count)
}

/// What a wakeup tells the agent happened to a reply request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wakeup {
    /// Someone asked the agent for a reply.
    Requested,
    /// A person followed up in a thread where the agent is working; the
    /// request names the claim it follows.
    FollowUp,
    /// A person pressed Stop.
    Cancelled,
}

impl Wakeup {
    pub(crate) fn event(self) -> &'static str {
        match self {
            Self::Requested => "reply.requested",
            Self::FollowUp => "reply.followup",
            Self::Cancelled => "reply.cancelled",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Requested => "You were asked to reply",
            Self::FollowUp => "A person followed up where you're working",
            Self::Cancelled => "A person stopped this request",
        }
    }
}

/// Tell each of the agent's active connections what happened: an inbox event
/// for pollers, plus a webhook delivery if the connection has one.
async fn wake(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    agent_actor_id: Uuid,
    request_id: Uuid,
    wakeup: Wakeup,
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
            wakeup.event(),
            wakeup.summary(),
            "reply_request",
            request_id,
            &format!("/messages/{conversation_id}"),
        )
        .await?;
        if has_webhook {
            sqlx::query(
                r#"
                INSERT INTO agent_webhook_deliveries (id, connection_id, reply_request_id, event)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (connection_id, reply_request_id, event) DO UPDATE
                SET attempts = 0, next_attempt_at = now(), delivered_at = NULL,
                    last_error = NULL
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(connection_id)
            .bind(request_id)
            .bind(wakeup.event())
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

/// Push a refresh to browsers watching a request's conversation and, for a
/// channel tag answered in a thread, that thread.
pub(crate) async fn notify_request(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    thread_id: Option<Uuid>,
) -> Result<(), AppError> {
    notify(transaction, project_id, conversation_id).await?;
    if let Some(thread_id) = thread_id.filter(|thread_id| *thread_id != conversation_id) {
        notify(transaction, project_id, thread_id).await?;
    }
    Ok(())
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
    let updated = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>)>(
        r#"
        UPDATE reply_requests
        SET status = 'replied', reply_message_id = $5, lease_expires_at = NULL,
            updated_at = now()
        WHERE id = $1 AND generation = $2 AND agent_actor_id = $3
          AND COALESCE(thread_id, conversation_id) = $4 AND status = 'claimed'
          AND lease_expires_at > clock_timestamp()
        RETURNING project_id, conversation_id, thread_id
        "#,
    )
    .bind(request_id)
    .bind(generation)
    .bind(agent_actor_id)
    .bind(conversation_id)
    .bind(reply_message_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some((project_id, request_conversation_id, thread_id)) = updated {
        notify_request(transaction, project_id, request_conversation_id, thread_id).await
    } else {
        Err(AppError::Conflict(
            "reply request",
            "not claimed by this agent any more",
        ))
    }
}

/// Keep the agent's claimed request open after an interim message, renewing
/// its lease. Fails like `complete_with_reply` when the claim is gone, so the
/// caller's transaction rolls the message back.
pub(crate) async fn renew_claim(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
    generation: i32,
    agent_actor_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let updated = sqlx::query(
        r#"
        UPDATE reply_requests
        SET lease_expires_at = now() + make_interval(mins => $5), updated_at = now()
        WHERE id = $1 AND generation = $2 AND agent_actor_id = $3
          AND COALESCE(thread_id, conversation_id) = $4 AND status = 'claimed'
          AND lease_expires_at > clock_timestamp()
        "#,
    )
    .bind(request_id)
    .bind(generation)
    .bind(agent_actor_id)
    .bind(conversation_id)
    .bind(LEASE_MINUTES)
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
    /// What the agent says it is doing, while it holds the claim.
    pub progress: Option<String>,
    pub progress_at: Option<String>,
    pub updated_at: String,
    /// When the agent last used any of its connections.
    pub agent_last_seen_at: Option<String>,
    /// Where the agent answers a channel tag: the thread started from the
    /// message, once the agent picked it up.
    pub thread_id: Option<Uuid>,
    /// Set on a follow-up queued behind a request the agent is working on.
    pub follows_request_id: Option<Uuid>,
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
    progress_text: Option<String>,
    progress_at: Option<String>,
    updated_at: String,
    agent_last_seen_at: Option<String>,
    thread_id: Option<Uuid>,
    follows_request_id: Option<Uuid>,
}

/// Recent requests for a conversation's timeline, newest first. A thread also
/// shows the channel tags answered in it.
/// The reply requests a conversation's timeline shows. The viewer's
/// visibility is checked in these same queries, not only before them, so a
/// reader removed from a private conversation in between sees nothing.
pub(crate) async fn timeline_requests(
    pool: &PgPool,
    conversation_id: Uuid,
    viewer_actor_id: Uuid,
) -> Result<(Vec<ReplyRequestView>, i32), AppError> {
    let rows = sqlx::query_as::<_, ReplyRequestRow>(
        r#"
        SELECT request.id, request.message_id, request.agent_actor_id,
               membership.display_name AS agent_name,
               membership.handle AS agent_handle,
               request.required, request.mentioned, request.status, request.reason,
               -- A status describes work in progress, so only a live claim shows one.
               CASE WHEN request.status = 'claimed' THEN request.progress_text END
                   AS progress_text,
               CASE WHEN request.status = 'claimed' THEN request.progress_at::text END
                   AS progress_at,
               request.updated_at::text AS updated_at,
               (
                   SELECT max(connection.last_used_at)::text
                   FROM external_agent_connections connection
                   WHERE agent_connection_can_access(connection.id, request.project_id)
                     AND connection.actor_id = request.agent_actor_id
               ) AS agent_last_seen_at,
               request.thread_id,
               request.follows_request_id
        FROM reply_requests request
        JOIN project_memberships membership
          ON membership.project_id = request.project_id
         AND membership.actor_id = request.agent_actor_id
        WHERE (request.conversation_id = $1 OR request.thread_id = $1)
          AND request.status <> 'superseded'
          AND conversation_visible_to($1, $2)
        ORDER BY request.created_at DESC, request.id
        LIMIT 60
        "#,
    )
    .bind(conversation_id)
    .bind(viewer_actor_id)
    .fetch_all(pool)
    .await?;
    let remaining = sqlx::query_scalar::<_, i32>(
        "SELECT agent_turns_remaining FROM conversations WHERE id = $1 AND conversation_visible_to(id, $2)",
    )
    .bind(conversation_id)
    .bind(viewer_actor_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
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
            progress: row.progress_text,
            progress_at: row.progress_at,
            updated_at: row.updated_at,
            agent_last_seen_at: row.agent_last_seen_at,
            thread_id: row.thread_id,
            follows_request_id: row.follows_request_id,
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
    let outcome = continue_conversation_in(
        &mut transaction,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        conversation_id,
    )
    .await?;
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        outcome.project_id,
        "conversation.continued",
        conversation_id,
        serde_json::json!({ "asked": outcome.asked.len() }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(ContinueResponse {
        agent_turns_remaining: outcome.agent_turns_remaining,
    }))
}

/// What `continue_conversation_in` did.
pub(crate) struct ContinueOutcome {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub agent_turns_remaining: i32,
    /// The newest message that wasn't removed, if any.
    pub latest_message_id: Option<Uuid>,
    /// Agents with an open request about that message, after continuing.
    pub asked: Vec<ActorView>,
}

/// Let the agents in a group (or a thread under one) take another round:
/// reset its turn allowance and, when the newest message is an agent's, ask
/// the group's other agents to answer it. People only, so agents can't keep a
/// conversation going on their own. Shared by the browser API and MCP;
/// callers record their own audit rows.
///
/// A removed message is never asked about: the newest message that wasn't
/// removed counts. Asking about the same message again leaves each agent
/// exactly one open request (`insert_requests` never supersedes the request
/// about the message being asked about).
pub(crate) async fn continue_conversation_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    conversation_id: Uuid,
) -> Result<ContinueOutcome, AppError> {
    if actor_kind != ActorKind::Human {
        return Err(AppError::Forbidden);
    }
    let conversation = lock_conversation_for(
        transaction,
        actor_id,
        actor_kind,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await?;
    if conversation.effective_kind() != "group" {
        return Err(AppError::Conflict("conversation", "only groups have turns"));
    }
    sqlx::query("UPDATE conversations SET agent_turns_remaining = $2 WHERE id = $1")
        .bind(conversation_id)
        .bind(TURN_ALLOWANCE)
        .execute(&mut **transaction)
        .await?;
    let latest = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        r#"
        SELECT message.id, message.author_actor_id, actor.kind
        FROM conversation_entries entry
        JOIN messages message ON message.id = entry.message_id
        JOIN actors actor ON actor.id = message.author_actor_id
        WHERE entry.conversation_id = $1
          AND message.removed_at IS NULL
        ORDER BY entry.sequence DESC
        LIMIT 1
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some((message_id, author_actor_id, author_kind)) = &latest
        && author_kind == "external_agent"
    {
        ask(
            transaction,
            &NewMessage {
                project_id: conversation.project_id,
                conversation_id,
                members_conversation_id: conversation.membership_conversation_id(),
                conversation_kind: "group",
                message_id: *message_id,
                author_actor_id: *author_actor_id,
                author_kind,
                mentioned_actor_ids: &[],
            },
            false,
        )
        .await?;
    }
    notify(transaction, conversation.project_id, conversation_id).await?;
    let latest_message_id = latest.map(|(message_id, _, _)| message_id);
    let asked = match latest_message_id {
        Some(message_id) => sqlx::query_as::<_, (Uuid, String, String, String)>(
            r#"
            SELECT actor.id, actor.kind, membership.display_name, membership.handle
            FROM reply_requests request
            JOIN actors actor ON actor.id = request.agent_actor_id
            JOIN project_memberships membership
              ON membership.project_id = request.project_id
             AND membership.actor_id = request.agent_actor_id
            WHERE request.conversation_id = $1
              AND request.message_id = $2
              AND request.status IN ('pending', 'claimed')
            ORDER BY lower(membership.display_name), actor.id
            "#,
        )
        .bind(conversation_id)
        .bind(message_id)
        .fetch_all(&mut **transaction)
        .await?
        .into_iter()
        .map(|(id, kind, display_name, handle)| ActorView {
            id,
            kind,
            display_name,
            handle,
            avatar_url: None,
        })
        .collect(),
        None => Vec::new(),
    };
    Ok(ContinueOutcome {
        project_id: conversation.project_id,
        conversation_id,
        agent_turns_remaining: TURN_ALLOWANCE,
        latest_message_id,
        asked,
    })
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
    retry_request_in(
        &mut transaction,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        request_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(RetryResponse {
        id: request_id,
        status: "pending",
    }))
}

/// What retrying a reply request did, for the caller's audit row.
pub(crate) struct RetryOutcome {
    pub(crate) project_id: Uuid,
    pub(crate) agent_actor_id: Uuid,
}

/// Put an expired, released or stopped request back in the agent's inbox
/// and wake it, for someone who may post in its conversation. Someone who
/// can't see the conversation gets the same answer as for a request that
/// doesn't exist. Shared by the browser API and MCP; callers record their
/// own audit rows.
pub(crate) async fn retry_request_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    request_id: Uuid,
) -> Result<RetryOutcome, AppError> {
    let (conversation_id, agent_actor_id, thread_id) =
        sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>)>(
            "SELECT conversation_id, agent_actor_id, thread_id FROM reply_requests WHERE id = $1",
        )
        .bind(request_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("reply request"))?;
    let conversation = lock_conversation_for(
        transaction,
        actor_id,
        actor_kind,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await?;
    // Retrying asks the agent again, so it has to be able to answer: still
    // active, still in the project, and able to see the conversation. An
    // agent removed from a group would otherwise hold a pending request it
    // can never see. Requests about a removed message were withdrawn when it
    // was removed; the update below refuses them anyway.
    let answerable = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT actor.status = 'active' AND membership.enabled
               AND conversation_visible_to(request.conversation_id, request.agent_actor_id)
        FROM reply_requests request
        JOIN actors actor ON actor.id = request.agent_actor_id
        JOIN project_memberships membership
          ON membership.project_id = request.project_id
         AND membership.actor_id = request.agent_actor_id
        WHERE request.id = $1
        "#,
    )
    .bind(request_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !answerable {
        return Err(AppError::Conflict(
            "reply request",
            "the agent can no longer answer in this conversation",
        ));
    }
    let updated = sqlx::query(
        r#"
        UPDATE reply_requests request
        SET status = 'pending', reason = NULL, updated_at = now()
        FROM messages message
        WHERE request.id = $1 AND request.status IN ('expired', 'released', 'cancelled')
          AND message.id = request.message_id AND message.removed_at IS NULL
        "#,
    )
    .bind(request_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "reply request",
            "only expired, released or stopped requests can be retried",
        ));
    }
    wake(
        transaction,
        conversation.project_id,
        conversation_id,
        agent_actor_id,
        request_id,
        Wakeup::Requested,
    )
    .await?;
    notify_request(
        transaction,
        conversation.project_id,
        conversation_id,
        thread_id,
    )
    .await?;
    Ok(RetryOutcome {
        project_id: conversation.project_id,
        agent_actor_id,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StopResponse {
    id: Uuid,
    status: &'static str,
    /// Every request stopped: this one and the agent's follow-ups queued
    /// behind it in the same place.
    stopped: usize,
}

/// Stop an agent's work on a request: a person's Stop button.
async fn stop_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(request_id): ApiPath<Uuid>,
) -> Result<Json<StopResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let outcome = stop_request_in(
        &mut transaction,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        request_id,
    )
    .await?;
    insert_audit_event_for(
        &mut transaction,
        &headers,
        &principal,
        outcome.project_id,
        "reply_request.stopped",
        "reply_request",
        request_id,
        serde_json::json!({
            "agent_actor_id": outcome.agent_actor_id,
            "stopped": outcome.stopped.len(),
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(StopResponse {
        id: request_id,
        status: "cancelled",
        stopped: outcome.stopped.len(),
    }))
}

/// What `stop_request_in` did.
pub(crate) struct StopOutcome {
    pub project_id: Uuid,
    pub agent_actor_id: Uuid,
    /// The requests stopped, the asked one first.
    pub stopped: Vec<Uuid>,
}

/// Stop an agent's pending or claimed request, and the follow-ups queued
/// behind it where it answers. People only, and only people who can see the
/// conversation. The agent's next call on any of them is refused with the
/// reason, and its connections get a `reply.cancelled` wakeup at once.
/// Shared by the browser API and MCP; callers record their own audit rows.
pub(crate) async fn stop_request_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    request_id: Uuid,
) -> Result<StopOutcome, AppError> {
    if actor_kind != ActorKind::Human {
        return Err(AppError::Forbidden);
    }
    let Some((conversation_id, answer_conversation_id, agent_actor_id)) =
        sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
            r#"
            SELECT conversation_id, COALESCE(thread_id, conversation_id), agent_actor_id
            FROM reply_requests WHERE id = $1
            "#,
        )
        .bind(request_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Err(AppError::NotFound("reply request"));
    };
    // The conversation's lock first, then the request rows (the order
    // `withdraw_requests` documents). Someone who can't see the conversation
    // gets the same answer as for a request that doesn't exist.
    let conversation = match lock_conversation_for(
        transaction,
        actor_id,
        actor_kind,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await
    {
        Ok(conversation) => conversation,
        Err(AppError::NotFound(_)) => return Err(AppError::NotFound("reply request")),
        Err(error) => return Err(error),
    };
    let name = sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(conversation.project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or_else(|| "someone".to_owned());
    let stopped = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>)>(
        r#"
        UPDATE reply_requests
        SET status = 'cancelled', lease_expires_at = NULL, reason = $4, updated_at = now()
        WHERE agent_actor_id = $2
          AND status IN ('pending', 'claimed')
          AND (id = $1 OR (COALESCE(thread_id, conversation_id) = $3 AND status = 'pending'))
        RETURNING id, conversation_id, thread_id
        "#,
    )
    .bind(request_id)
    .bind(agent_actor_id)
    .bind(answer_conversation_id)
    .bind(format!("Stopped by {name}."))
    .fetch_all(&mut **transaction)
    .await?;
    if !stopped.iter().any(|(id, _, _)| *id == request_id) {
        return Err(AppError::Conflict(
            "reply request",
            "only a waiting or working request can be stopped",
        ));
    }
    // The agent's session in that conversation stops too (A5 with A3).
    crate::agent_sessions::cancel_for_request(transaction, request_id).await?;
    let mut conversations = BTreeSet::new();
    for (id, request_conversation_id, thread_id) in &stopped {
        wake(
            transaction,
            conversation.project_id,
            *request_conversation_id,
            agent_actor_id,
            *id,
            Wakeup::Cancelled,
        )
        .await?;
        conversations.insert(*request_conversation_id);
        conversations.extend(*thread_id);
    }
    for conversation_id in conversations {
        notify(transaction, conversation.project_id, conversation_id).await?;
    }
    let mut ids = stopped.into_iter().map(|(id, _, _)| id).collect::<Vec<_>>();
    ids.sort_by_key(|id| *id != request_id);
    Ok(StopOutcome {
        project_id: conversation.project_id,
        agent_actor_id,
        stopped: ids,
    })
}

/// Background loop: expire lapsed claims and deliver webhook wakeups.
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    // Each delivery builds its own client, pinned to the address it checked
    // (see `webhook_client`), so there is no shared one here.
    loop {
        if let Err(error) = expire_leases(&state.pool).await {
            tracing::warn!(category = error.log_category(), "reply lease sweep failed");
        }
        if let Err(error) = deliver_webhooks(&state).await {
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
    event: String,
    attempts: i32,
    agent_actor_id: Uuid,
    project_id: Uuid,
    conversation_id: Uuid,
    thread_id: Option<Uuid>,
    message_id: Uuid,
    generation: i32,
    follows_request_id: Option<Uuid>,
    /// Whether what the wakeup says is still true: the request still waits
    /// for the agent (or, for a stop, was stopped), and the agent can still
    /// reach it.
    still_true: bool,
    /// The agent's session in the conversation (A3), to resume.
    session_ref: Option<String>,
    session_harness: Option<String>,
    wakeup_format: String,
    webhook_url: Option<String>,
    /// Plaintext from before the secrets store; moved into it at startup.
    webhook_secret: Option<String>,
    webhook_secret_id: Option<Uuid>,
}

async fn deliver_webhooks(state: &AppState) -> Result<(), AppError> {
    let pool = &state.pool;
    let trusted_origins = &state.config.webhook_trusted_origins;
    let mut transaction = pool.begin().await?;
    let due = sqlx::query_as::<_, DueDelivery>(
        r#"
        SELECT delivery.id, delivery.connection_id, delivery.reply_request_id,
               delivery.event, delivery.attempts, request.agent_actor_id,
               request.project_id, request.conversation_id, request.thread_id,
               request.message_id, request.generation, request.follows_request_id,
               (
                   CASE delivery.event
                       WHEN 'reply.cancelled' THEN request.status = 'cancelled'
                       ELSE request.status = 'pending'
                   END
                   AND agent_connection_can_access(connection.id, request.project_id)
                   AND conversation_visible_to(request.conversation_id, request.agent_actor_id)
               ) AS still_true,
               session.external_ref AS session_ref, session.harness AS session_harness,
               connection.wakeup_format,
               connection.webhook_url, connection.webhook_secret, connection.webhook_secret_id
        FROM agent_webhook_deliveries delivery
        JOIN reply_requests request ON request.id = delivery.reply_request_id
        JOIN external_agent_connections connection ON connection.id = delivery.connection_id
        LEFT JOIN agent_sessions session
          ON session.agent_actor_id = request.agent_actor_id
         AND session.conversation_id = COALESCE(request.thread_id, request.conversation_id)
         AND session.state IN ('active', 'idle')
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
        // A stored secret that can't be opened is a failed attempt, retried
        // with backoff, not a wakeup quietly dropped.
        let mut secret_unreadable = false;
        let secret = match (&delivery.webhook_secret_id, &delivery.webhook_secret) {
            (Some(id), _) => {
                let revealed = match crate::secrets::keyring(&state.config) {
                    Ok(keys) => crate::secrets::reveal(
                        &mut *transaction,
                        keys.as_ref(),
                        *id,
                        "agent_webhook.sign",
                    )
                    .await
                    .ok(),
                    Err(_) => None,
                };
                secret_unreadable = revealed.is_none();
                revealed
            }
            (None, Some(plaintext)) => Some(crate::secrets::SecretValue::new(plaintext.clone())),
            (None, None) => None,
        };
        let placeholder = crate::secrets::SecretValue::new(String::new());
        // Bridges on the old format only understand "check your inbox", which
        // a stop doesn't need.
        let legacy_stop = delivery.wakeup_format == "inbox" && delivery.event == "reply.cancelled";
        let (Some(url), Some(secret), true, false) = (
            delivery.webhook_url.as_deref(),
            secret
                .as_ref()
                .or(secret_unreadable.then_some(&placeholder))
                .map(crate::secrets::SecretValue::expose),
            delivery.still_true,
            legacy_stop,
        ) else {
            // Answered, superseded, or the webhook was removed: nothing to wake.
            sqlx::query("UPDATE agent_webhook_deliveries SET delivered_at = now() WHERE id = $1")
                .bind(delivery.id)
                .execute(&mut *transaction)
                .await?;
            continue;
        };
        let error = if secret_unreadable {
            Some("the webhook's signing secret couldn't be read")
        } else {
            post_wakeup(url, secret, &delivery, trusted_origins)
                .await
                .err()
        };
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

/// The webhook body. Typed wakeups say what happened and where, so a runner
/// can route them (to the session already working in a thread, say) without a
/// round trip. Neither format carries message text: the runner claims the
/// request to read it, and every read is checked against who can see the
/// conversation.
fn wakeup_body(delivery: &DueDelivery) -> serde_json::Value {
    if delivery.wakeup_format == "inbox" {
        return serde_json::json!({
            "event": "inbox.available",
            "deliveryId": delivery.id,
            "agentId": delivery.agent_actor_id,
            "replyRequestId": delivery.reply_request_id,
        });
    }
    serde_json::json!({
        "event": delivery.event,
        "deliveryId": delivery.id,
        "agentId": delivery.agent_actor_id,
        "projectId": delivery.project_id,
        "replyRequestId": delivery.reply_request_id,
        "conversationId": delivery.conversation_id,
        "threadId": delivery.thread_id,
        "messageId": delivery.message_id,
        "generation": delivery.generation,
        "followsRequestId": delivery.follows_request_id,
        // The agent's own session in this conversation (A3), so it resumes
        // rather than starting cold; null until it binds one.
        "sessionRef": delivery.session_ref,
        "sessionHarness": delivery.session_harness,
    })
}

async fn post_wakeup(
    url: &str,
    secret: &str,
    delivery: &DueDelivery,
    trusted_origins: &[String],
) -> Result<(), &'static str> {
    let (url, client) = webhook_client(url, trusted_origins).await?;
    let body = wakeup_body(delivery);
    let event = body["event"].as_str().unwrap_or_default().to_owned();
    let body = body.to_string();
    let timestamp = unix_timestamp();
    let signature = sign(secret, &timestamp, &body).ok_or("could not sign the wakeup")?;
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-workspace-delivery", delivery.id.to_string())
        .header("x-workspace-event", event)
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

/// What a runner answered to a test wakeup.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TestWakeup {
    pub ok: bool,
    /// The HTTP status the runner answered with, if it answered.
    pub status: Option<u16>,
    /// Why it didn't work, in words a person can act on.
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub delivery_id: Uuid,
}

/// Send a signed `ping` to a runner, now, and report what it answered. It is
/// signed and sent exactly like a real wakeup (same headers, same timeout,
/// same address checks), but names no request, so a runner only has to
/// verify it and answer 2xx. Nothing is queued or retried.
pub(crate) async fn send_test_wakeup(
    url: &str,
    secret: &str,
    agent_id: Uuid,
    project_id: Uuid,
    trusted_origins: &[String],
) -> TestWakeup {
    let delivery_id = Uuid::now_v7();
    let started = std::time::Instant::now();
    let outcome = async {
        let (url, client) = webhook_client(url, trusted_origins)
            .await
            .map_err(|error| (None, error.to_owned()))?;
        let body = serde_json::json!({
            "event": "ping",
            "deliveryId": delivery_id,
            "agentId": agent_id,
            "projectId": project_id,
            "message": "A test wakeup from Agent Workspace. Verify the signature and answer 2xx; there is nothing to claim.",
        })
        .to_string();
        let timestamp = unix_timestamp();
        let signature = sign(secret, &timestamp, &body)
            .ok_or((None, "could not sign the wakeup".to_owned()))?;
        let response = client
            .post(url)
            .header("content-type", "application/json")
            .header("x-workspace-delivery", delivery_id.to_string())
            .header("x-workspace-event", "ping")
            .header("x-workspace-timestamp", &timestamp)
            .header("x-workspace-signature", format!("sha256={signature}"))
            .body(body)
            .send()
            .await
            .map_err(|error| {
                (
                    None,
                    if error.is_timeout() {
                        "the runner didn't answer within 8 seconds".to_owned()
                    } else {
                        "the runner couldn't be reached".to_owned()
                    },
                )
            })?;
        let status = response.status();
        if status.is_success() {
            Ok(status.as_u16())
        } else if matches!(status.as_u16(), 401 | 403) {
            Err((
                Some(status.as_u16()),
                "the runner refused the signature: check it uses this webhook's current secret"
                    .to_owned(),
            ))
        } else {
            Err((
                Some(status.as_u16()),
                format!("the runner answered {status}"),
            ))
        }
    }
    .await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(status) => TestWakeup {
            ok: true,
            status: Some(status),
            error: None,
            elapsed_ms,
            delivery_id,
        },
        Err((status, error)) => TestWakeup {
            ok: false,
            status,
            error: Some(error),
            elapsed_ms,
            delivery_id,
        },
    }
}

/// Validate webhook destinations without making a network request: public
/// https only, unless the URL's origin is one the operator trusts
/// (`AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS`), which may also be http on a
/// private or tailnet address. Credentials and fragments are never allowed.
pub(crate) fn validate_webhook_url(
    value: &str,
    trusted_origins: &[String],
) -> Result<reqwest::Url, AppError> {
    let url = reqwest::Url::parse(value).map_err(|_| AppError::Validation("webhook URL"))?;
    let host = url.host().ok_or(AppError::Validation("webhook URL"))?;
    if value.len() > 500
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::Validation("webhook URL"));
    }
    if is_trusted_origin(&url, trusted_origins) {
        return Ok(url);
    }
    let loopback = match host {
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
        url::Host::Domain(_) => false,
    };
    if url.scheme() != "https"
        || matches!(host, url::Host::Domain(name) if name.eq_ignore_ascii_case("localhost") || name.ends_with(".localhost"))
        || matches!(host, url::Host::Ipv4(address) if !loopback && !public_ipv4(address))
        || matches!(host, url::Host::Ipv6(address) if !loopback && !public_ipv6(address))
        || loopback
    {
        return Err(AppError::Validation("webhook URL"));
    }
    Ok(url)
}

fn is_trusted_origin(url: &reqwest::Url, trusted_origins: &[String]) -> bool {
    matches!(url.scheme(), "http" | "https")
        && trusted_origins.contains(&url.origin().ascii_serialization())
}

pub(crate) async fn webhook_client(
    value: &str,
    trusted_origins: &[String],
) -> Result<(reqwest::Url, reqwest::Client), &'static str> {
    let url =
        validate_webhook_url(value, trusted_origins).map_err(|_| "the webhook URL is unsafe")?;
    let trusted = is_trusted_origin(&url, trusted_origins);
    let host = url
        .host_str()
        .ok_or("the webhook URL is unsafe")?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or("the webhook URL is unsafe")?;
    let addresses = tokio::time::timeout(
        WEBHOOK_TIMEOUT,
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| "the webhook host could not be resolved")?
    .map_err(|_| "the webhook host could not be resolved")?
    .collect::<Vec<_>>();
    if addresses.is_empty()
        || !trusted
            && addresses.iter().any(|address| match address.ip() {
                std::net::IpAddr::V4(ip) => !public_ipv4(ip),
                std::net::IpAddr::V6(ip) => !public_ipv6(ip),
            })
    {
        return Err("the webhook host resolved to an unsafe address");
    }
    // Pin the already-checked address so a second DNS lookup cannot be used
    // for rebinding. Disabling ambient proxies also prevents them from
    // changing the destination selected by this security check.
    let client = reqwest::Client::builder()
        .timeout(WEBHOOK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve(&host, addresses[0])
        .build()
        .map_err(|_| "the webhook client could not be built")?;
    Ok((url, client))
}

pub(crate) fn public_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

pub(crate) fn public_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    !ip.is_unspecified()
        && !ip.is_loopback()
        && (segments[0] & 0xfe00) != 0xfc00
        && (segments[0] & 0xffc0) != 0xfe80
        && (segments[0] & 0xff00) != 0xff00
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        && ip.to_ipv4_mapped().is_none_or(public_ipv4)
        && embedded_ipv4(ip).is_none_or(public_ipv4)
}

/// The IPv4 address inside a NAT64 (64:ff9b::/96), 6to4 (2002::/16) or
/// IPv4-compatible (::a.b.c.d) address, which can reach it on hosts that
/// translate them.
fn embedded_ipv4(ip: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let s = ip.segments();
    let from = |high: u16, low: u16| {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        std::net::Ipv4Addr::new(a, b, c, d)
    };
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        Some(from(s[6], s[7]))
    } else if s[0] == 0x2002 {
        Some(from(s[1], s[2]))
    } else if s[..6] == [0, 0, 0, 0, 0, 0] && !ip.is_loopback() && !ip.is_unspecified() {
        Some(from(s[6], s[7]))
    } else {
        None
    }
}

/// HMAC-SHA256 over "{timestamp}.{body}", hex encoded.
pub fn sign(secret: &str, timestamp: &str, body: &str) -> Option<String> {
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
    fn ipv6_addresses_carrying_a_private_ipv4_are_not_public() {
        for private in [
            "64:ff9b::10.0.0.1",
            "2002:a00:1::",
            "::192.168.1.1",
            "::ffff:127.0.0.1",
        ] {
            let ip = private.parse::<std::net::Ipv6Addr>().expect("address");
            assert!(!public_ipv6(ip), "{private}");
        }
        for public in ["64:ff9b::8.8.8.8", "2002:808:808::", "2606:4700::1111"] {
            let ip = public.parse::<std::net::Ipv6Addr>().expect("address");
            assert!(public_ipv6(ip), "{public}");
        }
    }

    #[test]
    fn signatures_cover_timestamp_and_body() {
        let signature = sign("secret", "1700000000", "{}").expect("sign");
        assert_eq!(signature.len(), 64);
        assert_ne!(Some(signature.clone()), sign("secret", "1700000001", "{}"));
        assert_ne!(Some(signature), sign("other", "1700000000", "{}"));
    }

    #[test]
    fn browser_webhooks_reject_ssrf_and_credential_vectors() {
        for value in [
            "http://example.com/hook",
            "https://localhost/hook",
            "https://127.0.0.1/hook",
            "https://169.254.169.254/latest/meta-data",
            "https://10.0.0.1/hook",
            "https://user:password@example.com/hook",
            "https://example.com/hook#secret",
        ] {
            assert!(
                validate_webhook_url(value, &[]).is_err(),
                "accepted {value}"
            );
        }
        assert!(validate_webhook_url("https://example.com/hook", &[]).is_ok());
    }

    #[test]
    fn only_operator_trusted_origins_may_use_http_and_tailnet_addresses() {
        let trusted = ["http://100.119.56.3:8732".to_owned()];
        assert!(validate_webhook_url("http://100.119.56.3:8732/wake", &trusted).is_ok());
        // Without the operator's say-so the same listener is refused.
        assert!(validate_webhook_url("http://100.119.56.3:8732/wake", &[]).is_err());
        for value in [
            // Another port, scheme or host on the same network is not trusted.
            "http://100.119.56.3:8733/wake",
            "https://100.119.56.3:8732/wake",
            "http://100.119.56.4:8732/wake",
            "http://169.254.169.254/latest/meta-data",
            // Credentials and fragments stay refused even for trusted origins.
            "http://user:secret@100.119.56.3:8732/wake",
            "http://100.119.56.3:8732/wake#secret",
        ] {
            assert!(
                validate_webhook_url(value, &trusted).is_err(),
                "accepted {value}"
            );
        }
    }
}

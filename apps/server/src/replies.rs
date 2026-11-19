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

pub mod ops;

use std::{collections::BTreeSet, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        ActorView, enqueue_event, insert_audit_event, insert_audit_event_for,
        lock_seen_conversation_for,
    },
    error::{ApiPath, AppError},
    outbox::{OutboxEventPayload, ReplyRequestsUpdatedPayload},
    policy::Action,
};

/// Replies allowed in a conversation, after a message that starts a chain,
/// before someone has to say Continue; the `replies.turn_allowance` setting
/// changes it per workspace or space (A2.8).
pub(crate) const TURN_ALLOWANCE: i32 = 6;
pub(crate) const LEASE_MINUTES: i32 = 15;
const WORKER_INTERVAL: Duration = Duration::from_secs(3);

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
    pub mentioned_actor_ids: &'a [Uuid],
}

/// The loop limits that apply in a space: the deepest chain of asks
/// (`replies.max_chain_depth`) and the replies allowed before someone says
/// Continue (`replies.turn_allowance`).
struct Limits {
    max_chain_depth: i32,
    turn_allowance: i32,
}

async fn limits(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<Limits, AppError> {
    Ok(Limits {
        max_chain_depth: crate::settings::space_integer(
            transaction,
            project_id,
            "replies.max_chain_depth",
        )
        .await?
        .try_into()
        .unwrap_or(i32::MAX),
        turn_allowance: crate::settings::space_integer(
            transaction,
            project_id,
            "replies.turn_allowance",
        )
        .await?
        .try_into()
        .unwrap_or(TURN_ALLOWANCE),
    })
}

/// How deep in a chain of asks a message is: one more than the deepest ask
/// its author is answering here (the one this message answered, or one
/// still open), and 0 when nobody asked them (a fresh start, whoever wrote
/// it). Mentions and replies by anyone ask, so this, not the kind of
/// account, is what ends a loop.
async fn chain_depth(
    transaction: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
) -> Result<i32, AppError> {
    Ok(sqlx::query_scalar::<_, i32>(
        r#"
        SELECT COALESCE(max(request.chain_depth)::integer + 1, 0)
        FROM reply_requests request
        WHERE request.agent_actor_id = $2
          AND (request.conversation_id = $1 OR request.thread_id = $1)
          AND (request.reply_message_id = $3 OR request.status IN ('pending', 'claimed'))
        "#,
    )
    .bind(message.conversation_id)
    .bind(message.author_actor_id)
    .bind(message.message_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Ask the right accounts to reply. Runs in the message's own transaction.
///
/// Anyone's message asks: in a channel or a thread under one, the accounts
/// it mentions and, in a thread, those already working there; in a DM or
/// group, the other members. Only accounts that take reply requests are
/// asked. Loops end by depth and turns, never by who wrote it: a message
/// that answers an ask is one deeper than it, nothing past
/// `replies.max_chain_depth` asks anyone, every message past the start of a
/// chain uses up a turn, and a fresh start gives the turns back (A2.8).
pub(crate) async fn ask_for_replies(
    transaction: &mut Transaction<'_, Postgres>,
    message: NewMessage<'_>,
) -> Result<(), AppError> {
    if !matches!(
        message.conversation_kind,
        "channel" | "thread" | "dm" | "group"
    ) {
        return Ok(());
    }
    let limits = limits(transaction, message.project_id).await?;
    let depth = chain_depth(transaction, &message).await?;
    if depth == 0 {
        sqlx::query("UPDATE conversations SET agent_turns_remaining = $2 WHERE id = $1")
            .bind(message.conversation_id)
            .bind(limits.turn_allowance)
            .execute(&mut **transaction)
            .await?;
    } else {
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
        if remaining == 0 || depth >= limits.max_chain_depth {
            // The chain has gone far enough: it waits for someone to start
            // afresh or say Continue.
            return notify(transaction, message.project_id, message.conversation_id).await;
        }
    }
    if matches!(message.conversation_kind, "channel" | "thread") {
        if !message.mentioned_actor_ids.is_empty() {
            ask(transaction, &message, true, depth).await?;
            // Tagging an agent backed by a harness launches a run (A8).
            if depth == 0 {
                crate::launch::queue_tag_launches(
                    transaction,
                    message.project_id,
                    message.conversation_id,
                    message.message_id,
                    message.mentioned_actor_ids,
                    false,
                )
                .await?;
            }
        }
        if message.conversation_kind == "thread" {
            let followers = thread_followers(transaction, &message).await?;
            insert_requests(transaction, &message, &followers, false, true, depth).await?;
        }
        // A2.9: a message nobody prompted invites those in ambient mode
        // here, unless they have a request open or answered just now.
        if depth == 0 {
            let ambient = crate::follows::ambient::invited(
                transaction,
                message.project_id,
                message.conversation_id,
                message.author_actor_id,
                message.mentioned_actor_ids,
            )
            .await?;
            insert_requests(transaction, &message, &ambient, false, false, depth).await?;
            // One a host runs (the built-in assistant too) is started, as a
            // mention would start it; others hear the request.
            if !ambient.is_empty() {
                crate::launch::queue_tag_launches(
                    transaction,
                    message.project_id,
                    message.conversation_id,
                    message.message_id,
                    &ambient,
                    true,
                )
                .await?;
            }
        }
        return Ok(());
    }
    let required = depth == 0 && message.conversation_kind == "dm";
    let asked = ask(transaction, &message, required, depth).await?;
    // A1.25: a DM to an agent a host runs, or a mention of one in a group,
    // launches it there as a tag does.
    if depth == 0 {
        let to_launch = if message.conversation_kind == "dm" {
            asked
        } else {
            asked
                .into_iter()
                .filter(|account| message.mentioned_actor_ids.contains(account))
                .collect()
        };
        if !to_launch.is_empty() {
            crate::launch::queue_tag_launches(
                transaction,
                message.project_id,
                message.conversation_id,
                message.message_id,
                &to_launch,
                true,
            )
            .await?;
        }
    }
    Ok(())
}

/// Ask the accounts newly @mentioned by editing a message, in a channel or
/// a thread under one. `message.mentioned_actor_ids` holds only the newly
/// mentioned (in no earlier revision). Nothing is superseded and turns don't
/// change; the edit is as deep in its chain as a new message from its author
/// would be, and past the deepest chain it asks nobody. In DMs and groups
/// (and threads under them) every member who takes requests was already
/// asked when the message was sent.
pub(crate) async fn ask_after_edit(
    transaction: &mut Transaction<'_, Postgres>,
    message: NewMessage<'_>,
) -> Result<(), AppError> {
    if !matches!(message.conversation_kind, "channel" | "thread")
        || message.mentioned_actor_ids.is_empty()
    {
        return Ok(());
    }
    let limits = limits(transaction, message.project_id).await?;
    let depth = chain_depth(transaction, &message).await?;
    if depth >= limits.max_chain_depth {
        return Ok(());
    }
    let agents = agents_to_ask(transaction, &message).await?;
    insert_requests(transaction, &message, &agents, true, false, depth).await?;
    Ok(())
}

/// Ask the accounts the reply rules pick, superseding their pending requests
/// about earlier messages here.
async fn ask(
    transaction: &mut Transaction<'_, Postgres>,
    message: &NewMessage<'_>,
    required: bool,
    depth: i32,
) -> Result<Vec<Uuid>, AppError> {
    let agents = agents_to_ask(transaction, message).await?;
    insert_requests(transaction, message, &agents, required, true, depth).await?;
    Ok(agents)
}

/// The accounts the reply rules ask about a message: only active accounts
/// that take reply requests, never its author. In a channel or thread under
/// one: those it mentions who can see it. In a DM or group (or a thread
/// under one): every other member.
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
              AND actor.accepts_reply_requests
              AND actor.status = 'active'
              AND actor.id = ANY($2)
              AND actor.id <> $4
              AND conversation_visible_to($3, actor.id)
            ORDER BY actor.id
            "#,
        )
        .bind(message.project_id)
        .bind(message.mentioned_actor_ids)
        .bind(message.conversation_id)
        .bind(message.author_actor_id)
        .fetch_all(&mut **transaction)
        .await?);
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT member.actor_id
        FROM conversation_members member
        JOIN actors actor ON actor.id = member.actor_id
        WHERE member.conversation_id = $1
          AND member.actor_id <> $2
          AND actor.accepts_reply_requests
          AND actor.status = 'active'
          AND conversation_visible_to($1, member.actor_id)
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
          AND actor.accepts_reply_requests
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
    depth: i32,
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
    // A2.10: a required request is due to be picked up within the space's
    // claim window; the sweep marks it unresponsive after that.
    let claim_window = if required {
        crate::settings::space_integer(
            transaction,
            message.project_id,
            "replies.claim_window_seconds",
        )
        .await?
    } else {
        0
    };
    let mut asked = 0;
    for &agent in agents {
        // When the agent is still working on a claim here, the new request
        // queues behind it and says which claim it follows.
        let inserted = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            r#"
            INSERT INTO reply_requests (
                id, project_id, conversation_id, message_id, agent_actor_id,
                required, mentioned, chain_depth, claim_due_at, follows_request_id
            )
            VALUES (
                $1, NULLIF($2, direct_scope_id()), $3, $4, $5, $6, $7, $8,
                CASE WHEN $6 THEN now() + make_interval(secs => $9) END,
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
        .bind(i16::try_from(depth).unwrap_or(i16::MAX))
        .bind(claim_window as f64)
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
                RETURNING COALESCE(project_id, direct_scope_id()), conversation_id
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
                RETURNING COALESCE(request.project_id, direct_scope_id()), request.conversation_id
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
    fn event_type(self) -> &'static crate::events::EventType {
        match self {
            Self::Requested => &crate::events::catalog::REPLY_REQUESTED,
            Self::FollowUp => &crate::events::catalog::REPLY_FOLLOWUP,
            Self::Cancelled => &crate::events::catalog::REPLY_CANCELLED,
        }
    }
}

/// Tells the agent what happened to a reply request: one event heard by
/// that agent alone, which its connections read or are pushed (A2.5).
async fn wake(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    agent_actor_id: Uuid,
    request_id: Uuid,
    wakeup: Wakeup,
) -> Result<(), AppError> {
    let Some((thread_id, message_id, generation, follows, required)) =
        sqlx::query_as::<_, (Option<Uuid>, Uuid, i32, Option<Uuid>, bool)>(
            "SELECT thread_id, message_id, generation, follows_request_id, required \
             FROM reply_requests WHERE id = $1",
        )
        .bind(request_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(());
    };
    crate::events::emit(
        transaction,
        crate::events::NewEvent::thing(
            wakeup.event_type(),
            project_id,
            "reply_request",
            request_id,
        )
        .access_thing(thread_id.unwrap_or(conversation_id))
        .audience(crate::events::Audience::Account(agent_actor_id))
        .payload(serde_json::json!({
            "replyRequestId": request_id,
            "conversationId": conversation_id,
            "threadId": thread_id,
            "messageId": message_id,
            "generation": generation,
            "followsRequestId": follows,
            "required": required,
        })),
    )
    .await?;
    Ok(())
}

/// Push a refresh to browsers watching this conversation.
pub(crate) async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    // The conversation's own place: a DM's is the direct scope, whatever
    // space the caller acts from.
    let place = sqlx::query_scalar::<_, Uuid>(
        "SELECT COALESCE(project_id, direct_scope_id()) FROM conversations \
         -- visibility: the place of an event, which carries no content\n \
         WHERE id = $1",
    )
    .bind(conversation_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or(project_id);
    enqueue_event(
        transaction,
        place,
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
        RETURNING COALESCE(project_id, direct_scope_id()), conversation_id, thread_id
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
    /// When a required request still waiting to be picked up passed the
    /// claim window (A2.10): the agent hasn't picked it up.
    pub unresponsive_at: Option<String>,
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
    unresponsive_at: Option<String>,
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
               agent_name.display_name AS agent_name,
               agent_name.handle AS agent_handle,
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
                   WHERE agent_connection_can_reach(connection.id, request.project_id)
                     AND connection.actor_id = request.agent_actor_id
               ) AS agent_last_seen_at,
               request.thread_id,
               request.follows_request_id,
               CASE WHEN request.status = 'pending' THEN request.unresponsive_at::text END
                   AS unresponsive_at
        FROM reply_requests request
        CROSS JOIN LATERAL place_profile(request.project_id, request.agent_actor_id) agent_name
        WHERE (request.conversation_id = $1 OR request.thread_id = $1)
          AND request.status <> 'superseded'
          AND conversation_visible_to($1, $2)
        ORDER BY request.created_at DESC, request.id
        LIMIT 60
        "#,
    )
    .bind(conversation_id)
    .bind(viewer_actor_id)
    .fetch_all(crate::db::scoped(pool))
    .await?;
    let remaining = sqlx::query_scalar::<_, i32>(
        "SELECT agent_turns_remaining FROM conversations WHERE id = $1 AND conversation_visible_to(id, $2)",
    )
    .bind(conversation_id)
    .bind(viewer_actor_id)
    .fetch_optional(crate::db::scoped(pool))
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
            unresponsive_at: row.unresponsive_at,
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let outcome =
        continue_conversation_in(&mut transaction, principal.actor_id, conversation_id).await?;
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
/// the group's other agents to answer it. Needs `replies.steer`
/// (roles decide who may keep a conversation going). Shared by the browser API and MCP;
/// callers record their own audit rows.
///
/// A removed message is never asked about: the newest message that wasn't
/// removed counts. Asking about the same message again leaves each agent
/// exactly one open request (`insert_requests` never supersedes the request
/// about the message being asked about).
pub(crate) async fn continue_conversation_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    conversation_id: Uuid,
) -> Result<ContinueOutcome, AppError> {
    let conversation =
        lock_seen_conversation_for(transaction, actor_id, conversation_id, Action::ReplySteer)
            .await?;
    let allowance = limits(transaction, conversation.place())
        .await?
        .turn_allowance;
    sqlx::query("UPDATE conversations SET agent_turns_remaining = $2 WHERE id = $1")
        .bind(conversation_id)
        .bind(allowance)
        .execute(&mut **transaction)
        .await?;
    let latest = sqlx::query_as::<_, (Uuid, Uuid, bool)>(
        r#"
        SELECT message.id, message.author_actor_id, actor.accepts_reply_requests
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
    let effective_kind = conversation.effective_kind();
    if let Some((message_id, author_actor_id, true)) = &latest
        && matches!(effective_kind, "dm" | "group")
    {
        ask(
            transaction,
            &NewMessage {
                project_id: conversation.place(),
                conversation_id,
                members_conversation_id: conversation.membership_conversation_id(),
                conversation_kind: effective_kind,
                message_id: *message_id,
                author_actor_id: *author_actor_id,
                mentioned_actor_ids: &[],
            },
            false,
            0,
        )
        .await?;
    }
    notify(transaction, conversation.place(), conversation_id).await?;
    let latest_message_id = latest.map(|(message_id, _, _)| message_id);
    let asked = match latest_message_id {
        Some(message_id) => sqlx::query_as::<_, (Uuid, String, String, String)>(
            r#"
            SELECT actor.id, actor.kind, agent_name.display_name, agent_name.handle
            FROM reply_requests request
            JOIN actors actor ON actor.id = request.agent_actor_id
            CROSS JOIN LATERAL place_profile(request.project_id, request.agent_actor_id) agent_name
            WHERE request.conversation_id = $1
              AND request.message_id = $2
              AND request.status IN ('pending', 'claimed')
            ORDER BY lower(agent_name.display_name), actor.id
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
        project_id: conversation.place(),
        conversation_id,
        agent_turns_remaining: allowance,
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    retry_request_in(&mut transaction, principal.actor_id, request_id).await?;
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
    let conversation =
        lock_seen_conversation_for(transaction, actor_id, conversation_id, Action::ReplySteer)
            .await?;
    // Retrying asks the agent again, so it has to be able to answer: still
    // active, still in the project, and able to see the conversation. An
    // agent removed from a group would otherwise hold a pending request it
    // can never see. Requests about a removed message were withdrawn when it
    // was removed; the update below refuses them anyway.
    let answerable = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT actor.status = 'active'
               AND (request.project_id IS NULL OR COALESCE(membership.enabled, false))
               AND conversation_visible_to(request.conversation_id, request.agent_actor_id)
        FROM reply_requests request
        JOIN actors actor ON actor.id = request.agent_actor_id
        LEFT JOIN project_memberships membership
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
    let window = crate::settings::space_integer(
        transaction,
        conversation.place(),
        "replies.claim_window_seconds",
    )
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE reply_requests request
        SET status = 'pending', reason = NULL, updated_at = now(),
            -- A2.10: asked again, it has a new window to pick it up in.
            unresponsive_at = NULL,
            claim_due_at = CASE WHEN request.required
                THEN now() + make_interval(secs => $2) END
        FROM messages message
        WHERE request.id = $1
          AND (request.status IN ('expired', 'released', 'cancelled')
               OR (request.status = 'pending' AND request.unresponsive_at IS NOT NULL))
          AND message.id = request.message_id AND message.removed_at IS NULL
        "#,
    )
    .bind(request_id)
    .bind(window as f64)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "reply request",
            "only expired, released, stopped or unanswered requests can be asked again",
        ));
    }
    wake(
        transaction,
        conversation.place(),
        conversation_id,
        agent_actor_id,
        request_id,
        Wakeup::Requested,
    )
    .await?;
    notify_request(
        transaction,
        conversation.place(),
        conversation_id,
        thread_id,
    )
    .await?;
    Ok(RetryOutcome {
        project_id: conversation.place(),
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let outcome = stop_request_in(&mut transaction, principal.actor_id, request_id).await?;
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
/// behind it where it answers. Needs `replies.steer` where the conversation
/// is, and only those who can see the conversation. The agent's next call on any of them is refused with the
/// reason, and its connections get a `reply.cancelled` wakeup at once.
/// Shared by the browser API and MCP; callers record their own audit rows.
pub(crate) async fn stop_request_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    request_id: Uuid,
) -> Result<StopOutcome, AppError> {
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
    let conversation = match lock_seen_conversation_for(
        transaction,
        actor_id,
        conversation_id,
        Action::ReplySteer,
    )
    .await
    {
        Ok(conversation) => conversation,
        Err(AppError::NotFound(_)) => return Err(AppError::NotFound("reply request")),
        Err(error) => return Err(error),
    };
    let name = sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM place_profile(NULLIF($1, direct_scope_id()), $2)",
    )
    .bind(conversation.place())
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
    // A1.4: a launched run behind any of them is asked to stop.
    let stopped_ids = stopped.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
    crate::runs::launch_paths::stopped(transaction, actor_id, agent_actor_id, &stopped_ids, None)
        .await?;
    let mut conversations = BTreeSet::new();
    for (id, request_conversation_id, thread_id) in &stopped {
        wake(
            transaction,
            conversation.place(),
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
        notify(transaction, conversation.place(), conversation_id).await?;
    }
    let mut ids = stopped.into_iter().map(|(id, _, _)| id).collect::<Vec<_>>();
    ids.sort_by_key(|id| *id != request_id);
    Ok(StopOutcome {
        project_id: conversation.place(),
        agent_actor_id,
        stopped: ids,
    })
}

/// A1.4: someone stopped a launched run from the run itself (`run_cancel`).
/// Its reply request stops the way the request's own Stop does, so a runner
/// that only knows the reply tools hears `reply.cancelled` and its claim is
/// revoked. The caller has already checked that `by` may stop the run.
/// Returns whether the request was still open to stop.
pub(crate) async fn stop_for_run(
    transaction: &mut Transaction<'_, Postgres>,
    by: Uuid,
    request_id: Uuid,
    reason: Option<String>,
) -> Result<bool, AppError> {
    let Some((project_id, conversation_id)) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT COALESCE(project_id, direct_scope_id()), conversation_id FROM reply_requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(false);
    };
    // The conversation's lock first, then the request, then the run: the
    // order the request's own Stop takes.
    sqlx::query(
        "SELECT id FROM conversations WHERE id = $1 FOR UPDATE -- visibility: a lock only, on the request's own conversation",
    )
        .bind(conversation_id)
        .execute(&mut **transaction)
        .await?;
    let name = sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM place_profile(NULLIF($1, direct_scope_id()), $2)",
    )
    .bind(project_id)
    .bind(by)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or_else(|| "someone".to_owned());
    let Some((agent_actor_id, thread_id)) = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        r#"
        UPDATE reply_requests
        SET status = 'cancelled', lease_expires_at = NULL, reason = $2, updated_at = now()
        WHERE id = $1 AND status IN ('pending', 'claimed')
        RETURNING agent_actor_id, thread_id
        "#,
    )
    .bind(request_id)
    .bind(format!("Stopped by {name}."))
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(false);
    };
    crate::agent_sessions::cancel_for_request(transaction, request_id).await?;
    crate::runs::launch_paths::stopped(transaction, by, agent_actor_id, &[request_id], reason)
        .await?;
    wake(
        transaction,
        project_id,
        conversation_id,
        agent_actor_id,
        request_id,
        Wakeup::Cancelled,
    )
    .await?;
    notify(transaction, project_id, conversation_id).await?;
    if let Some(thread_id) = thread_id {
        notify(transaction, project_id, thread_id).await?;
    }
    Ok(true)
}

/// Background loop: expire lapsed claims. Wakeups are deliveries now
/// (`crate::delivery`).
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    loop {
        round(&state).await;
        tokio::select! {
            _ = tokio::time::sleep(WORKER_INTERVAL) => {}
            _ = shutdown.changed() => return,
        }
        if *shutdown.borrow() {
            return;
        }
    }
}

/// One pass: expire lapsed claims. A failure is logged; the next pass
/// tries again.
pub async fn round(state: &AppState) {
    if let Err(error) = expire_leases(&state.pool).await {
        tracing::warn!(category = error.log_category(), "reply lease sweep failed");
    }
}

async fn expire_leases(pool: &PgPool) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let expired = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        UPDATE reply_requests
        SET status = 'expired', lease_expires_at = NULL, updated_at = now(),
            reason = 'The agent stopped checking in before replying.'
        WHERE status = 'claimed' AND lease_expires_at < now()
        RETURNING COALESCE(project_id, direct_scope_id()), conversation_id
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
    mark_unresponsive(pool).await
}

/// A2.10: required requests nobody picked up within the claim window. The
/// conversation shows it, and the people who manage the account and can see
/// the conversation hear about it once an hour at most.
pub async fn mark_unresponsive(pool: &PgPool) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let lapsed = sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid)>(
        r#"
        UPDATE reply_requests
        SET unresponsive_at = now(), updated_at = now()
        WHERE id IN (
            SELECT id FROM reply_requests
            WHERE status = 'pending' AND unresponsive_at IS NULL
              AND claim_due_at IS NOT NULL AND claim_due_at < now()
            ORDER BY claim_due_at
            LIMIT 200
            FOR UPDATE SKIP LOCKED
        )
        RETURNING project_id, conversation_id, message_id, agent_actor_id
        "#,
    )
    .fetch_all(&mut *transaction)
    .await?;
    let mut seen = std::collections::HashSet::new();
    let hour = chrono::Utc::now().timestamp() / 3600;
    for (project_id, conversation_id, message_id, agent) in lapsed {
        if seen.insert(conversation_id) {
            notify(&mut transaction, project_id, conversation_id).await?;
        }
        // Its owner and named managers who are in the space and can see
        // the conversation.
        let managers = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT DISTINCT manager.id
            FROM actors agent
            JOIN actors manager
              ON manager.id = agent.owner_account_id
              OR manager.id IN (
                  SELECT manager_account_id FROM account_managers
                  WHERE account_id = agent.id
              )
            JOIN project_memberships membership
              ON membership.project_id = $2 AND membership.actor_id = manager.id
             AND membership.enabled
            WHERE agent.id = $1 AND manager.status = 'active'
              AND conversation_visible_to($3, manager.id)
            "#,
        )
        .bind(agent)
        .bind(project_id)
        .bind(conversation_id)
        .fetch_all(&mut *transaction)
        .await?;
        for manager in managers {
            let notification = crate::notifications::NewNotification {
                project_id,
                recipient_actor_id: manager,
                kind: crate::notifications::kind::AGENT_UNRESPONSIVE,
                subject_kind: "conversation",
                subject_id: conversation_id,
                actor_id: Some(agent),
                conversation_id: Some(conversation_id),
                message_id: Some(message_id),
                work_item_id: None,
                agent_run_id: None,
                pull_request_id: None,
                detail: serde_json::json!({}),
                group_key: format!("agent_unresponsive:{agent}"),
                dedupe_key: format!("agent_unresponsive:{agent}:{hour}"),
            };
            if crate::notifications::deliver(&mut transaction, &notification).await? {
                crate::notifications::announce(&mut transaction, project_id, manager).await?;
            }
        }
    }
    transaction.commit().await?;
    Ok(())
}

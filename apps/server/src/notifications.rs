//! Notifications for people (roadmap N1) and the inbox they fill (N2).
//!
//! Notifications are written by the `notifications` consumer of the event
//! log (K3.9): `fan_out` runs in the transaction that advances its cursor,
//! so they need no job queue and a failed batch retries with its events.
//! Each source occurrence (one @mention, one assignment, one failed run) carries a
//! dedupe key and is applied once per recipient, which makes redelivery safe.
//! Repeats in the same group (more mentions in one channel, a run moving from
//! waiting to finished) collapse into the one open row for that group, which
//! counts them and turns unread again.
//!
//! Only people get notifications, and never for their own actions. Rows hold
//! IDs, not text: titles and previews are read live when listing, so edits,
//! removals and renames show at once, and every read checks the viewer's
//! current access (`actor_can_access_project`, `conversation_visible_to`,
//! `pull_request_visible_to`), because access can be lost after a row was
//! written.
//!
//! What fills the inbox today: @mentions in messages and task comments, task
//! assignments, approvals waiting on you, your runs finishing, failing or
//! waiting, and failing checks and review requests on pull requests linked
//! to your runs or tasks. Later workstreams add kinds of their own through
//! `NewNotification` (the kind column is open-ended).

use std::collections::{BTreeSet, HashMap, HashSet};

use axum::{Json, Router, extract::State, routing::get};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    addresses::{Addresses, Place, Target, WorkspaceAddress},
    auth::AuthenticatedActor,
    domain::RunState,
    error::{ApiQuery, AppError},
    outbox::{self, NotificationsUpdatedPayload, OutboxEventPayload},
};

pub const MAX_PAGE_SIZE: i64 = 100;
pub const DEFAULT_PAGE_SIZE: i64 = 50;
pub const MAX_UPDATE_IDS: usize = 200;
const PREVIEW_CHARACTERS: i32 = 280;
/// How far ahead a snooze may reach.
const MAX_SNOOZE_DAYS: i64 = 366;
/// Realtime nudges sent for one bulk change, at most (one per project).
const MAX_CHANGE_EVENTS: usize = 50;

// ---------------------------------------------------------------------------
// Kinds

pub mod ops;
pub mod views;

pub mod kind {
    pub const MENTION: &str = "mention";
    pub const COMMENT_MENTION: &str = "comment_mention";
    pub const ASSIGNED: &str = "assigned";
    /// Someone asked you, or a role you hold, something: a question, an
    /// approval or a proposal (A1.8).
    pub const REQUEST_RAISED: &str = "request_raised";
    /// A request you raised was answered or ran out of time (A1.8).
    pub const REQUEST_ANSWERED: &str = "request_answered";
    pub const RUN_WAITING: &str = "run_waiting";
    pub const RUN_COMPLETED: &str = "run_completed";
    pub const RUN_FAILED: &str = "run_failed";
    pub const CHECKS_FAILED: &str = "checks_failed";
    pub const REVIEW_REQUESTED: &str = "review_requested";
    /// Someone else changed or commented on a task you watch.
    pub const TASK_CHANGED: &str = "task_changed";
    /// A new message in a conversation you set to every message (W21).
    pub const CONVERSATION_MESSAGE: &str = "conversation_message";
    /// A reminder you set is due (W21).
    pub const REMINDER: &str = "reminder";
    /// A message you scheduled couldn't be sent (W21).
    pub const SCHEDULED_SEND_FAILED: &str = "scheduled_send_failed";
    /// An agent suggested edits to a page you made (AWD-53).
    pub const PAGE_SUGGESTION: &str = "page_suggestion";
    /// Someone commented on a page you made, or in a page thread you're in
    /// (AWD-53).
    pub const PAGE_COMMENT: &str = "page_comment";
    /// Someone commented on a thing you follow: one you made, or one whose
    /// discussion you're in (C1.3).
    pub const THING_COMMENT: &str = "thing_comment";
    /// A message in a DM or group you're in, by someone else (C2.5): one
    /// row per conversation that counts up.
    pub const DIRECT_MESSAGE: &str = "direct_message";
    /// A reply in a thread you follow, by someone else (C2.5).
    pub const THREAD_REPLY: &str = "thread_reply";
    /// A message with one of your keywords, in a conversation you can read
    /// and haven't muted (C2.13).
    pub const KEYWORD: &str = "keyword";
    /// A2: an agent or app you manage lost its connection or credential.
    pub const CONNECTION_PROBLEM: &str = "connection_problem";
    /// A2: an agent you manage or asked is marked "not responding".
    pub const AGENT_UNRESPONSIVE: &str = "agent_unresponsive";
    /// A2/H1: a budget you own reached its warning or its limit.
    pub const BUDGET_WARNING: &str = "budget_warning";
    /// A2's follows: a thing you follow changed.
    pub const THING_UPDATE: &str = "thing_update";
    /// T1.29: a public form you made reached its daily limit and paused.
    pub const FORM_PAUSED: &str = "form_paused";

    /// Kinds that ask something of you (C2.4): someone mentioned or
    /// messaged you, gave you a task or a review, a request or a waiting run
    /// is yours to answer, or something of yours failed. Everything else is
    /// for your information.
    pub const NEEDS_YOU: &[&str] = &[
        MENTION,
        COMMENT_MENTION,
        CONVERSATION_MESSAGE,
        DIRECT_MESSAGE,
        ASSIGNED,
        REQUEST_RAISED,
        REVIEW_REQUESTED,
        RUN_WAITING,
        RUN_FAILED,
        CHECKS_FAILED,
        SCHEDULED_SEND_FAILED,
        REMINDER,
        CONNECTION_PROBLEM,
        BUDGET_WARNING,
    ];
}

/// Whether a notification needs you or is for your information (C2.4).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    NeedsYou,
    Fyi,
}

impl Attention {
    /// A request someone already answered no longer needs you.
    pub fn of(kind: &str, request_status: Option<&str>) -> Self {
        if kind::NEEDS_YOU.contains(&kind) && request_status.is_none_or(|status| status == "open") {
            Self::NeedsYou
        } else {
            Self::Fyi
        }
    }
}

/// The same rule in SQL, over `n` with the kinds bound at `$needs`.
fn needs_you_sql(needs: &str) -> String {
    format!(
        "(n.kind = ANY({needs}::text[]) AND NOT (n.subject_kind = 'request' AND EXISTS (\
            SELECT 1 FROM requests answered \
            WHERE answered.id = n.subject_id AND answered.status <> 'open')))"
    )
}

/// Groups of kinds people filter the inbox by.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    #[default]
    All,
    Mentions,
    /// Replies in threads you follow (C2.5).
    Threads,
    Tasks,
    Runs,
    PullRequests,
    /// Capacity alerts about your provider accounts (W9).
    Usage,
}

impl Category {
    fn kinds(self) -> Option<&'static [&'static str]> {
        match self {
            Self::All => None,
            Self::Mentions => Some(&[kind::MENTION, kind::COMMENT_MENTION, kind::KEYWORD]),
            Self::Threads => Some(&[kind::THREAD_REPLY]),
            Self::Tasks => Some(&[kind::ASSIGNED, kind::COMMENT_MENTION, kind::TASK_CHANGED]),
            Self::Runs => Some(&[
                kind::REQUEST_RAISED,
                kind::REQUEST_ANSWERED,
                kind::RUN_WAITING,
                kind::RUN_COMPLETED,
                kind::RUN_FAILED,
            ]),
            Self::PullRequests => Some(&[kind::CHECKS_FAILED, kind::REVIEW_REQUESTED]),
            Self::Usage => Some(crate::usage::alerts::kind::ALL),
        }
    }
}

/// Which part of the inbox to show.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// Everything not archived and not snoozed.
    #[default]
    Inbox,
    /// The inbox, unread only.
    Unread,
    Snoozed,
    Archived,
}

impl View {
    fn condition(self) -> &'static str {
        match self {
            Self::Inbox => {
                "n.archived_at IS NULL AND (n.snoozed_until IS NULL OR n.snoozed_until <= now())"
            }
            Self::Unread => {
                "n.archived_at IS NULL AND (n.snoozed_until IS NULL OR n.snoozed_until <= now()) AND n.read_at IS NULL"
            }
            Self::Snoozed => "n.archived_at IS NULL AND n.snoozed_until > now()",
            Self::Archived => "n.archived_at IS NOT NULL",
        }
    }
}

/// What a person can do to their notifications.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateAction {
    Read,
    Unread,
    Archive,
    /// Back to the inbox from the archive.
    Restore,
    /// Out of the inbox until `snoozeUntil`, then back unread.
    Snooze,
    /// Out of the inbox until something new happens on it (C2.10), then
    /// back unread; a year at most.
    SnoozeUntilActivity,
    Unsnooze,
}

// ---------------------------------------------------------------------------
// Writing

/// One occurrence to deliver to one person. Rows in the same `group_key`
/// collapse; `dedupe_key` makes the same occurrence land once.
#[derive(Clone, Debug)]
pub struct NewNotification {
    pub project_id: Uuid,
    pub recipient_actor_id: Uuid,
    pub kind: &'static str,
    pub subject_kind: &'static str,
    pub subject_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub message_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub pull_request_id: Option<Uuid>,
    pub detail: serde_json::Value,
    pub group_key: String,
    pub dedupe_key: String,
}

impl NewNotification {
    fn new(
        project_id: Uuid,
        recipient_actor_id: Uuid,
        kind: &'static str,
        subject_kind: &'static str,
        subject_id: Uuid,
        group_key: String,
        dedupe_key: String,
    ) -> Self {
        Self {
            project_id,
            recipient_actor_id,
            kind,
            subject_kind,
            subject_id,
            actor_id: None,
            conversation_id: None,
            message_id: None,
            work_item_id: None,
            agent_run_id: None,
            pull_request_id: None,
            detail: serde_json::json!({}),
            group_key,
            dedupe_key,
        }
    }
}

impl NewNotification {
    /// About a page: its title and address go in `detail`.
    pub fn page(
        project_id: Uuid,
        recipient_actor_id: Uuid,
        kind: &'static str,
        page_id: Uuid,
        group_key: String,
        dedupe_key: String,
    ) -> Self {
        Self::new(
            project_id,
            recipient_actor_id,
            kind,
            "page",
            page_id,
            group_key,
            dedupe_key,
        )
    }
}

/// Whether the occurrence `dedupe_key` was already delivered to `recipient`.
pub async fn delivered(
    transaction: &mut Transaction<'_, Postgres>,
    recipient: Uuid,
    dedupe_key: &str,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM notification_occurrences \
         WHERE recipient_actor_id = $1 AND dedupe_key = $2)",
    )
    .bind(recipient)
    .bind(dedupe_key)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Delivers one occurrence. Returns whether anything changed (false when
/// this occurrence was already applied). The caller checks the recipient may
/// see the subject; reads check again.
pub async fn deliver(
    transaction: &mut Transaction<'_, Postgres>,
    notification: &NewNotification,
) -> Result<bool, AppError> {
    if notification.actor_id == Some(notification.recipient_actor_id) {
        return Ok(false);
    }
    // C2.39: nothing from someone the recipient blocked.
    if let Some(actor) = notification.actor_id
        && crate::direct_rules::has_blocked(transaction, notification.recipient_actor_id, actor)
            .await?
    {
        return Ok(false);
    }
    let first = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT NOT EXISTS (
            SELECT 1 FROM notification_occurrences
            WHERE recipient_actor_id = $1 AND dedupe_key = $2
        )
        "#,
    )
    .bind(notification.recipient_actor_id)
    .bind(&notification.dedupe_key)
    .fetch_one(&mut **transaction)
    .await?;
    if !first {
        return Ok(false);
    }
    // AWD-75: a module that's off sends nothing, not even a push.
    let modules = crate::modules::state::ProjectModules::load(
        transaction,
        notification.recipient_actor_id,
        &[notification.project_id],
    )
    .await?;
    if !modules.allows_notification(notification.project_id, notification.kind) {
        return Ok(false);
    }
    // N4: a muted project or group writes nothing; in-app off but push on
    // writes the row straight to the archive, for the push to open.
    let decision = crate::notification_settings::decide(
        transaction,
        notification.recipient_actor_id,
        notification.project_id,
        notification.kind,
    )
    .await?;
    if decision.nothing() {
        return Ok(false);
    }
    // A message the recipient has already seen (they had the conversation
    // open when it arrived) comes in read.
    //
    // The read mark is locked (shared) before the notification row is
    // written, the same order `read_state::mark_seen` takes them in: a read
    // that is moving the mark right now finishes first and this sees where
    // it got to, and a read that comes after waits for this row to commit.
    // Without it, a read could skip this row while it was being written
    // (`read_for_conversation` skips locked rows) and leave it unread.
    let already_seen = match notification.message_id {
        Some(message_id) => sqlx::query_scalar::<_, bool>(
            r#"
            SELECT entry.sequence <= state.highest_seen_sequence
            FROM conversation_entries entry
            JOIN conversation_read_states state
              ON state.conversation_id = entry.conversation_id
             AND state.actor_id = $2
            WHERE entry.message_id = $1
            FOR SHARE OF state
            "#,
        )
        .bind(message_id)
        .bind(notification.recipient_actor_id)
        .fetch_all(&mut **transaction)
        .await?
        .into_iter()
        .any(|seen| seen),
        None => false,
    };
    let notification_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO notifications (
            id, project_id, recipient_actor_id, kind, subject_kind, subject_id,
            actor_id, conversation_id, message_id, work_item_id,
            agent_run_id, pull_request_id, detail, group_key, archived_at,
            read_at
        ) VALUES ($1, NULLIF($2, direct_scope_id()), $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                  CASE WHEN $15 THEN NULL ELSE now() END,
                  CASE WHEN $16 THEN now() END)
        ON CONFLICT (recipient_actor_id, group_key) WHERE archived_at IS NULL
        DO UPDATE SET
            kind = EXCLUDED.kind,
            actor_id = EXCLUDED.actor_id,
            message_id = EXCLUDED.message_id,
            pull_request_id = COALESCE(EXCLUDED.pull_request_id, notifications.pull_request_id),
            detail = EXCLUDED.detail,
            repeat_count = notifications.repeat_count + 1,
            read_at = CASE WHEN $16 THEN COALESCE(notifications.read_at, now()) END,
            -- Snoozed until something new: this is it (C2.10).
            surfaced_at = CASE WHEN notifications.wake_on_activity THEN now()
                ELSE GREATEST(now(), COALESCE(notifications.snoozed_until, now())) END,
            snoozed_until = CASE WHEN notifications.wake_on_activity THEN NULL
                ELSE notifications.snoozed_until END,
            wake_on_activity = false,
            updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(notification.project_id)
    .bind(notification.recipient_actor_id)
    .bind(notification.kind)
    .bind(notification.subject_kind)
    .bind(notification.subject_id)
    .bind(notification.actor_id)
    .bind(notification.conversation_id)
    .bind(notification.message_id)
    .bind(notification.work_item_id)
    .bind(notification.agent_run_id)
    .bind(notification.pull_request_id)
    .bind(&notification.detail)
    .bind(&notification.group_key)
    .bind(decision.in_app)
    .bind(already_seen)
    .fetch_one(&mut **transaction)
    .await?;
    if decision.push || decision.email {
        let organization_id = notice_workspace(
            transaction,
            notification.project_id,
            notification.recipient_actor_id,
        )
        .await?;
        if decision.push {
            crate::push::enqueue_delivery(
                transaction,
                organization_id,
                notification_id,
                notification.recipient_actor_id,
                &notification.dedupe_key,
            )
            .await?;
        }
        if decision.email {
            crate::notification_email::enqueue(
                transaction,
                organization_id,
                notification_id,
                notification.recipient_actor_id,
                &notification.dedupe_key,
            )
            .await?;
        }
    }
    sqlx::query(
        r#"
        INSERT INTO notification_occurrences (
            recipient_actor_id, dedupe_key, notification_id, message_id
        ) VALUES ($1, $2, $3, $4)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(notification.recipient_actor_id)
    .bind(&notification.dedupe_key)
    .bind(notification_id)
    .bind(notification.message_id)
    .execute(&mut **transaction)
    .await?;
    Ok(true)
}

/// The workspace a notification is sent from: its space's, or for a DM
/// (no space) the recipient's default workspace.
async fn notice_workspace(
    transaction: &mut Transaction<'_, Postgres>,
    place: Uuid,
    recipient: Uuid,
) -> Result<Uuid, AppError> {
    if crate::spaces::kind::holds_messages(place) {
        return crate::workspace::default_workspace(&mut **transaction, recipient)
            .await?
            .map(|(organization_id, _)| organization_id)
            .ok_or(AppError::NotFound("organization"));
    }
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(place)
            .fetch_one(&mut **transaction)
            .await?,
    )
}

/// The `notifications` consumer: every change notice in the log, turned
/// into notifications in the transaction that moves its cursor.
pub struct Notifier;

/// The notifier's cursor.
pub const NOTIFIER: &str = "notifications";

#[async_trait::async_trait]
impl crate::events::Consumer for Notifier {
    fn name(&self) -> &'static str {
        NOTIFIER
    }

    /// From the log's start, so nothing written before its first round is
    /// missed; deliveries dedupe on their keys.
    fn starts_at_head(&self) -> bool {
        false
    }

    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        batch: &[crate::events::LoggedEvent],
    ) -> Result<(), crate::events::ConsumerError> {
        for event in batch {
            // A DM's event is in the direct scope.
            let (Some(notice), Some(project_id)) = (&event.legacy_type, event.place()) else {
                continue;
            };
            let Ok(payload) = OutboxEventPayload::from_json(notice, event.payload.clone()) else {
                continue;
            };
            fan_out(transaction, event.id, project_id, &payload).await?;
        }
        Ok(())
    }
}

/// Turns one change notice into notifications, in the notifier's
/// transaction. Returns how many people were notified.
pub async fn fan_out(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    project_id: Uuid,
    payload: &OutboxEventPayload,
) -> Result<usize, AppError> {
    let notifications = match payload {
        OutboxEventPayload::MessageCreated(created) => {
            let mut found = message_mentions(transaction, created.message_id).await?;
            found.extend(followed_messages(transaction, created.message_id).await?);
            found.extend(direct_messages(transaction, created.message_id).await?);
            found.extend(thing_comments(transaction, created.message_id).await?);
            found.extend(keyword_messages(transaction, created.message_id).await?);
            found
        }
        OutboxEventPayload::MessageUpdated(updated) if updated.removed => {
            forget_message(transaction, updated.message_id).await?;
            Vec::new()
        }
        // An edit can add a mention; ones already delivered are skipped.
        OutboxEventPayload::MessageUpdated(updated) => {
            message_mentions(transaction, updated.message_id).await?
        }
        OutboxEventPayload::WorkItemUpdated(updated) => {
            let mut found =
                work_item_changes(transaction, event_id, project_id, updated.work_item_id).await?;
            found.extend(
                pull_request_notifications(
                    transaction,
                    project_id,
                    PullRequestSource::WorkItem(updated.work_item_id),
                )
                .await?,
            );
            found
        }
        OutboxEventPayload::AgentRunUpdated(updated) => {
            let mut found = match updated.state {
                Some(state) => run_notifications(transaction, updated.agent_run_id, state).await?,
                None => Vec::new(),
            };
            found.extend(
                pull_request_notifications(
                    transaction,
                    project_id,
                    PullRequestSource::Run(updated.agent_run_id),
                )
                .await?,
            );
            found
        }
        _ => Vec::new(),
    };
    let mut recipients = BTreeSet::new();
    for notification in &notifications {
        if deliver(transaction, notification).await? {
            recipients.insert((notification.project_id, notification.recipient_actor_id));
        }
    }
    for (project_id, actor_id) in &recipients {
        announce(transaction, *project_id, *actor_id).await?;
    }
    Ok(recipients.len())
}

/// Tells the recipient's open browsers their notifications changed.
pub(crate) async fn announce(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    outbox::enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::NotificationsUpdated(NotificationsUpdatedPayload { actor_id }),
    )
    .await?;
    Ok(())
}

/// Viewing a conversation up to `sequence` reads what the viewer was told
/// about it there. Snoozed notifications wait for their time.
///
/// A notification another transaction is writing right now is left alone
/// (`SKIP LOCKED`): it is being told about something newer, and waiting for
/// it deadlocked a send against the fan-out of the message before it, which
/// holds one of the viewer's rows while it waits for another. A skipped row
/// isn't left unread: `deliver` holds the viewer's read mark (shared) while
/// it writes, so the caller, having just moved that mark, has made `deliver`
/// wait and see the new mark.
pub(crate) async fn read_for_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    viewer: Uuid,
    conversation_id: Uuid,
    sequence: i64,
) -> Result<(), AppError> {
    let read = sqlx::query(
        r#"
        UPDATE notifications
        SET read_at = now(), updated_at = now()
        WHERE id IN (
            SELECT n.id
            FROM notifications n
            WHERE n.recipient_actor_id = $1
              AND n.conversation_id = $2
              AND n.read_at IS NULL
              AND n.archived_at IS NULL
              AND (n.snoozed_until IS NULL OR n.snoozed_until <= now())
              AND (n.message_id IS NULL OR EXISTS (
                  SELECT 1 FROM conversation_entries entry
                  WHERE entry.conversation_id = $2
                    AND entry.message_id = n.message_id
                    AND entry.sequence <= $3
              ))
            ORDER BY n.id
            FOR UPDATE OF n SKIP LOCKED
        )
        "#,
    )
    .bind(viewer)
    .bind(conversation_id)
    .bind(sequence)
    .execute(&mut **transaction)
    .await?;
    if read.rows_affected() > 0 {
        announce(transaction, project_id, viewer).await?;
    }
    Ok(())
}

#[derive(FromRow)]
struct MentionRow {
    project_id: Uuid,
    conversation_id: Uuid,
    author_actor_id: Uuid,
    recipient_actor_id: Uuid,
}

/// A conversation with more people than this who can see it takes an owner
/// or admin of its project to notify everyone.
const EVERYONE_OPEN_LIMIT: i64 = 50;

/// The people a message mentions: everyone it names, for `@everyone` every
/// person who can see the conversation, and for `@here` those of them online
/// and not away. Agents are only ever
/// named; the reply rules decide whether a message wakes them.
async fn message_mentions(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let rows = sqlx::query_as::<_, MentionRow>(
        r#"
        WITH message AS (
            SELECT id, COALESCE(project_id, direct_scope_id()) AS project_id, conversation_id,
                   author_actor_id, content,
                   may_notify_everyone
            FROM messages
            WHERE id = $1 AND removed_at IS NULL
        ),
        named AS (
            SELECT mention.actor_id
            FROM message
            JOIN message_mentions mention ON mention.message_id = message.id
        ),
        everyone AS (
            SELECT membership.actor_id
            FROM message
            JOIN project_memberships membership
              ON membership.project_id = message.project_id AND membership.enabled
            JOIN actors actor ON actor.id = membership.actor_id AND actor.kind = 'human' -- // kind-check: mechanical (people are notified; agents are asked to reply by the reply rules)
            -- Written in the text itself, so a person whose handle happens
            -- to be "everyone" is still only a mention of them.
            WHERE (EXISTS (
                    SELECT 1
                    FROM jsonb_array_elements(message.content -> 'nodes') node
                    WHERE node ->> 'type' = 'text' AND node ->> 'text' ~* $2
                  )
                  -- C2.16: `@here` reaches only the people online now (a
                  -- fresh connection on any instance) who aren't away.
                  OR (EXISTS (
                        SELECT 1
                        FROM jsonb_array_elements(message.content -> 'nodes') node
                        WHERE node ->> 'type' = 'text' AND node ->> 'text' ~* $4
                      )
                      AND EXISTS (
                        SELECT 1 FROM realtime_connections live
                        WHERE live.account_id = membership.actor_id
                          AND live.seen_at > now() - make_interval(secs => $5)
                      )
                      AND NOT EXISTS (
                        SELECT 1 FROM account_statuses status
                        WHERE status.account_id = membership.actor_id
                          AND (status.workspace_id IS NULL OR status.workspace_id = (
                              SELECT space.organization_id FROM projects space
                              WHERE space.id = message.project_id))
                          AND status.away_until > now()
                          AND (status.clear_at IS NULL OR status.clear_at > now())
                      )))
              AND conversation_visible_to(message.conversation_id, membership.actor_id)
        ),
        -- C2.18: `@handle` of a group of this workspace, written as a word:
        -- its members who can see the conversation, people and agents alike.
        grouped AS (
            SELECT DISTINCT member.account_id AS actor_id
            FROM message
            LEFT JOIN projects space ON space.id = message.project_id
            -- A DM or group is in no workspace (K4.17): the groups of the
            -- workspaces its author is in.
            JOIN account_groups grp
              ON grp.archived_at IS NULL
             AND (grp.workspace_id = space.organization_id
                  OR (message.project_id = direct_scope_id() AND grp.workspace_id IN (
                      SELECT author_in.organization_id FROM organization_memberships author_in
                      WHERE author_in.actor_id = message.author_actor_id AND author_in.enabled)))
            JOIN account_group_members member
              ON member.group_id = grp.id AND member.workspace_id = grp.workspace_id
            WHERE EXISTS (
                    SELECT 1
                    FROM jsonb_array_elements(message.content -> 'nodes') node
                    WHERE node ->> 'type' = 'text'
                      AND node ->> 'text' ~* ('(^|[^[:alnum:]_@])@' || grp.handle || '($|[^[:alnum:]_-])')
                  )
              AND conversation_visible_to(message.conversation_id, member.account_id)
        ),
        -- Whether the message may reach many people at once: decided when it
        -- was sent or last edited, with the app's agent limit then in force
        -- (messages.may_notify_everyone); older and system messages fall back
        -- to the author's role.
        may_reach_many AS (
            SELECT COALESCE((SELECT may_notify_everyone FROM message), EXISTS (
                   SELECT 1
                   FROM message
                   JOIN project_memberships author
                     ON author.project_id = message.project_id
                    AND author.actor_id = message.author_actor_id
                    AND author.enabled
                   WHERE role_allows(author.role, author.role_id, 'channels.mention_everyone')
               )) AS allowed
        ),
        everyone_allowed AS (
            SELECT actor_id FROM everyone
            WHERE (SELECT count(*) FROM everyone) <= $3
               OR (SELECT allowed FROM may_reach_many)
        ),
        -- A group as large as a crowd takes the same say as @everyone.
        grouped_allowed AS (
            SELECT actor_id FROM grouped
            WHERE (SELECT count(*) FROM grouped) <= $3
               OR (SELECT allowed FROM may_reach_many)
        )
        SELECT message.project_id, message.conversation_id, message.author_actor_id,
               recipient.actor_id AS recipient_actor_id
        FROM message
        JOIN (SELECT actor_id FROM named
              UNION SELECT actor_id FROM everyone_allowed
              UNION SELECT actor_id FROM grouped_allowed) recipient
          ON recipient.actor_id <> message.author_actor_id
        WHERE (message.project_id = direct_scope_id()
               OR actor_can_access_project(recipient.actor_id, NULL, message.project_id))
          AND conversation_visible_to(message.conversation_id, recipient.actor_id)
          -- W21: set to nothing, a conversation doesn't notify even of mentions.
          AND conversation_notification_level(recipient.actor_id, message.conversation_id)
              <> 'nothing'
        ORDER BY recipient.actor_id
        "#,
    )
    .bind(message_id)
    .bind(crate::message_content::EVERYONE_MENTION_PATTERN)
    .bind(EVERYONE_OPEN_LIMIT)
    .bind(crate::message_content::HERE_MENTION_PATTERN)
    .bind(crate::realtime::presence::FRESH_SECONDS as f64)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut notification = NewNotification::new(
                row.project_id,
                row.recipient_actor_id,
                kind::MENTION,
                "conversation",
                row.conversation_id,
                format!("mention:{}", row.conversation_id),
                format!("mention:{message_id}"),
            );
            notification.actor_id = Some(row.author_actor_id);
            notification.conversation_id = Some(row.conversation_id);
            notification.message_id = Some(message_id);
            notification
        })
        .collect())
}

/// People who set this message's conversation (or a thread's channel) to
/// every message, and can see it, other than its author and anyone it
/// mentions (they get the mention).
async fn followed_messages(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let rows = sqlx::query_as::<_, FollowedRow>(
        r#"
        WITH message AS (
            SELECT message.id, COALESCE(message.project_id, direct_scope_id()) AS project_id,
                   message.conversation_id,
                   message.author_actor_id, conversation.parent_channel_id
            FROM messages message
            JOIN conversations conversation ON conversation.id = message.conversation_id
            WHERE message.id = $1 AND message.removed_at IS NULL
              -- A DM or group tells its members as a direct message.
              AND conversation.kind NOT IN ('dm', 'group')
        ),
        following AS (
            SELECT level.actor_id
            FROM message
            JOIN conversation_notification_levels level
              ON level.level = 'all'
             AND level.conversation_id IN (message.conversation_id, message.parent_channel_id)
        )
        SELECT DISTINCT message.project_id, message.conversation_id, message.author_actor_id,
               following.actor_id AS recipient_actor_id,
               message.parent_channel_id IS NOT NULL AS in_thread
        FROM message
        JOIN following ON following.actor_id <> message.author_actor_id
        JOIN actors actor ON actor.id = following.actor_id AND actor.kind = 'human' -- // kind-check: mechanical (people are notified; agents are asked to reply by the reply rules)
        WHERE conversation_notification_level(following.actor_id, message.conversation_id) = 'all'
          AND (message.project_id = direct_scope_id()
               OR actor_can_access_project(following.actor_id, NULL, message.project_id))
          AND conversation_visible_to(message.conversation_id, following.actor_id)
          AND NOT EXISTS (
              SELECT 1 FROM message_mentions mention
              WHERE mention.message_id = message.id AND mention.actor_id = following.actor_id
          )
        ORDER BY recipient_actor_id
        "#,
    )
    .bind(message_id)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            // A followed thread's reply is a thread reply; a channel set to
            // every message keeps telling of each message (C2.5).
            let (kind, group) = if row.in_thread {
                (
                    kind::THREAD_REPLY,
                    format!("thread:{}", row.conversation_id),
                )
            } else {
                (
                    kind::CONVERSATION_MESSAGE,
                    format!("messages:{}", row.conversation_id),
                )
            };
            let mut notification = NewNotification::new(
                row.project_id,
                row.recipient_actor_id,
                kind,
                "conversation",
                row.conversation_id,
                group,
                format!("message:{message_id}"),
            );
            notification.actor_id = Some(row.author_actor_id);
            notification.conversation_id = Some(row.conversation_id);
            notification.message_id = Some(message_id);
            notification
        })
        .collect())
}

#[derive(Debug, FromRow)]
struct FollowedRow {
    project_id: Uuid,
    conversation_id: Uuid,
    author_actor_id: Uuid,
    recipient_actor_id: Uuid,
    in_thread: bool,
}

/// A message in a DM or group tells its other members (C2.5): one row per
/// conversation that counts up as messages arrive (`dm:<conversation>`),
/// and reading the DM reads it. A member who set it to nothing hears
/// nothing; one it mentions gets the mention instead.
async fn direct_messages(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let rows = sqlx::query_as::<_, MentionRow>(
        r#"
        WITH message AS (
            SELECT message.id, COALESCE(message.project_id, direct_scope_id()) AS project_id,
                   message.conversation_id,
                   message.author_actor_id
            FROM messages message
            JOIN conversations conversation ON conversation.id = message.conversation_id
            WHERE message.id = $1 AND message.removed_at IS NULL
              AND conversation.kind IN ('dm', 'group')
        )
        SELECT message.project_id, message.conversation_id, message.author_actor_id,
               member.actor_id AS recipient_actor_id
        FROM message
        JOIN conversation_members member
          ON member.conversation_id = message.conversation_id
         AND member.actor_id <> message.author_actor_id
        JOIN actors actor ON actor.id = member.actor_id AND actor.kind = 'human' -- // kind-check: mechanical (an agent in a DM is asked to reply, not told)
        WHERE conversation_notification_level(member.actor_id, message.conversation_id)
              <> 'nothing'
          AND (message.project_id = direct_scope_id()
               OR actor_can_access_project(member.actor_id, NULL, message.project_id))
          AND conversation_visible_to(message.conversation_id, member.actor_id)
          AND NOT EXISTS (
              SELECT 1 FROM message_mentions mention
              WHERE mention.message_id = message.id AND mention.actor_id = member.actor_id
          )
        ORDER BY recipient_actor_id
        "#,
    )
    .bind(message_id)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut notification = NewNotification::new(
                row.project_id,
                row.recipient_actor_id,
                kind::DIRECT_MESSAGE,
                "conversation",
                row.conversation_id,
                format!("dm:{}", row.conversation_id),
                format!("message:{message_id}"),
            );
            notification.actor_id = Some(row.author_actor_id);
            notification.conversation_id = Some(row.conversation_id);
            notification.message_id = Some(message_id);
            notification
        })
        .collect())
}

/// A message naming someone's keyword tells them (C2.13): only in a channel,
/// thread or thing's comments (a DM or group already tells its members),
/// only while they may read it and haven't muted it, never of their own
/// message, and not when it mentions them (they get the mention). A keyword
/// matches as a whole word or phrase, in any case. It's one row per
/// message, and one told of the message another way (a followed channel or
/// thread) isn't told twice: the occurrence is the message.
async fn keyword_messages(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let rows = sqlx::query_as::<_, MentionRow>(
        r#"
        WITH message AS (
            SELECT message.id, COALESCE(message.project_id, direct_scope_id()) AS project_id,
                   message.conversation_id,
                   message.author_actor_id, message.plain_text,
                   message.workspace_id
            FROM messages message
            JOIN conversations conversation ON conversation.id = message.conversation_id
            WHERE message.id = $1 AND message.removed_at IS NULL
              AND conversation.kind NOT IN ('dm', 'group')
              AND message.plain_text IS NOT NULL
        ),
        matched AS (
            SELECT DISTINCT keyword.account_id
            FROM message
            -- A thread in a personal DM or group (the direct scope) hears
            -- its members' keywords from any workspace.
            JOIN notification_keywords keyword
              ON keyword.workspace_id = message.workspace_id
              OR message.workspace_id = direct_scope_id()
            WHERE keyword.account_id <> message.author_actor_id
              AND message.plain_text ~* ('(^|[^[:alnum:]_])'
                  || regexp_replace(keyword.keyword, '([^[:alnum:][:space:]])', '\\\1', 'g')
                  || '($|[^[:alnum:]_])')
        )
        SELECT message.project_id, message.conversation_id, message.author_actor_id,
               matched.account_id AS recipient_actor_id
        FROM message
        JOIN matched ON true
        WHERE (message.project_id = direct_scope_id()
               OR actor_can_access_project(matched.account_id, NULL, message.project_id))
          AND conversation_visible_to(message.conversation_id, matched.account_id)
          AND conversation_notification_level(matched.account_id, message.conversation_id)
              <> 'nothing'
          AND NOT EXISTS (
              SELECT 1 FROM message_mentions mention
              WHERE mention.message_id = message.id AND mention.actor_id = matched.account_id
          )
        ORDER BY recipient_actor_id
        "#,
    )
    .bind(message_id)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut notification = NewNotification::new(
                row.project_id,
                row.recipient_actor_id,
                kind::KEYWORD,
                "conversation",
                row.conversation_id,
                format!("keyword:{message_id}"),
                format!("message:{message_id}"),
            );
            notification.actor_id = Some(row.author_actor_id);
            notification.conversation_id = Some(row.conversation_id);
            notification.message_id = Some(message_id);
            notification
        })
        .collect())
}

/// A comment on a thing tells its followers (C1.3): the thing's maker (a
/// task's assignee and watchers) and anyone who commented there before,
/// each only while they may see it. A mention is told as a mention instead,
/// and nobody is told of their own comment.
async fn thing_comments(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let Some((project_id, conversation_id, comments_id, author, subject_kind, subject_id, href)) =
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, String, Uuid, Option<String>)>(
            r#"
            -- visibility: a worker reading the message it was told of; recipients are
            -- filtered by conversation_visible_to below
            SELECT message.project_id, message.conversation_id,
                   COALESCE(parent.id, conversation.id), message.author_actor_id,
                   COALESCE(parent.subject_kind, conversation.subject_kind),
                   COALESCE(parent.subject_id, conversation.subject_id),
                   subject_href(COALESCE(parent.subject_kind, conversation.subject_kind),
                                COALESCE(parent.subject_id, conversation.subject_id))
            FROM messages message
            JOIN conversations conversation ON conversation.id = message.conversation_id
            LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
            WHERE message.id = $1 AND message.removed_at IS NULL
              AND COALESCE(parent.kind, conversation.kind) = 'comments'
            "#,
        )
        .bind(message_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(Vec::new());
    };
    let Some(thing_kind) = crate::discussion::SubjectKind::parse(&subject_kind) else {
        return Ok(Vec::new());
    };
    let followers = thing_kind
        .subject()
        .followers(transaction, subject_id)
        .await?;
    let recipients = sqlx::query_as::<_, (Uuid, bool)>(
        r#"
        WITH candidate AS (
            SELECT unnest($1::uuid[]) AS actor_id
            UNION
            -- visibility: authors only, of the discussion this message is in
            SELECT DISTINCT earlier.author_actor_id
            FROM messages earlier
            JOIN conversations place ON place.id = earlier.conversation_id
            WHERE (place.id = $2 OR place.parent_channel_id = $2)
              AND earlier.removed_at IS NULL AND earlier.id <> $4
        ),
        -- The author of the comment this one answers hears it as a reply.
        answered AS (
            -- visibility: the author of a comment in the same discussion
            SELECT answered.author_actor_id AS actor_id
            FROM messages comment
            JOIN messages answered ON answered.id = comment.reply_to_message_id
            WHERE comment.id = $4 AND answered.removed_at IS NULL
        )
        SELECT candidate.actor_id,
               candidate.actor_id IN (SELECT actor_id FROM answered) AS answered
        FROM candidate
        -- An inbox row is how people are told; agents are asked by mention.
        JOIN actors actor ON actor.id = candidate.actor_id AND actor.kind = 'human' -- // kind-check: mechanical (how a follower is told)
        WHERE candidate.actor_id <> $3
          AND actor_can_access_project(candidate.actor_id, NULL, $5)
          AND conversation_visible_to($6, candidate.actor_id)
          -- Muting the thing's comments, or the thread, quiets it.
          AND conversation_notification_level(candidate.actor_id, $6) <> 'nothing'
          AND NOT EXISTS (
              SELECT 1 FROM message_mentions mention
              WHERE mention.message_id = $4 AND mention.actor_id = candidate.actor_id
          )
        ORDER BY candidate.actor_id
        "#,
    )
    .bind(&followers)
    .bind(comments_id)
    .bind(author)
    .bind(message_id)
    .bind(project_id)
    .bind(conversation_id)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(recipients
        .into_iter()
        .map(|(recipient, answered)| {
            let mut notification = if answered {
                NewNotification::new(
                    project_id,
                    recipient,
                    kind::MENTION,
                    "conversation",
                    conversation_id,
                    format!("mention:{conversation_id}"),
                    format!("mention:{message_id}"),
                )
            } else {
                NewNotification::new(
                    project_id,
                    recipient,
                    kind::THING_COMMENT,
                    "conversation",
                    conversation_id,
                    // One per place: the stream, or each thread on its own.
                    format!("comments:{conversation_id}"),
                    format!("message:{message_id}"),
                )
            };
            notification.actor_id = Some(author);
            notification.conversation_id = Some(conversation_id);
            notification.message_id = Some(message_id);
            notification.detail = serde_json::json!({
                "subjectKind": thing_kind.as_str(),
                "subjectId": subject_id,
                "href": href,
                "reply": answered,
            });
            notification
        })
        .collect())
}

/// A removed message stops counting: its occurrences go, and a notification
/// left with none goes too. The text itself was never copied here.
async fn forget_message(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<(), AppError> {
    let affected = sqlx::query_scalar::<_, Uuid>(
        r#"
        DELETE FROM notification_occurrences
        WHERE message_id = $1
        RETURNING notification_id
        "#,
    )
    .bind(message_id)
    .fetch_all(&mut **transaction)
    .await?;
    if affected.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        DELETE FROM notifications n
        WHERE n.id = ANY($1)
          AND NOT EXISTS (
              SELECT 1 FROM notification_occurrences o WHERE o.notification_id = n.id
          )
        "#,
    )
    .bind(&affected)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE notifications n
        SET message_id = (
                SELECT o.message_id FROM notification_occurrences o
                WHERE o.notification_id = n.id AND o.message_id IS NOT NULL
                ORDER BY o.created_at DESC
                LIMIT 1
            ),
            repeat_count = GREATEST(1, (
                SELECT count(*) FROM notification_occurrences o WHERE o.notification_id = n.id
            )::integer),
            updated_at = now()
        WHERE n.id = ANY($1)
        "#,
    )
    .bind(&affected)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct AssignmentRow {
    activity_id: Uuid,
    actor_id: Uuid,
    assignee_actor_id: Uuid,
}

#[derive(FromRow)]
struct WatchedChangeRow {
    /// The activity this change is.
    change_id: Uuid,
    actor_id: Uuid,
    /// created, updated, archived or restored.
    change: String,
    recipient_actor_id: Uuid,
}

/// What a task change means for people. `work_item.updated` carries only the
/// task and its version, so this finds the activity rows written in the same
/// transaction as the event: they share its `created_at` (PostgreSQL's
/// `now()` is the transaction's start). Comments are messages (C1.4): their
/// mentions and followers are told as any comment's are (`thing_comments`).
async fn work_item_changes(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let mut notifications = Vec::new();
    let assignments = sqlx::query_as::<_, AssignmentRow>(
        r#"
        SELECT activity.id AS activity_id, activity.actor_id,
               item.assignee_actor_id
        FROM events event
        JOIN work_item_activity activity
          ON activity.project_id = event.space_id
         AND activity.work_item_id = $3
         AND activity.created_at = event.created_at
        JOIN task_rows item
          ON item.project_id = activity.project_id AND item.id = activity.work_item_id
        WHERE event.id = $1 AND event.space_id = $2
          AND (
              activity.kind = 'work_item_created'
              OR (activity.kind = 'work_item_updated'
                  AND activity.payload->'changedFields' ? 'assignee')
          )
          AND item.assignee_actor_id IS NOT NULL
          AND item.assignee_actor_id <> activity.actor_id
          AND item.archived_at IS NULL
          AND actor_can_access_project(item.assignee_actor_id, NULL, item.project_id)
        ORDER BY activity.id
        "#,
    )
    .bind(event_id)
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in assignments {
        let mut notification = NewNotification::new(
            project_id,
            row.assignee_actor_id,
            kind::ASSIGNED,
            "work_item",
            work_item_id,
            format!("assigned:{work_item_id}"),
            format!("assigned:{}", row.activity_id),
        );
        notification.actor_id = Some(row.actor_id);
        notification.work_item_id = Some(work_item_id);
        notifications.push(notification);
    }
    // Watchers hear about every other change, unless this event already
    // mentioned or assigned them.
    let changes = sqlx::query_as::<_, WatchedChangeRow>(
        r#"
        WITH changes AS (
            SELECT activity.id AS change_id, activity.actor_id,
                   replace(activity.kind, 'work_item_', '') AS change,
                   activity.project_id, activity.work_item_id
            FROM events event
            JOIN work_item_activity activity
              ON activity.project_id = event.space_id
             AND activity.work_item_id = $3
             AND activity.created_at = event.created_at
            WHERE event.id = $1 AND event.space_id = $2
        )
        SELECT change.change_id, change.actor_id, change.change,
               watcher.account_id AS recipient_actor_id
        FROM changes change
        JOIN follows watcher
          ON watcher.target_kind = 'task'
         AND watcher.target_id = change.work_item_id
         AND watcher.space_id = change.project_id
         -- A comment is the task's discussion: only for those who follow
         -- everything about it.
         AND (change.change <> 'commented' OR watcher.level = 'everything')
        WHERE watcher.account_id <> change.actor_id
          AND actor_can_access_project(watcher.account_id, NULL, change.project_id)
        ORDER BY change.change_id, watcher.account_id
        "#,
    )
    .bind(event_id)
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&mut **transaction)
    .await?;
    let told = notifications
        .iter()
        .map(|notification| notification.recipient_actor_id)
        .collect::<HashSet<_>>();
    for row in changes {
        if told.contains(&row.recipient_actor_id) {
            continue;
        }
        let mut notification = NewNotification::new(
            project_id,
            row.recipient_actor_id,
            kind::TASK_CHANGED,
            "work_item",
            work_item_id,
            format!("watch:{work_item_id}"),
            format!("watch:{}", row.change_id),
        );
        notification.actor_id = Some(row.actor_id);
        notification.work_item_id = Some(work_item_id);
        notification.detail = serde_json::json!({ "change": row.change });
        notifications.push(notification);
    }
    Ok(notifications)
}

async fn run_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    state: RunState,
) -> Result<Vec<NewNotification>, AppError> {
    let kind = match state {
        RunState::Blocked => kind::RUN_WAITING,
        RunState::Completed => kind::RUN_COMPLETED,
        RunState::Failed | RunState::Interrupted => kind::RUN_FAILED,
        _ => return Ok(Vec::new()),
    };
    let Some(run) = crate::runs::reads::notify_context(transaction, run_id).await? else {
        return Ok(Vec::new());
    };
    let group_key = format!("run:{run_id}");
    let base = |recipient: Uuid, kind: &'static str, dedupe_key: String| {
        let mut notification = NewNotification::new(
            run.project_id,
            recipient,
            kind,
            "agent_run",
            run_id,
            group_key.clone(),
            dedupe_key,
        );
        notification.conversation_id = Some(run.conversation_id);
        notification.agent_run_id = Some(run_id);
        notification
    };
    let mut notifications = Vec::new();
    // While it waits, the newest thing it asked for; whoever may answer that
    // was told when it was raised (`requests::announce_raised`).
    let waiting_on = if state == RunState::Blocked {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM requests WHERE run_id = $1 AND status = 'open' ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(run_id)
        .fetch_optional(&mut **transaction)
        .await?
    } else {
        None
    };
    let told_already = match waiting_on {
        Some(request_id) => {
            sqlx::query_scalar::<_, bool>(
                r#"
            SELECT EXISTS (
                SELECT 1 FROM notifications
                WHERE subject_kind = 'request' AND subject_id = $1 AND recipient_actor_id = $2
            )
            "#,
            )
            .bind(request_id)
            .bind(run.requesting_actor_id)
            .fetch_one(&mut **transaction)
            .await?
        }
        None => false,
    };
    if run.requester_reachable && !told_already {
        let dedupe_key = match state {
            // Each wait is its own occurrence; the request says which.
            RunState::Blocked => match waiting_on {
                Some(request_id) => format!("run-waiting:{request_id}"),
                None => return Ok(notifications),
            },
            _ => format!("run-ended:{run_id}"),
        };
        notifications.push(base(run.requesting_actor_id, kind, dedupe_key));
    }
    Ok(notifications)
}

enum PullRequestSource {
    Run(Uuid),
    WorkItem(Uuid),
}

#[derive(FromRow)]
struct PullRequestRow {
    id: Uuid,
    head_sha: String,
    checks_state: String,
    owner_actor_id: Option<Uuid>,
}

#[derive(FromRow)]
struct ReviewerRow {
    pull_request_id: Uuid,
    actor_id: Uuid,
}

/// Failing checks go to whoever the pull request is working for (the run's
/// requester, or the task's assignee); review requests go to the people
/// whose GitHub username was asked. Each recipient must be able to see the
/// pull request.
async fn pull_request_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    source: PullRequestSource,
) -> Result<Vec<NewNotification>, AppError> {
    let rows = match source {
        PullRequestSource::Run(run_id) => {
            let Some(owner) = crate::runs::reads::requester(&mut **transaction, run_id).await?
            else {
                return Ok(Vec::new());
            };
            sqlx::query_as::<_, PullRequestRow>(
                r#"
                SELECT DISTINCT pr.id, pr.head_sha, pr.checks_state,
                       $3::uuid AS owner_actor_id
                FROM github_pull_request_workspace_links link
                JOIN github_pull_requests pr
                  ON pr.project_id = link.project_id AND pr.id = link.github_pull_request_id
                WHERE link.project_id = $1 AND link.agent_run_id = $2 AND pr.state = 'open'
                "#,
            )
            .bind(project_id)
            .bind(run_id)
            .bind(owner)
            .fetch_all(&mut **transaction)
            .await?
        }
        PullRequestSource::WorkItem(work_item_id) => {
            sqlx::query_as::<_, PullRequestRow>(
                r#"
                SELECT DISTINCT pr.id, pr.head_sha, pr.checks_state,
                       item.assignee_actor_id AS owner_actor_id
                FROM github_pull_request_work_item_links link
                JOIN github_pull_requests pr
                  ON pr.project_id = link.project_id AND pr.id = link.github_pull_request_id
                JOIN task_rows item
                  ON item.project_id = link.project_id AND item.id = link.work_item_id
                WHERE link.project_id = $1 AND link.work_item_id = $2 AND pr.state = 'open'
                "#,
            )
            .bind(project_id)
            .bind(work_item_id)
            .fetch_all(&mut **transaction)
            .await?
        }
    };
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut notifications = Vec::new();
    for row in &rows {
        let Some(owner) = row.owner_actor_id else {
            continue;
        };
        if row.checks_state != "failure" {
            continue;
        }
        let reachable = sqlx::query_scalar::<_, bool>(
            "SELECT actor_can_access_project($1, NULL, $2) AND pull_request_visible_to($3, $1)",
        )
        .bind(owner)
        .bind(project_id)
        .bind(row.id)
        .fetch_one(&mut **transaction)
        .await?;
        if !reachable {
            continue;
        }
        let mut notification = NewNotification::new(
            project_id,
            owner,
            kind::CHECKS_FAILED,
            "pull_request",
            row.id,
            format!("checks:{}", row.id),
            format!("checks-failed:{}:{}", row.id, row.head_sha),
        );
        notification.pull_request_id = Some(row.id);
        notifications.push(notification);
    }
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let reviewers = sqlx::query_as::<_, ReviewerRow>(
        r#"
        SELECT DISTINCT pr.id AS pull_request_id, membership.actor_id
        FROM github_pull_requests pr
        CROSS JOIN LATERAL jsonb_array_elements_text(pr.requested_reviewers) AS requested(login)
        JOIN users person ON lower(person.github_login) = lower(requested.login)
        JOIN project_memberships membership
          ON membership.project_id = pr.project_id
         AND membership.actor_id = person.actor_id
         AND membership.enabled
        WHERE pr.id = ANY($1) AND pr.project_id = $2
          AND actor_can_access_project(membership.actor_id, NULL, pr.project_id)
          AND pull_request_visible_to(pr.id, membership.actor_id)
        ORDER BY pr.id, membership.actor_id
        "#,
    )
    .bind(&ids)
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in reviewers {
        let mut notification = NewNotification::new(
            project_id,
            row.actor_id,
            kind::REVIEW_REQUESTED,
            "pull_request",
            row.pull_request_id,
            format!("review:{}", row.pull_request_id),
            format!("review-requested:{}", row.pull_request_id),
        );
        notification.pull_request_id = Some(row.pull_request_id);
        notifications.push(notification);
    }
    Ok(notifications)
}

// ---------------------------------------------------------------------------
// Reading

/// The checks every read applies, for viewer `$1`.
const VISIBLE: &str = r#"
    -- A DM's notification (no space) by the visibility rule alone.
    (CASE WHEN n.project_id IS NULL THEN n.conversation_id IS NOT NULL
          ELSE actor_can_access_project($1, NULL, n.project_id) END)
    AND (n.conversation_id IS NULL OR conversation_visible_to(n.conversation_id, $1))
    AND (n.pull_request_id IS NULL OR pull_request_visible_to(n.pull_request_id, $1))
    -- A task's or pull request's notification only while the recipient's
    -- role there still lets them see tasks, or code. An app's agent limit
    -- is applied to the listed rows (`hidden_by_limit`).
    AND (n.work_item_id IS NULL OR EXISTS (
        SELECT 1 FROM project_memberships sees_task
        WHERE sees_task.project_id = n.project_id AND sees_task.actor_id = $1
          AND sees_task.enabled AND role_allows(sees_task.role, sees_task.role_id, 'tasks.view')
    ))
    AND (n.pull_request_id IS NULL OR EXISTS (
        SELECT 1 FROM project_memberships sees_code
        WHERE sees_code.project_id = n.project_id AND sees_code.actor_id = $1
          AND sees_code.enabled AND role_allows(sees_code.role, sees_code.role_id, 'code.view')
    ))
    AND (n.subject_kind <> 'provider_account' OR EXISTS (
        SELECT 1 FROM provider_accounts account
        WHERE account.id = n.subject_id AND account.owner_actor_id = $1
    ))
    -- A page's notification only while the page is still in the project.
    AND (n.subject_kind <> 'page' OR EXISTS (
        SELECT 1 FROM project_pages subject_page
        WHERE subject_page.id = n.subject_id AND subject_page.project_id = n.project_id
    ))
"#;

#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub view: View,
    pub category: Category,
    pub project_id: Option<Uuid>,
    /// Only these projects (MCP connections confined to some), when set.
    pub within: Option<Vec<Uuid>>,
    pub cursor: Option<String>,
    pub limit: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationView {
    pub id: Uuid,
    pub kind: String,
    pub project_id: Uuid,
    pub project_name: String,
    /// What the notification is about, for grouping: a conversation, task,
    /// run or pull request.
    pub subject: SubjectView,
    /// One line saying what happened.
    pub title: String,
    /// Up to 280 characters of the message, comment, question or failure,
    /// when there is one. Read live: a removed message shows nothing.
    pub preview: Option<String>,
    pub actor: Option<NotificationActor>,
    /// How many occurrences collapsed into this one.
    pub count: i32,
    pub read: bool,
    pub archived: bool,
    pub snoozed_until: Option<String>,
    /// Snoozed until something new happens on it (C2.10).
    pub until_activity: bool,
    /// When it last came to the top of the inbox.
    pub at: String,
    pub created_at: String,
    /// Where to open it in the web app (an https URL for a pull request).
    pub href: String,
    /// Where its Launch action goes, for notifications that have one (a
    /// capacity alert opens the account's ready tasks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_href: Option<String>,
    /// A capacity alert's suggested batch, which Launch starts in one tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchAction>,
    /// Set when the notification is about a request (A1.8): what it asks,
    /// its options, and its answer once given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<crate::requests::RequestCard>,
    /// Whether it asks something of you or is for your information.
    pub attention: Attention,
    /// The conversation a Reply from the notification posts in (mentions,
    /// DMs and thread replies), for replying in place (C2.10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_conversation_id: Option<Uuid>,
    /// A reminder's saved item, which Done and Snooze change (W21).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_item_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAction {
    pub account_id: Uuid,
    /// The workspace the alert (and its provider account) is in, which the
    /// launch goes to: the inbox lists every workspace's alerts.
    pub workspace_id: Uuid,
    /// How many tasks the batch held when the alert fired.
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectView {
    pub kind: String,
    pub id: Uuid,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationActor {
    pub id: Uuid,
    pub name: String,
    /// The actor's kind (human, external_agent, execution_worker, ...), so
    /// an agent reads as an agent.
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPage {
    pub notifications: Vec<NotificationView>,
    /// Pass back as `cursor` for the next page; absent on the last.
    pub next_cursor: Option<String>,
    /// Unread notifications in the inbox, across every project you can see.
    pub unread_count: i64,
}

#[derive(FromRow)]
struct NotificationRow {
    id: Uuid,
    kind: String,
    subject_kind: String,
    subject_id: Uuid,
    project_id: Uuid,
    workspace_id: Uuid,
    workspace_slug: Option<String>,
    project_name: String,
    direct_space: bool,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    actor_kind: Option<String>,
    repeat_count: i32,
    read: bool,
    archived: bool,
    snoozed_until: Option<String>,
    until_activity: bool,
    at: String,
    at_micros: i64,
    created_at: String,
    detail: serde_json::Value,
    conversation_id: Option<Uuid>,
    conversation_kind: Option<String>,
    conversation_name: Option<String>,
    parent_conversation_id: Option<Uuid>,
    /// Where a comment's thing opens, for a comments conversation (C1.3).
    subject_href: Option<String>,
    /// What a comment is on, for a comments conversation: a task's key or a
    /// thing's title.
    subject_label: Option<String>,
    message_id: Option<Uuid>,
    message_preview: Option<String>,
    page_title: Option<String>,
    page_archived: Option<bool>,
    work_item_id: Option<Uuid>,
    work_item_key: Option<String>,
    work_item_title: Option<String>,
    run_failure: Option<String>,
    run_state: Option<String>,
    run_agent_name: Option<String>,
    run_waiting_on: Option<String>,
    pull_request_number: Option<i64>,
    pull_request_title: Option<String>,
    pull_request_url: Option<String>,
    request_kind: Option<String>,
    request_status: Option<String>,
    request_title: Option<String>,
    request_options: Option<serde_json::Value>,
    request_allow_text: Option<bool>,
    request_answer: Option<serde_json::Value>,
    request_answered_at: Option<String>,
}

fn list_sql(view_condition: &str, extra: &str) -> String {
    // AWD-75: nothing from a module that's switched off.
    let switches = crate::modules::state::notification_switches_sql();
    let run_join = crate::runs::sql::builtin_run_and_worker_join("n.agent_run_id");
    format!(
        r#"
        SELECT n.id, n.kind, n.subject_kind, n.subject_id,
               COALESCE(n.project_id, direct_scope_id()) AS project_id,
               COALESCE(project.organization_id, conversation.workspace_id, direct_scope_id())
                   AS workspace_id,
               workspace.slug AS workspace_slug,
               COALESCE(project.name, 'Messages') AS project_name,
               n.project_id IS NULL AS direct_space,
               n.actor_id, actor_member.display_name AS actor_name,
               actor.kind AS actor_kind,
               n.repeat_count, n.read_at IS NOT NULL AS read,
               n.archived_at IS NOT NULL AS archived,
               CASE WHEN n.snoozed_until > now() AND NOT n.wake_on_activity THEN
                   to_char(n.snoozed_until AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
               END AS snoozed_until,
               n.snoozed_until > now() AND n.wake_on_activity AS until_activity,
               to_char(n.surfaced_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS at,
               (extract(epoch FROM n.surfaced_at) * 1000000)::bigint AS at_micros,
               to_char(n.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                   AS created_at,
               n.detail,
               n.conversation_id, conversation.kind AS conversation_kind,
               conversation.name AS conversation_name,
               conversation.parent_channel_id AS parent_conversation_id,
               CASE WHEN COALESCE(comments_parent.kind, conversation.kind) = 'comments'
                    THEN subject_href(
                        COALESCE(comments_parent.subject_kind, conversation.subject_kind),
                        COALESCE(comments_parent.subject_id, conversation.subject_id))
               END AS subject_href,
               -- What a comment is on, named live (C1.4): a task by its key,
               -- anything else by its title.
               CASE WHEN COALESCE(comments_parent.kind, conversation.kind) = 'comments' THEN
                   CASE COALESCE(comments_parent.subject_kind, conversation.subject_kind)
                       WHEN 'task' THEN (
                           SELECT subject_space.project_key || '-' || subject_task.number::text
                           FROM task_rows subject_task
                           JOIN projects subject_space ON subject_space.id = subject_task.project_id
                           WHERE subject_task.id = COALESCE(comments_parent.subject_id, conversation.subject_id))
                       WHEN 'page' THEN (
                           SELECT title FROM project_pages
                           WHERE id = COALESCE(comments_parent.subject_id, conversation.subject_id))
                       WHEN 'whiteboard' THEN (
                           SELECT title FROM whiteboards
                           WHERE id = COALESCE(comments_parent.subject_id, conversation.subject_id))
                       WHEN 'sheet' THEN (
                           SELECT name FROM sheets
                           WHERE id = COALESCE(comments_parent.subject_id, conversation.subject_id))
                       WHEN 'record' THEN (
                           SELECT title FROM collection_records
                           WHERE id = COALESCE(comments_parent.subject_id, conversation.subject_id))
                   END
               END AS subject_label,
               n.message_id,
               CASE WHEN message.removed_at IS NULL
                    THEN left(message.plain_text, {PREVIEW_CHARACTERS}) END AS message_preview,
               page.title AS page_title, page.archived_at IS NOT NULL AS page_archived,
               n.work_item_id,
               project.project_key || '-' || item.number::text AS work_item_key,
               item.title AS work_item_title,
               left(run.failure_reason, {PREVIEW_CHARACTERS}) AS run_failure,
               run.state AS run_state, run_agent.display_name AS run_agent_name,
               (SELECT left(waiting.title, {PREVIEW_CHARACTERS}) FROM requests waiting
                 WHERE n.kind = 'run_waiting' AND waiting.run_id = n.agent_run_id
                   AND waiting.status = 'open'
                   AND request_visible_to(waiting.id, n.recipient_actor_id)
                 ORDER BY waiting.created_at DESC LIMIT 1) AS run_waiting_on,
               pr.pull_request_number, pr.title AS pull_request_title,
               pr.html_url AS pull_request_url,
               request.kind AS request_kind, request.status AS request_status,
               request.title AS request_title, request.options AS request_options,
               request.allow_text AS request_allow_text, request.answer AS request_answer,
               to_char(request.answered_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                   AS request_answered_at
        FROM notifications n
        LEFT JOIN projects project ON project.id = n.project_id
        LEFT JOIN LATERAL place_profile(n.project_id, n.actor_id) actor_member
          ON n.actor_id IS NOT NULL
        LEFT JOIN actors actor ON actor.id = n.actor_id
        LEFT JOIN conversations conversation ON conversation.id = n.conversation_id
        LEFT JOIN organizations workspace
          ON workspace.id = COALESCE(project.organization_id, conversation.workspace_id)
        LEFT JOIN conversations comments_parent
          ON comments_parent.id = conversation.parent_channel_id
        LEFT JOIN messages message ON message.id = n.message_id
        LEFT JOIN project_pages page ON n.subject_kind = 'page' AND page.id = n.subject_id
        LEFT JOIN task_rows item ON item.id = n.work_item_id
        {run_join}
        LEFT JOIN project_memberships run_agent
          ON run_agent.project_id = n.project_id AND run_agent.actor_id = run_worker.actor_id
        LEFT JOIN github_pull_requests pr ON pr.id = n.pull_request_id
        LEFT JOIN requests request
          ON n.subject_kind = 'request' AND request.id = n.subject_id
         AND request_visible_to(request.id, n.recipient_actor_id)
        WHERE n.recipient_actor_id = $1
          AND {view_condition}
          AND ($2::text[] IS NULL OR n.kind = ANY($2))
          AND ($3::uuid IS NULL OR COALESCE(n.project_id, direct_scope_id()) = $3)
          AND ($4::uuid[] IS NULL OR COALESCE(n.project_id, direct_scope_id()) = ANY($4))
          AND {VISIBLE}
          AND {switches}
          {extra}
        "#
    )
}

/// One page of `viewer`'s notifications, newest first.
pub async fn list(
    pool: &PgPool,
    addresses: Addresses,
    viewer: Uuid,
    filter: &ListFilter,
) -> Result<NotificationPage, AppError> {
    let limit = filter.limit.clamp(1, MAX_PAGE_SIZE);
    let cursor = filter.cursor.as_deref().map(parse_cursor).transpose()?;
    let kinds = filter.category.kinds().map(|kinds| {
        kinds
            .iter()
            .map(|kind| (*kind).to_owned())
            .collect::<Vec<_>>()
    });
    let sql = list_sql(
        filter.view.condition(),
        r#"
          AND ($5::bigint IS NULL
               OR ((extract(epoch FROM n.surfaced_at) * 1000000)::bigint, n.id) < ($5, $6))
          -- A DM's or group's follows a credential's limit by the
          -- workspace it is kept in.
          AND ($8::uuid[] IS NULL OR n.project_id IS NOT NULL
               OR COALESCE(conversation.workspace_id, n.workspace_id) = ANY($8))
        ORDER BY n.surfaced_at DESC, n.id DESC
        LIMIT $7
        "#,
    );
    let mut rows = sqlx::query_as::<_, NotificationRow>(&sql)
        .bind(viewer)
        .bind(kinds)
        .bind(filter.project_id)
        .bind(filter.within.as_deref())
        .bind(cursor.map(|(micros, _)| micros))
        .bind(cursor.map(|(_, id)| id))
        .bind(limit + 1)
        .bind(crate::policy::agent_limit_direct_workspaces(
            viewer,
            crate::policy::Permission::ProjectsView,
        ))
        .fetch_all(crate::db::scoped(pool))
        .await?;
    let next_cursor = if rows.len() > usize::try_from(limit).unwrap_or(usize::MAX) {
        rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        rows.last()
            .map(|row| format!("{}.{}", row.at_micros, row.id))
    } else {
        None
    };
    let unread_count = unread_count(pool, viewer, filter.within.as_deref()).await?;
    Ok(NotificationPage {
        notifications: rows
            .into_iter()
            .filter(|row| !hidden_by_limit(viewer, row))
            .map(|row| view(addresses, row))
            .collect(),
        next_cursor,
        unread_count,
    })
}

/// A task's or pull request's notification an app acting for `viewer` may
/// not see: its agent limit leaves out "see tasks", or "see code", there.
fn hidden_by_limit(viewer: Uuid, row: &NotificationRow) -> bool {
    use crate::policy::Permission;
    (row.work_item_id.is_some()
        && crate::access::limit_hides(viewer, row.project_id, Permission::TasksView))
        || (row.pull_request_number.is_some()
            && crate::access::limit_hides(viewer, row.project_id, Permission::CodeView))
}

/// The given notifications, if they are the viewer's and still visible.
pub async fn get_many(
    pool: &PgPool,
    addresses: Addresses,
    viewer: Uuid,
    ids: &[Uuid],
) -> Result<Vec<NotificationView>, AppError> {
    // Any view: the IDs decide.
    let sql = list_sql(
        "true",
        "AND n.id = ANY($5) ORDER BY n.surfaced_at DESC, n.id DESC",
    );
    let rows = sqlx::query_as::<_, NotificationRow>(&sql)
        .bind(viewer)
        .bind(None::<Vec<String>>)
        .bind(None::<Uuid>)
        .bind(None::<Vec<Uuid>>)
        .bind(ids)
        .fetch_all(crate::db::scoped(pool))
        .await?;
    Ok(rows
        .into_iter()
        .filter(|row| !hidden_by_limit(viewer, row))
        .map(|row| view(addresses, row))
        .collect())
}

fn parse_cursor(value: &str) -> Result<(i64, Uuid), AppError> {
    let (micros, id) = value
        .split_once('.')
        .ok_or(AppError::Validation("cursor"))?;
    let micros = micros
        .parse::<i64>()
        .map_err(|_| AppError::Validation("cursor"))?;
    let id = id
        .parse::<Uuid>()
        .map_err(|_| AppError::Validation("cursor"))?;
    Ok((micros, id))
}

/// Unread notifications in `viewer`'s inbox, across what they can see.
pub async fn unread_count(
    pool: &PgPool,
    viewer: Uuid,
    within: Option<&[Uuid]>,
) -> Result<i64, AppError> {
    // AWD-75: nothing from a module that's switched off.
    let switches = crate::modules::state::notification_switches_sql();
    Ok(sqlx::query_scalar::<_, i64>(&format!(
        r#"
        SELECT count(*) FROM notifications n
        WHERE n.recipient_actor_id = $1
          AND {view}
          AND ($2::uuid[] IS NULL OR COALESCE(n.project_id, direct_scope_id()) = ANY($2))
          AND ($3::uuid[] IS NULL OR n.project_id IS NOT NULL
               -- visibility: only the DM's workspace is read, to apply a credential's limit
               OR COALESCE((SELECT kept.workspace_id FROM conversations kept
                            WHERE kept.id = n.conversation_id), n.workspace_id) = ANY($3))
          AND {VISIBLE}
          AND {switches}
        "#,
        view = View::Unread.condition(),
    ))
    .bind(viewer)
    .bind(within)
    .bind(crate::policy::agent_limit_direct_workspaces(
        viewer,
        crate::policy::Permission::ProjectsView,
    ))
    .fetch_one(crate::db::scoped(pool))
    .await?)
}

/// What the badges show (C2.4): the places that need you (a conversation,
/// task, pull request or other subject with an unread needs-you
/// notification, each counted once), and how many unread notifications are
/// only for your information.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attentions {
    pub unread_count: i64,
    pub needs_you_conversations: i64,
    pub fyi_unread: i64,
}

pub async fn attentions(pool: &PgPool, viewer: Uuid) -> Result<Attentions, AppError> {
    // AWD-75: nothing from a module that's switched off.
    let switches = crate::modules::state::notification_switches_sql();
    let (unread_count, needs_you_conversations, fyi_unread) =
        sqlx::query_as::<_, (i64, i64, i64)>(&format!(
            r#"
            SELECT count(*),
                   count(DISTINCT COALESCE(n.conversation_id, n.work_item_id,
                                           n.pull_request_id, n.subject_id))
                       FILTER (WHERE {needs}),
                   count(*) FILTER (WHERE NOT {needs})
            FROM notifications n
            WHERE n.recipient_actor_id = $1
              AND {view}
              AND {VISIBLE}
              AND {switches}
            "#,
            needs = needs_you_sql("$2"),
            view = View::Unread.condition(),
        ))
        .bind(viewer)
        .bind(kind::NEEDS_YOU)
        .fetch_one(crate::db::scoped(pool))
        .await?;
    Ok(Attentions {
        unread_count,
        needs_you_conversations,
        fyi_unread,
    })
}

/// Unread notifications per project: the project's attention count.
pub async fn attention_counts(
    executor: impl sqlx::PgExecutor<'_>,
    viewer: Uuid,
) -> Result<HashMap<Uuid, u32>, AppError> {
    // AWD-75: nothing from a module that's switched off.
    let switches = crate::modules::state::notification_switches_sql();
    let rows = sqlx::query_as::<_, (Uuid, i64)>(&format!(
        r#"
        SELECT COALESCE(n.project_id, direct_scope_id()), count(*) FROM notifications n
        WHERE n.recipient_actor_id = $1
          AND {view}
          AND {VISIBLE}
          AND {switches}
        GROUP BY 1
        "#,
        view = View::Unread.condition(),
    ))
    .bind(viewer)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(project_id, count)| (project_id, u32::try_from(count).unwrap_or(u32::MAX)))
        .collect())
}

fn conversation_label(row: &NotificationRow) -> String {
    if let Some(label) = row
        .subject_label
        .as_deref()
        .filter(|label| !label.is_empty())
    {
        return label.to_owned();
    }
    match (
        row.conversation_kind.as_deref(),
        row.conversation_name.as_deref(),
    ) {
        (Some("dm"), _) => "a DM".to_owned(),
        (Some("group"), Some(name)) if !name.is_empty() => name.to_owned(),
        (Some("group"), _) => "a group".to_owned(),
        (Some("channel"), Some(name)) => format!("#{name}"),
        (_, Some(name)) if !name.is_empty() => name.to_owned(),
        _ => "a conversation".to_owned(),
    }
}

/// Where the notification's conversation opens, in today's shape.
fn conversation_href(row: &NotificationRow) -> Option<String> {
    let conversation_id = row.conversation_id?;
    // A comment opens on its thing (C1.3).
    if let Some(href) = &row.subject_href {
        return Some(href.clone());
    }
    Some(
        crate::later::message_target(
            row.project_id,
            row.direct_space,
            conversation_id,
            Some(row.conversation_kind.as_deref().unwrap_or("channel")),
            row.parent_conversation_id,
        )
        .legacy_path(),
    )
}

/// "Muse's run", or "Your run" when the agent has no name here.
fn run_owner(row: &NotificationRow) -> String {
    row.run_agent_name
        .as_ref()
        .map_or_else(|| "Your run".to_owned(), |agent| format!("{agent}'s run"))
}

fn view(addresses: Addresses, row: NotificationRow) -> NotificationView {
    let who = row
        .actor_name
        .clone()
        .unwrap_or_else(|| "Someone".to_owned());
    let place = conversation_label(&row);
    let task = match (&row.work_item_key, &row.work_item_title) {
        (Some(key), Some(title)) => format!("{key} {title}"),
        (Some(key), None) => key.clone(),
        _ => "a task".to_owned(),
    };
    let task_key = row
        .work_item_key
        .clone()
        .unwrap_or_else(|| "a task".to_owned());
    let pull_request = row.pull_request_number.map_or_else(
        || "a pull request".to_owned(),
        |number| format!("PR #{number}"),
    );
    let (title, preview) = match row.kind.as_str() {
        kind::MENTION
            if row.subject_label.is_some()
                && row.detail.get("reply").and_then(serde_json::Value::as_bool) == Some(true) =>
        {
            (
                format!("{who} replied to you on {place}"),
                row.message_preview.clone(),
            )
        }
        kind::MENTION if row.subject_label.is_some() => (
            format!("{who} mentioned you on {place}"),
            row.message_preview.clone(),
        ),
        kind::MENTION => (
            format!("{who} mentioned you in {place}"),
            row.message_preview.clone(),
        ),
        kind::CONVERSATION_MESSAGE => (format!("{who} in {place}"), row.message_preview.clone()),
        kind::THREAD_REPLY => (
            format!("{who} replied in {place}"),
            row.message_preview.clone(),
        ),
        kind::DIRECT_MESSAGE => (format!("{who} messaged you"), row.message_preview.clone()),
        kind::KEYWORD => (
            format!("{who} used one of your words in {place}"),
            row.message_preview.clone(),
        ),
        kind::THING_COMMENT if row.subject_label.is_some() => (
            format!("{who} commented on {place}"),
            row.message_preview.clone(),
        ),
        kind::THING_COMMENT => (format!("{who} commented"), row.message_preview.clone()),
        kind::REMINDER => {
            let note = row
                .detail
                .get("note")
                .and_then(serde_json::Value::as_str)
                .filter(|note| !note.trim().is_empty());
            let title = match (note, row.message_id, &row.work_item_key) {
                (Some(note), _, _) => format!("Reminder: {note}"),
                (None, Some(_), _) => format!("Reminder: a message in {place}"),
                (None, None, Some(_)) => format!("Reminder: {task}"),
                _ => "Reminder".to_owned(),
            };
            let preview = if note.is_some() || row.message_id.is_some() {
                row.message_preview
                    .clone()
                    .or_else(|| row.work_item_title.clone())
            } else {
                row.work_item_title.clone()
            };
            (title, preview)
        }
        kind::COMMENT_MENTION => (
            if row.detail.get("reply").and_then(serde_json::Value::as_bool) == Some(true) {
                format!("{who} replied to you on {task_key}")
            } else {
                format!("{who} mentioned you on {task_key}")
            },
            row.message_preview.clone(),
        ),
        kind::ASSIGNED => (
            format!("{who} assigned you {task_key}"),
            row.work_item_title.clone(),
        ),
        kind::TASK_CHANGED => (
            match row.detail.get("change").and_then(serde_json::Value::as_str) {
                Some("commented") => format!("{who} commented on {task_key}"),
                Some("archived") => format!("{who} archived {task_key}"),
                Some("restored") => format!("{who} restored {task_key}"),
                Some("created") => format!("{who} created {task_key}"),
                _ => format!("{who} updated {task_key}"),
            },
            row.message_preview
                .clone()
                .or_else(|| row.work_item_title.clone()),
        ),
        kind::RUN_WAITING => (
            row.run_agent_name.as_ref().map_or_else(
                || format!("Your run in {place} is waiting on you"),
                |agent| format!("{agent} is waiting on you in {place}"),
            ),
            row.run_waiting_on.clone(),
        ),
        kind::REQUEST_RAISED => (
            {
                let what = match row.request_kind.as_deref() {
                    Some("approval") => "needs your approval",
                    Some("proposal") => "has a proposal for you",
                    _ => "has a question for you",
                };
                match (&row.conversation_id, &row.work_item_key) {
                    (Some(_), _) => format!("{who} {what} in {place}"),
                    (None, Some(key)) => format!("{who} {what} on {key}"),
                    _ => format!("{who} {what}"),
                }
            },
            row.request_title
                .as_deref()
                .map(|text| text.chars().take(PREVIEW_CHARACTERS as usize).collect()),
        ),
        kind::REQUEST_ANSWERED => (
            match row.request_status.as_deref() {
                Some("expired") => format!("Nobody answered your request in {place} in time"),
                _ => format!("{who} answered your request in {place}"),
            },
            row.request_title
                .as_deref()
                .map(|text| text.chars().take(PREVIEW_CHARACTERS as usize).collect()),
        ),
        kind::RUN_COMPLETED => (format!("{} in {place} is done", run_owner(&row)), None),
        kind::RUN_FAILED => (
            // Failures and interruptions both land here; say which.
            match row.run_state.as_deref() {
                Some("interrupted") => format!("{} in {place} was interrupted", run_owner(&row)),
                Some("canceled") => format!("{} in {place} was stopped", run_owner(&row)),
                _ => format!("{} in {place} failed", run_owner(&row)),
            },
            row.run_failure.clone(),
        ),
        kind::CHECKS_FAILED => (
            format!("Checks are failing on {pull_request}"),
            row.pull_request_title.clone(),
        ),
        kind::REVIEW_REQUESTED => (
            format!("Your review was requested on {pull_request}"),
            row.pull_request_title.clone(),
        ),
        // The page's title and the comment's words, read live (P0.36): a
        // removed comment shows nothing, an archived page says so and
        // shows nothing.
        kind::PAGE_COMMENT | kind::PAGE_SUGGESTION => {
            let archived = row.page_archived == Some(true);
            let page = match (&row.page_title, archived) {
                (Some(title), false) => title.clone(),
                (Some(title), true) => format!("{title} (archived)"),
                (None, _) => "a page".to_owned(),
            };
            if row.kind == kind::PAGE_COMMENT {
                (
                    format!("New comment on {page}"),
                    row.message_preview.clone().filter(|_| !archived),
                )
            } else {
                (format!("Suggested edits to {page}"), None)
            }
        }
        // A2.14: its words come from the budget's numbers (rule 10).
        kind::BUDGET_WARNING => (
            if row.detail.get("level").and_then(serde_json::Value::as_str) == Some("hard") {
                "A budget you set is used up"
            } else {
                "A budget you set is nearly used"
            }
            .to_owned(),
            match (
                row.detail.get("used").and_then(serde_json::Value::as_i64),
                row.detail.get("limit").and_then(serde_json::Value::as_i64),
                row.detail
                    .get("meter")
                    .and_then(serde_json::Value::as_str)
                    .and_then(crate::metering::find_meter),
            ) {
                (Some(used), Some(limit), Some(meter)) => {
                    Some(format!("{used} of {limit} for {}.", meter.name))
                }
                _ => None,
            },
        ),
        // Its words come from the limit it hit, not stored text (rule 10).
        kind::FORM_PAUSED => (
            "A form you made paused itself".to_owned(),
            row.detail
                .get("dailyLimit")
                .and_then(serde_json::Value::as_i64)
                .map(|limit| {
                    format!(
                        "It took {limit} answers today, its daily limit. Turn it on again to take more."
                    )
                }),
        ),
        other => (
            row.detail
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| other.replace('_', " "), str::to_owned),
            row.detail
                .get("preview")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        ),
    };
    let subject_title = match row.subject_kind.as_str() {
        // A DM has no name of its own: it goes by the person it's with, who
        // is whoever wrote to you there.
        "conversation"
            if row.conversation_kind.as_deref() == Some("dm")
                && row.subject_label.is_none()
                && row.actor_name.is_some() =>
        {
            who.clone()
        }
        "conversation" | "agent_run" => place.clone(),
        "work_item" => task,
        "pull_request" => match &row.pull_request_title {
            Some(title) => format!("{pull_request} {title}"),
            None => pull_request.clone(),
        },
        _ => row.project_name.clone(),
    };
    let href = match row.subject_kind.as_str() {
        "work_item" => row
            .work_item_id
            .map(|id| Target::task(row.project_id, id).legacy_path()),
        "pull_request" => row.pull_request_url.clone(),
        _ => conversation_href(&row).map(|href| match row.message_id {
            Some(message_id)
                if matches!(
                    row.kind.as_str(),
                    kind::MENTION
                        | kind::CONVERSATION_MESSAGE
                        | kind::THREAD_REPLY
                        | kind::DIRECT_MESSAGE
                        | kind::KEYWORD
                        | kind::REMINDER
                        | kind::THING_COMMENT
                ) && row.message_preview.is_some() =>
            {
                format!("{href}#message-{message_id}")
            }
            _ => href,
        }),
    }
    .or_else(|| {
        row.detail
            .get("href")
            .and_then(serde_json::Value::as_str)
            .filter(|href| href.starts_with('/'))
            .map(str::to_owned)
    })
    .unwrap_or_else(|| Target::space(row.project_id, Place::Home).legacy_path());
    // Built (and stored) in today's shape; written as this server writes
    // links, in the notification's workspace.
    let workspace = match &row.workspace_slug {
        Some(slug) => WorkspaceAddress::new(row.workspace_id, slug.clone()),
        None => WorkspaceAddress::id(row.workspace_id),
    };
    let href = addresses.from_legacy(&workspace, &href);
    let request = match (
        row.request_kind.clone(),
        row.request_status.clone(),
        row.request_title.clone(),
    ) {
        (Some(kind), Some(status), Some(title)) => {
            let options = crate::requests::options_of(
                row.request_options
                    .as_ref()
                    .unwrap_or(&serde_json::Value::Null),
            );
            Some(crate::requests::RequestCard {
                id: row.subject_id,
                kind,
                status,
                title,
                answer: crate::requests::answer_of(row.request_answer.as_ref(), &options),
                options,
                allow_text: row.request_allow_text.unwrap_or(false),
                answered_at: row.request_answered_at.clone(),
            })
        }
        _ => None,
    };
    let launch_href = row
        .detail
        .get("launchHref")
        .and_then(serde_json::Value::as_str)
        .filter(|href| href.starts_with('/'))
        .map(|href| addresses.from_legacy(&workspace, href));
    let batch = row.detail.get("batch").and_then(|batch| {
        Some(BatchAction {
            account_id: batch
                .get("accountId")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())?,
            workspace_id: row.workspace_id,
            size: batch.get("size").and_then(serde_json::Value::as_u64)?,
            window_key: batch
                .get("windowKey")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })
    });
    let reply_conversation_id = row.conversation_id.filter(|_| {
        matches!(
            row.kind.as_str(),
            kind::MENTION
                | kind::CONVERSATION_MESSAGE
                | kind::THREAD_REPLY
                | kind::DIRECT_MESSAGE
                | kind::KEYWORD
        ) && row.message_id.is_some()
    });
    let saved_item_id = (row.kind == kind::REMINDER)
        .then(|| {
            row.detail
                .get("savedItemId")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
        })
        .flatten();
    // Kinds that belong to no project (capacity alerts) name their place.
    let project_name = row
        .detail
        .get("place")
        .and_then(serde_json::Value::as_str)
        .filter(|place| !place.is_empty())
        .map_or(row.project_name, str::to_owned);
    let subject_title = if row.subject_kind == "provider_account" {
        project_name.clone()
    } else {
        subject_title
    };
    let attention = Attention::of(&row.kind, row.request_status.as_deref());
    NotificationView {
        attention,
        id: row.id,
        kind: row.kind,
        project_id: row.project_id,
        project_name,
        subject: SubjectView {
            kind: row.subject_kind,
            id: row.subject_id,
            title: subject_title,
        },
        title,
        preview: preview.filter(|text| !text.trim().is_empty()),
        actor: row
            .actor_id
            .zip(row.actor_name)
            .map(|(id, name)| NotificationActor {
                id,
                name,
                kind: row.actor_kind.unwrap_or_else(|| "human".to_owned()),
            }),
        count: row.repeat_count,
        read: row.read,
        archived: row.archived,
        snoozed_until: row.snoozed_until,
        until_activity: row.until_activity,
        at: row.at,
        created_at: row.created_at,
        href,
        request,
        launch_href,
        batch,
        reply_conversation_id,
        saved_item_id,
    }
}

// ---------------------------------------------------------------------------
// Changing

#[derive(Clone, Debug)]
pub enum Selection {
    Ids(Vec<Uuid>),
    /// Everything in the inbox (optionally one project's).
    AllInInbox {
        project_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    /// Notifications that changed.
    pub updated: Vec<Uuid>,
    pub unread_count: i64,
}

/// Changes `viewer`'s own notifications. Only rows they can still see are
/// touched; anything else is skipped silently, like an ID that isn't theirs.
pub async fn update(
    pool: &PgPool,
    viewer: Uuid,
    within: Option<&[Uuid]>,
    selection: &Selection,
    action: UpdateAction,
    snooze_until: Option<&str>,
) -> Result<UpdateResult, AppError> {
    let snooze_until = match (action, snooze_until) {
        (UpdateAction::Snooze, Some(value)) => Some(parse_snooze(value)?),
        (UpdateAction::Snooze, None) => return Err(AppError::Validation("snoozeUntil")),
        (_, Some(_)) => return Err(AppError::Validation("snoozeUntil")),
        (_, None) => None,
    };
    let (ids, all_project, all) = match selection {
        Selection::Ids(ids) => {
            if ids.is_empty() || ids.len() > MAX_UPDATE_IDS {
                return Err(AppError::Validation("ids"));
            }
            (Some(ids.clone()), None, false)
        }
        Selection::AllInInbox { project_id } => {
            if !matches!(action, UpdateAction::Read | UpdateAction::Archive) {
                return Err(AppError::Validation("action"));
            }
            (None, *project_id, true)
        }
    };
    let change = match action {
        UpdateAction::Read => "read_at = COALESCE(n.read_at, now())",
        UpdateAction::Unread => "read_at = NULL",
        UpdateAction::Archive => {
            "archived_at = COALESCE(n.archived_at, now()), read_at = COALESCE(n.read_at, now()), snoozed_until = NULL, wake_on_activity = false"
        }
        UpdateAction::Restore => "archived_at = NULL",
        UpdateAction::Snooze => {
            "snoozed_until = $6, surfaced_at = $6, read_at = NULL, archived_at = NULL, wake_on_activity = false"
        }
        UpdateAction::SnoozeUntilActivity => {
            "snoozed_until = now() + make_interval(days => 366), surfaced_at = now() + make_interval(days => 366), read_at = NULL, archived_at = NULL, wake_on_activity = true"
        }
        UpdateAction::Unsnooze => {
            "snoozed_until = NULL, surfaced_at = now(), wake_on_activity = false"
        }
    };
    let applies = match action {
        UpdateAction::Read => "n.read_at IS NULL",
        UpdateAction::Unread => "n.read_at IS NOT NULL",
        UpdateAction::Archive => "n.archived_at IS NULL",
        // Restoring must not make a second open row in a group.
        UpdateAction::Restore => {
            "n.archived_at IS NOT NULL AND NOT EXISTS (SELECT 1 FROM notifications open WHERE open.recipient_actor_id = n.recipient_actor_id AND open.group_key = n.group_key AND open.archived_at IS NULL)"
        }
        UpdateAction::Snooze | UpdateAction::SnoozeUntilActivity => {
            "NOT EXISTS (SELECT 1 FROM notifications open WHERE open.recipient_actor_id = n.recipient_actor_id AND open.group_key = n.group_key AND open.archived_at IS NULL AND open.id <> n.id)"
        }
        UpdateAction::Unsnooze => "n.snoozed_until IS NOT NULL",
    };
    let all_condition = if all { View::Inbox.condition() } else { "true" };
    // AWD-75: nothing from a module that's switched off.
    let switches = crate::modules::state::notification_switches_sql();
    let sql = format!(
        r#"
        UPDATE notifications n
        SET {change}, updated_at = now()
        WHERE n.recipient_actor_id = $1
          AND ($2::uuid[] IS NULL OR n.id = ANY($2))
          AND ($3::uuid IS NULL OR COALESCE(n.project_id, direct_scope_id()) = $3)
          AND ($4::uuid[] IS NULL OR COALESCE(n.project_id, direct_scope_id()) = ANY($4))
          AND ($5 OR $2::uuid[] IS NOT NULL)
          AND {all_condition}
          AND {applies}
          AND ($6::timestamptz IS NULL OR true)
          AND {VISIBLE}
          AND {switches}
        RETURNING n.id, COALESCE(n.project_id, direct_scope_id())
        "#
    );
    let mut transaction = crate::db::scoped_tx(pool).await?;
    // The recipient acts on their own inbox; the call's request id and
    // credential come from the edge.
    crate::events::set_context(
        &mut transaction,
        &crate::events::EventContext::actor(viewer, None),
    )
    .await?;
    let changed = sqlx::query_as::<_, (Uuid, Uuid)>(&sql)
        .bind(viewer)
        .bind(ids)
        .bind(all_project)
        .bind(within)
        .bind(all)
        .bind(snooze_until)
        .fetch_all(&mut *transaction)
        .await?;
    let projects = changed
        .iter()
        .map(|(_, project_id)| *project_id)
        .collect::<BTreeSet<_>>();
    for project_id in projects.into_iter().take(MAX_CHANGE_EVENTS) {
        announce(&mut transaction, project_id, viewer).await?;
    }
    transaction.commit().await?;
    Ok(UpdateResult {
        updated: changed.into_iter().map(|(id, _)| id).collect(),
        unread_count: unread_count(pool, viewer, within).await?,
    })
}

fn parse_snooze(value: &str) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
    let until = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| AppError::Validation("snoozeUntil"))?
        .with_timezone(&chrono::Utc);
    let now = chrono::Utc::now();
    if until <= now || until > now + chrono::Duration::days(MAX_SNOOZE_DAYS) {
        return Err(AppError::Validation("snoozeUntil"));
    }
    Ok(until)
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/inbox", get(list_http))
        .route("/api/v1/inbox/summary", get(summary_http))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    view: Option<View>,
    category: Option<Category>,
    project_id: Option<Uuid>,
    /// Only these spaces, comma-separated (a saved inbox view's, C2.11).
    spaces: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<NotificationPage>, AppError> {
    let spaces = query
        .spaces
        .as_deref()
        .map(views::parse_spaces)
        .transpose()?;
    let mut page = list(
        &state.pool,
        Addresses::of(&state),
        principal.actor_id,
        &ListFilter {
            view: query.view.unwrap_or_default(),
            category: query.category.unwrap_or_default(),
            project_id: query.project_id,
            within: spaces.clone(),
            cursor: query.cursor,
            limit: query.limit.unwrap_or(DEFAULT_PAGE_SIZE),
        },
    )
    .await?;
    if spaces.is_some() {
        // The badge counts the whole inbox, not the view's spaces.
        page.unread_count = unread_count(&state.pool, principal.actor_id, None).await?;
    }
    Ok(Json(page))
}

/// The inbox's unread count and what the badges show (C2.4).
async fn summary_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<Attentions>, AppError> {
    Ok(Json(attentions(&state.pool, principal.actor_id).await?))
}

#[cfg(test)]
mod tests {
    use super::{Category, View, kind, parse_cursor};

    #[test]
    fn categories_cover_every_built_in_kind() {
        let covered = [
            Category::Mentions,
            Category::Tasks,
            Category::Runs,
            Category::PullRequests,
        ]
        .iter()
        .flat_map(|category| category.kinds().unwrap_or_default().iter().copied())
        .collect::<std::collections::BTreeSet<_>>();
        for built_in in [
            kind::MENTION,
            kind::COMMENT_MENTION,
            kind::ASSIGNED,
            kind::REQUEST_RAISED,
            kind::RUN_WAITING,
            kind::RUN_COMPLETED,
            kind::RUN_FAILED,
            kind::CHECKS_FAILED,
            kind::REVIEW_REQUESTED,
            kind::TASK_CHANGED,
        ] {
            assert!(covered.contains(built_in), "{built_in}");
        }
        assert!(Category::All.kinds().is_none());
    }

    #[test]
    fn cursors_round_trip_and_reject_junk() {
        let id = uuid::Uuid::now_v7();
        assert_eq!(parse_cursor(&format!("123.{id}")).ok(), Some((123, id)));
        assert!(parse_cursor("123").is_err());
        assert!(parse_cursor("x.y").is_err());
        assert!(parse_cursor(&format!("1.{id}x")).is_err());
    }

    #[test]
    fn the_unread_view_is_the_inbox_narrowed() {
        assert!(
            View::Unread
                .condition()
                .starts_with(View::Inbox.condition())
        );
    }
}

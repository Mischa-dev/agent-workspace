//! Notifications for people (roadmap N1) and the inbox they fill (N2).
//!
//! Notifications are written by the outbox publisher: once an event has been
//! broadcast, `fan_out` runs in the same transaction that marks it published,
//! so they need no job queue and a failed write retries with the event. Each
//! source occurrence (one @mention, one assignment, one failed run) carries a
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

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    domain::RunState,
    error::{ApiJson, ApiQuery, AppError},
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

pub mod kind {
    pub const MENTION: &str = "mention";
    pub const COMMENT_MENTION: &str = "comment_mention";
    pub const ASSIGNED: &str = "assigned";
    pub const APPROVAL_REQUESTED: &str = "approval_requested";
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
}

/// Groups of kinds people filter the inbox by.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    #[default]
    All,
    Mentions,
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
            Self::Mentions => Some(&[kind::MENTION, kind::COMMENT_MENTION]),
            Self::Tasks => Some(&[kind::ASSIGNED, kind::COMMENT_MENTION, kind::TASK_CHANGED]),
            Self::Runs => Some(&[
                kind::APPROVAL_REQUESTED,
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
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
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
    pub comment_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub approval_id: Option<Uuid>,
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
            comment_id: None,
            agent_run_id: None,
            approval_id: None,
            pull_request_id: None,
            detail: serde_json::json!({}),
            group_key,
            dedupe_key,
        }
    }
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
    let already_seen = match notification.message_id {
        Some(message_id) => {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM conversation_entries entry
                    JOIN conversation_read_states state
                      ON state.conversation_id = entry.conversation_id
                     AND state.actor_id = $2
                    WHERE entry.message_id = $1
                      AND entry.sequence <= state.highest_seen_sequence
                )
                "#,
            )
            .bind(message_id)
            .bind(notification.recipient_actor_id)
            .fetch_one(&mut **transaction)
            .await?
        }
        None => false,
    };
    let notification_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO notifications (
            id, project_id, recipient_actor_id, kind, subject_kind, subject_id,
            actor_id, conversation_id, message_id, work_item_id, comment_id,
            agent_run_id, approval_id, pull_request_id, detail, group_key, archived_at,
            read_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                  CASE WHEN $17 THEN NULL ELSE now() END,
                  CASE WHEN $18 THEN now() END)
        ON CONFLICT (recipient_actor_id, group_key) WHERE archived_at IS NULL
        DO UPDATE SET
            kind = EXCLUDED.kind,
            actor_id = EXCLUDED.actor_id,
            message_id = EXCLUDED.message_id,
            comment_id = EXCLUDED.comment_id,
            approval_id = EXCLUDED.approval_id,
            pull_request_id = COALESCE(EXCLUDED.pull_request_id, notifications.pull_request_id),
            detail = EXCLUDED.detail,
            repeat_count = notifications.repeat_count + 1,
            read_at = CASE WHEN $18 THEN COALESCE(notifications.read_at, now()) END,
            surfaced_at = GREATEST(now(), COALESCE(notifications.snoozed_until, now())),
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
    .bind(notification.comment_id)
    .bind(notification.agent_run_id)
    .bind(notification.approval_id)
    .bind(notification.pull_request_id)
    .bind(&notification.detail)
    .bind(&notification.group_key)
    .bind(decision.in_app)
    .bind(already_seen)
    .fetch_one(&mut **transaction)
    .await?;
    if decision.push || decision.email {
        let organization_id =
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(notification.project_id)
                .fetch_one(&mut **transaction)
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

/// Turns one published outbox event into notifications. Called by the
/// outbox publisher inside the transaction that marks the event published.
/// Returns how many people were notified.
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
pub(crate) async fn read_for_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    viewer: Uuid,
    conversation_id: Uuid,
    sequence: i64,
) -> Result<(), AppError> {
    let read = sqlx::query(
        r#"
        UPDATE notifications n
        SET read_at = now(), updated_at = now()
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

/// The people a message mentions: everyone it names, and for `@everyone` or
/// `@here`, every person who can see the conversation. Agents are only ever
/// named; the reply rules decide whether a message wakes them.
async fn message_mentions(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let rows = sqlx::query_as::<_, MentionRow>(
        r#"
        WITH message AS (
            SELECT id, project_id, conversation_id, author_actor_id, content,
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
            JOIN actors actor ON actor.id = membership.actor_id AND actor.kind = 'human'
            -- Written in the text itself, so a person whose handle happens
            -- to be "everyone" is still only a mention of them.
            WHERE EXISTS (
                    SELECT 1
                    FROM jsonb_array_elements(message.content -> 'nodes') node
                    WHERE node ->> 'type' = 'text' AND node ->> 'text' ~* $2
                  )
              AND conversation_visible_to(message.conversation_id, membership.actor_id)
        ),
        everyone_allowed AS (
            SELECT actor_id FROM everyone
            WHERE (SELECT count(*) FROM everyone) <= $3
               -- Decided when it was sent or last edited, with the app's agent
               -- limit then in force (messages.may_notify_everyone); older and
               -- system messages fall back to the author's role.
               OR COALESCE((SELECT may_notify_everyone FROM message), EXISTS (
                   SELECT 1
                   FROM message
                   JOIN project_memberships author
                     ON author.project_id = message.project_id
                    AND author.actor_id = message.author_actor_id
                    AND author.enabled
                   WHERE author.role IN ('owner', 'admin')
                     AND custom_role_allows(author.role_id, 'channels.moderate')
               ))
        )
        SELECT message.project_id, message.conversation_id, message.author_actor_id,
               recipient.actor_id AS recipient_actor_id
        FROM message
        JOIN (SELECT actor_id FROM named UNION SELECT actor_id FROM everyone_allowed) recipient
          ON recipient.actor_id <> message.author_actor_id
        WHERE actor_can_access_project(recipient.actor_id, NULL, message.project_id)
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
    let rows = sqlx::query_as::<_, MentionRow>(
        r#"
        WITH message AS (
            SELECT message.id, message.project_id, message.conversation_id,
                   message.author_actor_id, conversation.parent_channel_id
            FROM messages message
            JOIN conversations conversation ON conversation.id = message.conversation_id
            WHERE message.id = $1 AND message.removed_at IS NULL
        ),
        following AS (
            SELECT level.actor_id
            FROM message
            JOIN conversation_notification_levels level
              ON level.level = 'all'
             AND level.conversation_id IN (message.conversation_id, message.parent_channel_id)
        )
        SELECT DISTINCT message.project_id, message.conversation_id, message.author_actor_id,
               following.actor_id AS recipient_actor_id
        FROM message
        JOIN following ON following.actor_id <> message.author_actor_id
        JOIN actors actor ON actor.id = following.actor_id AND actor.kind = 'human'
        WHERE conversation_notification_level(following.actor_id, message.conversation_id) = 'all'
          AND actor_can_access_project(following.actor_id, NULL, message.project_id)
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
            let mut notification = NewNotification::new(
                row.project_id,
                row.recipient_actor_id,
                kind::CONVERSATION_MESSAGE,
                "conversation",
                row.conversation_id,
                format!("messages:{}", row.conversation_id),
                format!("message:{message_id}"),
            );
            notification.actor_id = Some(row.author_actor_id);
            notification.conversation_id = Some(row.conversation_id);
            notification.message_id = Some(message_id);
            notification
        })
        .collect())
}

/// A removed message stops counting: its occurrences go, and a notification
/// left with none goes too. The text itself was never copied here.
/// A deleted task comment's notifications go with it, as a removed
/// message's do. A notification that also stood for other occurrences stays,
/// counted again.
pub(crate) async fn forget_comment(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    comment_id: Uuid,
) -> Result<(), AppError> {
    let affected = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        DELETE FROM notification_occurrences
        WHERE dedupe_key = ANY($1)
        RETURNING notification_id, recipient_actor_id
        "#,
    )
    .bind(vec![
        format!("comment-mention:{comment_id}"),
        format!("watch:{comment_id}"),
    ])
    .fetch_all(&mut **transaction)
    .await?;
    if affected.is_empty() {
        return Ok(());
    }
    let ids = affected.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    sqlx::query(
        r#"
        DELETE FROM notifications n
        WHERE n.id = ANY($1)
          AND NOT EXISTS (
              SELECT 1 FROM notification_occurrences o WHERE o.notification_id = n.id
          )
        "#,
    )
    .bind(&ids)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE notifications n
        SET comment_id = NULLIF(n.comment_id, $2),
            repeat_count = GREATEST(1, (
                SELECT count(*) FROM notification_occurrences o WHERE o.notification_id = n.id
            )::integer),
            updated_at = now()
        WHERE n.id = ANY($1)
        "#,
    )
    .bind(&ids)
    .bind(comment_id)
    .execute(&mut **transaction)
    .await?;
    let recipients = affected
        .into_iter()
        .map(|(_, recipient)| recipient)
        .collect::<BTreeSet<_>>();
    for recipient in recipients {
        announce(transaction, project_id, recipient).await?;
    }
    Ok(())
}

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
struct CommentMentionRow {
    comment_id: Uuid,
    author_actor_id: Uuid,
    recipient_actor_id: Uuid,
}

#[derive(FromRow)]
struct AssignmentRow {
    activity_id: Uuid,
    actor_id: Uuid,
    assignee_actor_id: Uuid,
}

#[derive(FromRow)]
struct WatchedChangeRow {
    /// The activity or comment this change is.
    change_id: Uuid,
    actor_id: Uuid,
    comment_id: Option<Uuid>,
    /// created, updated, archived, restored or commented.
    change: String,
    recipient_actor_id: Uuid,
}

/// What a task change means for people. `work_item.updated` carries only the
/// task and its version, so this finds the comment and activity rows written
/// in the same transaction as the event: they share its `created_at`
/// (PostgreSQL's `now()` is the transaction's start).
async fn work_item_changes(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<NewNotification>, AppError> {
    let mut notifications = Vec::new();
    let comments = sqlx::query_as::<_, CommentMentionRow>(
        r#"
        SELECT DISTINCT comment.id AS comment_id, comment.author_actor_id,
               mention.actor_id AS recipient_actor_id
        FROM outbox_events event
        JOIN work_item_comments comment
          ON comment.project_id = event.project_id
         AND comment.work_item_id = $3
         -- A new comment, or an edit (it can add mentions; people already
         -- told about this comment aren't told again).
         AND (comment.created_at = event.created_at OR comment.edited_at = event.created_at)
         AND comment.removed_at IS NULL
        JOIN work_item_comment_mentions mention ON mention.comment_id = comment.id
        WHERE event.event_id = $1 AND event.project_id = $2
          AND mention.actor_id <> comment.author_actor_id
          AND actor_can_access_project(mention.actor_id, NULL, comment.project_id)
        ORDER BY comment.id, mention.actor_id
        "#,
    )
    .bind(event_id)
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in comments {
        let mut notification = NewNotification::new(
            project_id,
            row.recipient_actor_id,
            kind::COMMENT_MENTION,
            "work_item",
            work_item_id,
            format!("mention:task:{work_item_id}"),
            format!("comment-mention:{}", row.comment_id),
        );
        notification.actor_id = Some(row.author_actor_id);
        notification.work_item_id = Some(work_item_id);
        notification.comment_id = Some(row.comment_id);
        notifications.push(notification);
    }
    let assignments = sqlx::query_as::<_, AssignmentRow>(
        r#"
        SELECT activity.id AS activity_id, activity.actor_id,
               item.assignee_actor_id
        FROM outbox_events event
        JOIN work_item_activity activity
          ON activity.project_id = event.project_id
         AND activity.work_item_id = $3
         AND activity.created_at = event.created_at
        JOIN work_items item
          ON item.project_id = activity.project_id AND item.id = activity.work_item_id
        WHERE event.event_id = $1 AND event.project_id = $2
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
            SELECT activity.id AS change_id, activity.actor_id, NULL::uuid AS comment_id,
                   replace(activity.kind, 'work_item_', '') AS change,
                   activity.project_id, activity.work_item_id
            FROM outbox_events event
            JOIN work_item_activity activity
              ON activity.project_id = event.project_id
             AND activity.work_item_id = $3
             AND activity.created_at = event.created_at
            WHERE event.event_id = $1 AND event.project_id = $2
            UNION ALL
            SELECT comment.id, comment.author_actor_id, comment.id, 'commented',
                   comment.project_id, comment.work_item_id
            FROM outbox_events event
            JOIN work_item_comments comment
              ON comment.project_id = event.project_id
             AND comment.work_item_id = $3
             AND comment.created_at = event.created_at
             AND comment.removed_at IS NULL
            WHERE event.event_id = $1 AND event.project_id = $2
        )
        SELECT change.change_id, change.actor_id, change.comment_id, change.change,
               watcher.actor_id AS recipient_actor_id
        FROM changes change
        JOIN work_item_watchers watcher
          ON watcher.project_id = change.project_id
         AND watcher.work_item_id = change.work_item_id
        WHERE watcher.actor_id <> change.actor_id
          AND actor_can_access_project(watcher.actor_id, NULL, change.project_id)
        ORDER BY change.change_id, watcher.actor_id
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
        notification.comment_id = row.comment_id;
        notification.detail = serde_json::json!({ "change": row.change });
        notifications.push(notification);
    }
    Ok(notifications)
}

#[derive(FromRow)]
struct RunRow {
    project_id: Uuid,
    conversation_id: Uuid,
    requesting_actor_id: Uuid,
    requester_reachable: bool,
}

#[derive(FromRow)]
struct ApproverRow {
    approval_id: Uuid,
    actor_id: Uuid,
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
    let Some(run) = sqlx::query_as::<_, RunRow>(
        r#"
        SELECT conversation.project_id, run.conversation_id, run.requesting_actor_id,
               actor_can_access_project(run.requesting_actor_id, NULL, conversation.project_id)
               AND conversation_visible_to(run.conversation_id, run.requesting_actor_id)
                   AS requester_reachable
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
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
    let mut approvers = BTreeSet::new();
    if state == RunState::Blocked {
        // Everyone who may answer, as respond_to_approval decides it.
        let rows = sqlx::query_as::<_, ApproverRow>(
            r#"
            SELECT approval.id AS approval_id, membership.actor_id
            FROM agent_approvals approval
            JOIN agent_runs run ON run.id = approval.agent_run_id
            JOIN conversations conversation ON conversation.id = run.conversation_id
            JOIN project_memberships membership
              ON membership.project_id = conversation.project_id
             AND membership.enabled
             AND membership.role IN ('owner', 'admin', 'member')
             AND custom_role_allows(membership.role_id, 'runs.approve')
            WHERE approval.agent_run_id = $1
              AND approval.status = 'pending'
              AND (
                  approval.eligible_actor_id = membership.actor_id
                  OR (
                      approval.eligible_actor_id IS NULL
                      AND approval.eligible_role IN (membership.role, 'any_authorized')
                  )
              )
              AND actor_can_access_project(membership.actor_id, NULL, conversation.project_id)
              AND conversation_visible_to(conversation.id, membership.actor_id)
            ORDER BY approval.id, membership.actor_id
            "#,
        )
        .bind(run_id)
        .fetch_all(&mut **transaction)
        .await?;
        for row in rows {
            approvers.insert(row.actor_id);
            let mut notification = base(
                row.actor_id,
                kind::APPROVAL_REQUESTED,
                format!("approval:{}", row.approval_id),
            );
            notification.approval_id = Some(row.approval_id);
            notifications.push(notification);
        }
    }
    if run.requester_reachable && !approvers.contains(&run.requesting_actor_id) {
        let dedupe_key = match state {
            // Each wait is its own occurrence; the approval says which.
            RunState::Blocked => {
                let approval_id = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM agent_approvals WHERE agent_run_id = $1 AND status = 'pending' ORDER BY created_at DESC LIMIT 1",
                )
                .bind(run_id)
                .fetch_optional(&mut **transaction)
                .await?;
                match approval_id {
                    Some(approval_id) => format!("run-waiting:{approval_id}"),
                    None => return Ok(notifications),
                }
            }
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
            sqlx::query_as::<_, PullRequestRow>(
                r#"
                SELECT DISTINCT pr.id, pr.head_sha, pr.checks_state,
                       run.requesting_actor_id AS owner_actor_id
                FROM github_pull_request_workspace_links link
                JOIN github_pull_requests pr
                  ON pr.project_id = link.project_id AND pr.id = link.github_pull_request_id
                JOIN agent_runs run ON run.id = link.agent_run_id
                WHERE link.project_id = $1 AND link.agent_run_id = $2 AND pr.state = 'open'
                "#,
            )
            .bind(project_id)
            .bind(run_id)
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
                JOIN work_items item
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
    actor_can_access_project($1, NULL, n.project_id)
    AND (n.conversation_id IS NULL OR conversation_visible_to(n.conversation_id, $1))
    AND (n.pull_request_id IS NULL OR pull_request_visible_to(n.pull_request_id, $1))
    -- A task's or pull request's notification only while the recipient's
    -- role there still lets them see tasks, or code. An app's agent limit
    -- is applied to the listed rows (`hidden_by_limit`).
    AND (n.work_item_id IS NULL OR EXISTS (
        SELECT 1 FROM project_memberships sees_task
        WHERE sees_task.project_id = n.project_id AND sees_task.actor_id = $1
          AND sees_task.enabled AND custom_role_allows(sees_task.role_id, 'tasks.view')
    ))
    AND (n.pull_request_id IS NULL OR EXISTS (
        SELECT 1 FROM project_memberships sees_code
        WHERE sees_code.project_id = n.project_id AND sees_code.actor_id = $1
          AND sees_code.enabled AND custom_role_allows(sees_code.role_id, 'code.view')
    ))
    AND (n.subject_kind <> 'provider_account' OR EXISTS (
        SELECT 1 FROM provider_accounts account
        WHERE account.id = n.subject_id AND account.owner_actor_id = $1
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
    /// When it last came to the top of the inbox.
    pub at: String,
    pub created_at: String,
    /// Where to open it in the web app (an https URL for a pull request).
    pub href: String,
    /// Set when the notification is about an approval that can be answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalView>,
    /// Where its Launch action goes, for notifications that have one (a
    /// capacity alert opens the account's ready tasks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_href: Option<String>,
    /// A capacity alert's suggested batch, which Launch starts in one tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchAction>,
    /// The conversation a Reply from the notification posts in (mentions).
    #[serde(skip)]
    pub reply_conversation_id: Option<Uuid>,
    /// A reminder's saved item, which Done and Snooze change (W21).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_item_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAction {
    pub account_id: Uuid,
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
pub struct ApprovalView {
    pub id: Uuid,
    /// approval or question
    pub kind: String,
    /// pending, resolved, superseded or canceled
    pub status: String,
    pub options: serde_json::Value,
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
    project_name: String,
    direct_space: bool,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    actor_kind: Option<String>,
    repeat_count: i32,
    read: bool,
    archived: bool,
    snoozed_until: Option<String>,
    at: String,
    at_micros: i64,
    created_at: String,
    detail: serde_json::Value,
    conversation_id: Option<Uuid>,
    conversation_kind: Option<String>,
    conversation_name: Option<String>,
    parent_conversation_id: Option<Uuid>,
    message_id: Option<Uuid>,
    message_preview: Option<String>,
    comment_preview: Option<String>,
    work_item_id: Option<Uuid>,
    work_item_key: Option<String>,
    work_item_title: Option<String>,
    run_failure: Option<String>,
    run_state: Option<String>,
    run_agent_name: Option<String>,
    approval_id: Option<Uuid>,
    approval_kind: Option<String>,
    approval_status: Option<String>,
    approval_summary: Option<String>,
    approval_options: Option<serde_json::Value>,
    pull_request_number: Option<i64>,
    pull_request_title: Option<String>,
    pull_request_url: Option<String>,
}

fn list_sql(view_condition: &str, extra: &str) -> String {
    // AWD-75: nothing from a module that's switched off.
    let switches = crate::modules::state::notification_switches_sql();
    format!(
        r#"
        SELECT n.id, n.kind, n.subject_kind, n.subject_id, n.project_id,
               CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END
                   AS project_name,
               project.kind = 'direct' AS direct_space,
               n.actor_id, actor_member.display_name AS actor_name,
               actor.kind AS actor_kind,
               n.repeat_count, n.read_at IS NOT NULL AS read,
               n.archived_at IS NOT NULL AS archived,
               CASE WHEN n.snoozed_until > now() THEN
                   to_char(n.snoozed_until AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
               END AS snoozed_until,
               to_char(n.surfaced_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS at,
               (extract(epoch FROM n.surfaced_at) * 1000000)::bigint AS at_micros,
               to_char(n.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
                   AS created_at,
               n.detail,
               n.conversation_id, conversation.kind AS conversation_kind,
               conversation.name AS conversation_name,
               conversation.parent_channel_id AS parent_conversation_id,
               n.message_id,
               CASE WHEN message.removed_at IS NULL
                    THEN left(message.plain_text, {PREVIEW_CHARACTERS}) END AS message_preview,
               left(comment.plain_text, {PREVIEW_CHARACTERS}) AS comment_preview,
               n.work_item_id,
               project.project_key || '-' || item.number::text AS work_item_key,
               item.title AS work_item_title,
               left(run.failure_reason, {PREVIEW_CHARACTERS}) AS run_failure,
               run.state AS run_state, run_agent.display_name AS run_agent_name,
               n.approval_id, approval.kind AS approval_kind, approval.status AS approval_status,
               left(approval.summary, {PREVIEW_CHARACTERS}) AS approval_summary,
               approval.options AS approval_options,
               pr.pull_request_number, pr.title AS pull_request_title,
               pr.html_url AS pull_request_url
        FROM notifications n
        JOIN projects project ON project.id = n.project_id
        LEFT JOIN project_memberships actor_member
          ON actor_member.project_id = n.project_id AND actor_member.actor_id = n.actor_id
        LEFT JOIN actors actor ON actor.id = n.actor_id
        LEFT JOIN conversations conversation ON conversation.id = n.conversation_id
        LEFT JOIN messages message ON message.id = n.message_id
        LEFT JOIN work_item_comments comment ON comment.id = n.comment_id
        LEFT JOIN work_items item ON item.id = n.work_item_id
        LEFT JOIN agent_runs run ON run.id = n.agent_run_id
        LEFT JOIN execution_workers run_worker ON run_worker.id = run.execution_worker_id
        LEFT JOIN project_memberships run_agent
          ON run_agent.project_id = n.project_id AND run_agent.actor_id = run_worker.actor_id
        LEFT JOIN agent_approvals approval ON approval.id = n.approval_id
        LEFT JOIN github_pull_requests pr ON pr.id = n.pull_request_id
        WHERE n.recipient_actor_id = $1
          AND {view_condition}
          AND ($2::text[] IS NULL OR n.kind = ANY($2))
          AND ($3::uuid IS NULL OR n.project_id = $3)
          AND ($4::uuid[] IS NULL OR n.project_id = ANY($4))
          AND {VISIBLE}
          AND {switches}
          {extra}
        "#
    )
}

/// One page of `viewer`'s notifications, newest first.
pub async fn list(
    pool: &PgPool,
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
        .fetch_all(pool)
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
            .map(view)
            .collect(),
        next_cursor,
        unread_count,
    })
}

/// A task's or pull request's notification an app acting for `viewer` may
/// not see: its agent limit leaves out "see tasks", or "see code", there.
fn hidden_by_limit(viewer: Uuid, row: &NotificationRow) -> bool {
    use crate::policy::{Permission, agent_limit_hides};
    (row.work_item_id.is_some() && agent_limit_hides(viewer, row.project_id, Permission::TasksView))
        || (row.pull_request_number.is_some()
            && agent_limit_hides(viewer, row.project_id, Permission::CodeView))
}

/// The given notifications, if they are the viewer's and still visible.
pub async fn get_many(
    pool: &PgPool,
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
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter(|row| !hidden_by_limit(viewer, row))
        .map(view)
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
          AND ($2::uuid[] IS NULL OR n.project_id = ANY($2))
          AND {VISIBLE}
          AND {switches}
        "#,
        view = View::Unread.condition(),
    ))
    .bind(viewer)
    .bind(within)
    .fetch_one(pool)
    .await?)
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
        SELECT n.project_id, count(*) FROM notifications n
        WHERE n.recipient_actor_id = $1
          AND {view}
          AND {VISIBLE}
          AND {switches}
        GROUP BY n.project_id
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

fn conversation_href(row: &NotificationRow) -> Option<String> {
    let conversation_id = row.conversation_id?;
    if row.direct_space {
        let id = row.parent_conversation_id.unwrap_or(conversation_id);
        return Some(format!("/messages/{id}"));
    }
    let segment = if row.conversation_kind.as_deref() == Some("thread") {
        "threads"
    } else {
        "channels"
    };
    Some(format!(
        "/projects/{}/{segment}/{conversation_id}",
        row.project_id
    ))
}

/// "Muse's run", or "Your run" when the agent has no name here.
fn run_owner(row: &NotificationRow) -> String {
    row.run_agent_name
        .as_ref()
        .map_or_else(|| "Your run".to_owned(), |agent| format!("{agent}'s run"))
}

fn view(row: NotificationRow) -> NotificationView {
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
    let question = row.approval_kind.as_deref() == Some("question");
    let (title, preview) = match row.kind.as_str() {
        kind::MENTION => (
            format!("{who} mentioned you in {place}"),
            row.message_preview.clone(),
        ),
        kind::CONVERSATION_MESSAGE => (format!("{who} in {place}"), row.message_preview.clone()),
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
            format!("{who} mentioned you on {task_key}"),
            row.comment_preview.clone(),
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
            row.comment_preview
                .clone()
                .or_else(|| row.work_item_title.clone()),
        ),
        kind::APPROVAL_REQUESTED => (
            match (&row.run_agent_name, question) {
                (Some(agent), true) => format!("{agent} has a question for you in {place}"),
                (Some(agent), false) => format!("{agent} needs your approval in {place}"),
                (None, true) => format!("A run in {place} has a question for you"),
                (None, false) => format!("A run in {place} needs your approval"),
            },
            row.approval_summary.clone(),
        ),
        kind::RUN_WAITING => (
            row.run_agent_name.as_ref().map_or_else(
                || format!("Your run in {place} is waiting on you"),
                |agent| format!("{agent} is waiting on you in {place}"),
            ),
            row.approval_summary.clone(),
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
            .map(|id| format!("/projects/{}/work-items/{id}", row.project_id)),
        "pull_request" => row.pull_request_url.clone(),
        _ => conversation_href(&row).map(|href| match row.message_id {
            Some(message_id)
                if matches!(
                    row.kind.as_str(),
                    kind::MENTION | kind::CONVERSATION_MESSAGE | kind::REMINDER
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
    .unwrap_or_else(|| format!("/projects/{}", row.project_id));
    let approval = match (row.approval_id, row.approval_kind, row.approval_status) {
        (Some(id), Some(kind), Some(status)) => Some(ApprovalView {
            id,
            kind,
            status,
            options: row
                .approval_options
                .unwrap_or_else(|| serde_json::json!([])),
        }),
        _ => None,
    };
    let launch_href = row
        .detail
        .get("launchHref")
        .and_then(serde_json::Value::as_str)
        .filter(|href| href.starts_with('/'))
        .map(str::to_owned);
    let batch = row.detail.get("batch").and_then(|batch| {
        Some(BatchAction {
            account_id: batch
                .get("accountId")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())?,
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
            kind::MENTION | kind::CONVERSATION_MESSAGE
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
    NotificationView {
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
        at: row.at,
        created_at: row.created_at,
        href,
        approval,
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
            "archived_at = COALESCE(n.archived_at, now()), read_at = COALESCE(n.read_at, now()), snoozed_until = NULL"
        }
        UpdateAction::Restore => "archived_at = NULL",
        UpdateAction::Snooze => {
            "snoozed_until = $6, surfaced_at = $6, read_at = NULL, archived_at = NULL"
        }
        UpdateAction::Unsnooze => "snoozed_until = NULL, surfaced_at = now()",
    };
    let applies = match action {
        UpdateAction::Read => "n.read_at IS NULL",
        UpdateAction::Unread => "n.read_at IS NOT NULL",
        UpdateAction::Archive => "n.archived_at IS NULL",
        // Restoring must not make a second open row in a group.
        UpdateAction::Restore => {
            "n.archived_at IS NOT NULL AND NOT EXISTS (SELECT 1 FROM notifications open WHERE open.recipient_actor_id = n.recipient_actor_id AND open.group_key = n.group_key AND open.archived_at IS NULL)"
        }
        UpdateAction::Snooze => {
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
          AND ($3::uuid IS NULL OR n.project_id = $3)
          AND ($4::uuid[] IS NULL OR n.project_id = ANY($4))
          AND ($5 OR $2::uuid[] IS NOT NULL)
          AND {all_condition}
          AND {applies}
          AND ($6::timestamptz IS NULL OR true)
          AND {VISIBLE}
          AND {switches}
        RETURNING n.id, n.project_id
        "#
    );
    let mut transaction = pool.begin().await?;
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
        .route("/api/v1/inbox/update", post(update_http))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    view: Option<View>,
    category: Option<Category>,
    project_id: Option<Uuid>,
    cursor: Option<String>,
    limit: Option<i64>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<NotificationPage>, AppError> {
    let page = list(
        &state.pool,
        principal.actor_id,
        &ListFilter {
            view: query.view.unwrap_or_default(),
            category: query.category.unwrap_or_default(),
            project_id: query.project_id,
            within: None,
            cursor: query.cursor,
            limit: query.limit.unwrap_or(DEFAULT_PAGE_SIZE),
        },
    )
    .await?;
    Ok(Json(page))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SummaryResponse {
    unread_count: i64,
}

async fn summary_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<SummaryResponse>, AppError> {
    Ok(Json(SummaryResponse {
        unread_count: unread_count(&state.pool, principal.actor_id, None).await?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    action: UpdateAction,
    ids: Option<Vec<Uuid>>,
    /// Everything in the inbox instead of `ids` (read and archive only).
    all: Option<bool>,
    project_id: Option<Uuid>,
    snooze_until: Option<String>,
}

async fn update_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<Json<UpdateResult>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let selection = match (request.ids, request.all) {
        (Some(ids), None | Some(false)) => {
            if request.project_id.is_some() {
                return Err(AppError::Validation("projectId"));
            }
            Selection::Ids(ids)
        }
        (None, Some(true)) => Selection::AllInInbox {
            project_id: request.project_id,
        },
        _ => return Err(AppError::Validation("ids")),
    };
    let result = update(
        &state.pool,
        principal.actor_id,
        None,
        &selection,
        request.action,
        request.snooze_until.as_deref(),
    )
    .await?;
    Ok(Json(result))
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
            kind::APPROVAL_REQUESTED,
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

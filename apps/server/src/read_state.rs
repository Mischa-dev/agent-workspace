//! A person's or agent's read mark in a conversation (P0.5): one function
//! moves it, for the web and for agents and apps, so reading anywhere also
//! reads the notifications the conversation holds for them.

pub mod ops;

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    outbox::{OutboxEventPayload, ReadStateUpdatedPayload},
};

/// Where an actor's read mark stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct ReadState {
    pub highest_seen_sequence: i64,
    pub version: i64,
}

/// Moves `actor`'s read mark in the conversation forward to `sequence`
/// (never back), tells their other tabs, and marks read the notifications
/// for what they have now seen. The caller has checked the actor may read
/// the conversation and that `sequence` exists. Returns the new mark, or
/// None when it was already at or past `sequence`.
pub async fn mark_seen(
    transaction: &mut Transaction<'_, Postgres>,
    // The conversation's place: its space, or the direct scope.
    project_id: Uuid,
    conversation_id: Uuid,
    actor_id: Uuid,
    sequence: i64,
) -> Result<Option<ReadState>, AppError> {
    let moved = sqlx::query_as::<_, ReadState>(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence, version
        )
        VALUES ($1, $2, $3, $4, 1)
        ON CONFLICT (conversation_id, actor_id) DO UPDATE
        SET highest_seen_sequence = EXCLUDED.highest_seen_sequence,
            version = conversation_read_states.version + 1,
            updated_at = now()
        WHERE EXCLUDED.highest_seen_sequence > conversation_read_states.highest_seen_sequence
        RETURNING highest_seen_sequence, version
        "#,
    )
    .bind(crate::spaces::space_of(project_id))
    .bind(conversation_id)
    .bind(actor_id)
    .bind(sequence)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some(state) = moved {
        crate::conversations::enqueue_event(
            transaction,
            project_id,
            OutboxEventPayload::ReadStateUpdated(ReadStateUpdatedPayload {
                conversation_id,
                actor_id,
                highest_seen_sequence: state.highest_seen_sequence,
                version: state.version,
            }),
        )
        .await?;
    }
    // Even when the mark didn't move (an earlier read got there first), what
    // is now seen has no business staying unread in the inbox.
    let seen = moved.map_or(sequence, |state| state.highest_seen_sequence);
    crate::notifications::read_for_conversation(
        transaction,
        project_id,
        actor_id,
        conversation_id,
        seen,
    )
    .await?;
    Ok(moved)
}

/// Unread counts stop here: "99+" is as precise as anyone needs.
pub const UNREAD_CAP: i64 = 99;

/// One conversation's unread state for a viewer (C2 plan 3.3, R1 to R7).
#[derive(Clone, Copy, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct UnreadRow {
    pub conversation_id: Uuid,
    /// The newest message that isn't removed is past the read mark (R1).
    pub unread: bool,
    /// Others' messages after the mark that aren't removed, at most 99.
    pub unread_messages: i64,
    /// Unread mention notifications for this conversation, at most 99.
    pub mentions: i64,
    /// Set to "nothing": still unread, but not bold and never counted.
    pub muted: bool,
}

/// Which conversations `unread_summary` reads.
#[derive(Clone, Debug)]
pub enum UnreadScope {
    /// A space's channels, DMs and groups the viewer can read (not threads:
    /// a thread's messages don't make its channel unread, R6).
    Space(Uuid),
    /// The viewer's DMs and groups, wherever they live.
    Direct,
    /// These conversations, those the viewer can read.
    Conversations(Vec<Uuid>),
}

/// The unread state of each conversation in `scope` the viewer can read.
/// Archived conversations, and ones the viewer can no longer read, never
/// appear (R7). Counting is bounded per row (`LIMIT 99` in a lateral).
pub async fn unread_summary(
    connection: &mut sqlx::PgConnection,
    viewer: Uuid,
    scope: UnreadScope,
) -> Result<Vec<UnreadRow>, AppError> {
    let (space, direct, ids) = match scope {
        UnreadScope::Space(space) => (Some(space), false, None),
        UnreadScope::Direct => (None, true, None),
        UnreadScope::Conversations(ids) => (None, false, Some(ids)),
    };
    Ok(sqlx::query_as::<_, UnreadRow>(
        r#"
        SELECT conversation.id AS conversation_id,
               conversation.last_message_sequence
                   > COALESCE(read_state.highest_seen_sequence, 0) AS unread,
               counted.messages AS unread_messages,
               LEAST(COALESCE(mentioned.count, 0), $5)::bigint AS mentions,
               conversation_notification_level($1, conversation.id) = 'nothing' AS muted
        FROM conversations conversation
        LEFT JOIN conversation_read_states read_state
          ON read_state.conversation_id = conversation.id AND read_state.actor_id = $1
        CROSS JOIN LATERAL (
            SELECT count(*) AS messages
            FROM (
                SELECT 1
                FROM conversation_entries entry
                JOIN messages message ON message.id = entry.message_id
                WHERE entry.conversation_id = conversation.id
                  AND entry.kind = 'message'
                  AND entry.sequence > COALESCE(read_state.highest_seen_sequence, 0)
                  AND entry.sequence <= conversation.last_message_sequence
                  AND message.removed_at IS NULL
                  -- AWD-220: a working note is never unread.
                  AND message.register = 'message'
                  AND message.author_actor_id <> $1
                LIMIT $5
            ) newer
        ) counted
        LEFT JOIN LATERAL (
            SELECT sum(notification.repeat_count) AS count
            FROM notifications notification
            WHERE notification.recipient_actor_id = $1
              AND notification.conversation_id = conversation.id
              AND notification.kind = 'mention'
              AND notification.read_at IS NULL
              AND notification.archived_at IS NULL
        ) mentioned ON true
        WHERE conversation.archived_at IS NULL
          -- A space's conversations to its members; DMs and groups (no
          -- space) by the visibility rule alone.
          AND (conversation.project_id IS NULL OR EXISTS (
                SELECT 1 FROM project_memberships me
                WHERE me.project_id = conversation.project_id AND me.actor_id = $1
                  AND me.enabled))
          AND ($2::uuid IS NULL
               OR (COALESCE(conversation.project_id, direct_scope_id()) = $2
                   AND conversation.kind IN ('channel', 'dm', 'group')))
          AND (NOT $3 OR (conversation.kind IN ('dm', 'group') AND EXISTS (
                SELECT 1 FROM conversation_members member
                WHERE member.conversation_id = conversation.id AND member.actor_id = $1)))
          AND ($4::uuid[] IS NULL OR conversation.id = ANY($4))
          AND conversation_visible_to(conversation.id, $1)
        "#,
    )
    .bind(viewer)
    .bind(space)
    .bind(direct)
    .bind(ids)
    .bind(UNREAD_CAP)
    .fetch_all(&mut *connection)
    .await?)
}

/// `unread_summary` as a map, for list responses.
pub async fn unread_by_conversation(
    connection: &mut sqlx::PgConnection,
    viewer: Uuid,
    scope: UnreadScope,
) -> Result<std::collections::HashMap<Uuid, UnreadRow>, AppError> {
    Ok(unread_summary(connection, viewer, scope)
        .await?
        .into_iter()
        .map(|row| (row.conversation_id, row))
        .collect())
}

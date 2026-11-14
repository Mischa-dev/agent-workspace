//! A person's or agent's read mark in a conversation (P0.5): one function
//! moves it, for the web and for agents and apps, so reading anywhere also
//! reads the notifications the conversation holds for them.

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
        ON CONFLICT (project_id, conversation_id, actor_id) DO UPDATE
        SET highest_seen_sequence = EXCLUDED.highest_seen_sequence,
            version = conversation_read_states.version + 1,
            updated_at = now()
        WHERE EXCLUDED.highest_seen_sequence > conversation_read_states.highest_seen_sequence
        RETURNING highest_seen_sequence, version
        "#,
    )
    .bind(project_id)
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

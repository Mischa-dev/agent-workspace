//! Message drafts across devices (C2.28). The composer keeps a draft in the
//! browser first; a moment after typing stops it saves it here too, so the
//! same draft is waiting on the writer's other devices. When two devices
//! save, the newer save wins and the other is kept as a recoverable copy for
//! a day. Only the writer reads a draft, and only while they can still read
//! its conversation.
//!
//! `message_draft_set` and `message_draft_list` (in `ops.rs`) are the only
//! ways in; the `message_draft.prune` job forgets old drafts and copies.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

pub mod ops;

/// The largest draft kept, as JSON.
pub const MAX_DRAFT_BYTES: usize = 65_536;
/// The most drafts one list answers with.
pub const MAX_LISTED: i64 = 200;

/// A draft as its writer sees it.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    #[schemars(with = "String")]
    pub conversation_id: Uuid,
    /// The space (project) the conversation is in.
    #[schemars(with = "String")]
    pub space_id: Uuid,
    /// channel, thread, dm or group.
    pub conversation_kind: String,
    /// The conversation's name; none for a DM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_name: Option<String>,
    /// What the composer saved: its text, and the people and tasks named.
    pub draft: Value,
    #[schemars(with = "String")]
    pub updated_at: DateTime<Utc>,
    /// The other device's version, when two saved at once (kept a day).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Value>,
}

/// Whether a draft is worth keeping: an object with some text, small enough.
pub fn checked(draft: &Value) -> Result<Option<&Value>, &'static str> {
    let Some(object) = draft.as_object() else {
        return Err("A draft is an object, such as {\"text\": \"…\"}.");
    };
    if serde_json::to_vec(draft).map_or(true, |bytes| bytes.len() > MAX_DRAFT_BYTES) {
        return Err("A draft is at most 64 KB.");
    }
    let has_text = object
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    Ok(has_text.then_some(draft))
}

/// What a save did.
pub struct Saved {
    /// The draft now kept, or None when it was cleared.
    pub view: Option<DraftView>,
    /// Another device saved after this one started: theirs stays, and this
    /// one is the recoverable copy.
    pub kept_newer: bool,
}

/// Saves `account`'s draft in a conversation they may read (the caller has
/// checked), or clears it when `draft` is None. `saved_at` is when the
/// writer's device saved it; a draft already newer than that stays, and
/// this one is kept as its recoverable copy.
pub async fn save(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    reach: Reach,
    conversation_id: Uuid,
    draft: Option<&Value>,
    saved_at: Option<DateTime<Utc>>,
) -> Result<Saved, AppError> {
    // The writer's other devices hear of it (their id only) and read again.
    let space_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        -- visibility: the caller checked the conversation is readable.
        SELECT project_id FROM conversations WHERE id = $1
        "#,
    )
    .bind(conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
    crate::outbox::enqueue_typed(
        transaction,
        space_id,
        crate::outbox::OutboxEventPayload::MessageDraftChanged(
            crate::outbox::MessageDraftChangedPayload {
                actor_id: account_id,
            },
        ),
    )
    .await?;
    let Some(draft) = draft else {
        // A clear dated before a draft another device saved since leaves
        // that draft alone, as an older save would.
        sqlx::query(
            "DELETE FROM message_drafts WHERE account_id = $1 AND conversation_id = $2 \
             AND updated_at <= LEAST(COALESCE($3::timestamptz, now()), now())",
        )
        .bind(account_id)
        .bind(conversation_id)
        .bind(saved_at)
        .execute(&mut **transaction)
        .await?;
        // The conversation was checked against the caller already.
        let kept = listed(transaction, account_id, reach, None, Some(conversation_id))
            .await?
            .into_iter()
            .next();
        return Ok(Saved {
            kept_newer: kept.is_some(),
            view: kept,
        });
    };
    // One statement, so two devices saving at once can't both miss the
    // other's row: a draft already newer than this one stays, and this one
    // becomes its recoverable copy.
    let newer = sqlx::query_scalar::<_, bool>(
        r#"
        INSERT INTO message_drafts (
            workspace_id, space_id, account_id, conversation_id, content, updated_at
        )
        -- visibility: the caller checked the conversation is readable.
        -- A draft is dated by when its device saved it (never later than
        -- now), so saves compare in one time domain.
        SELECT project.organization_id, project.id, $1, conversation.id, $3,
               LEAST(COALESCE($4::timestamptz, now()), now())
        FROM conversations conversation
        JOIN projects project ON project.id = conversation.project_id
        WHERE conversation.id = $2
        ON CONFLICT (account_id, conversation_id) DO UPDATE
        SET previous_content = CASE
                WHEN message_drafts.updated_at > EXCLUDED.updated_at THEN EXCLUDED.content
                WHEN message_drafts.content = EXCLUDED.content
                THEN message_drafts.previous_content
                ELSE message_drafts.content END,
            previous_updated_at = CASE
                WHEN message_drafts.updated_at > EXCLUDED.updated_at THEN now()
                WHEN message_drafts.content = EXCLUDED.content
                THEN message_drafts.previous_updated_at
                ELSE message_drafts.updated_at END,
            content = CASE
                WHEN message_drafts.updated_at > EXCLUDED.updated_at THEN message_drafts.content
                ELSE EXCLUDED.content END,
            updated_at = GREATEST(message_drafts.updated_at, EXCLUDED.updated_at)
        RETURNING updated_at > LEAST(COALESCE($4::timestamptz, now()), now())
        "#,
    )
    .bind(account_id)
    .bind(conversation_id)
    .bind(draft)
    .bind(saved_at)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or(false);
    // The conversation was checked against the caller already.
    let view = listed(transaction, account_id, reach, None, Some(conversation_id))
        .await?
        .into_iter()
        .next();
    Ok(Saved {
        view,
        kept_newer: newer,
    })
}

/// What narrows a listing past the account's own access.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reach {
    /// An agent's connection, whose rules decide the spaces it reaches.
    pub connection_id: Option<Uuid>,
    /// The one space a project-scoped credential is held to.
    pub confined_space: Option<Uuid>,
}

/// `account`'s drafts in `workspace` (or in one conversation), newest
/// first, in conversations they can still read and in spaces the caller
/// reaches: an agent's connection's spaces, or the one space a scoped
/// credential is held to. A recoverable copy shows for a day.
pub async fn listed(
    connection: &mut PgConnection,
    account_id: Uuid,
    reach: Reach,
    workspace_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
) -> Result<Vec<DraftView>, AppError> {
    Ok(sqlx::query_as::<_, DraftView>(
        r#"
        SELECT draft.conversation_id, draft.space_id,
               conversation.kind AS conversation_kind,
               conversation.name AS conversation_name,
               draft.content AS draft, draft.updated_at,
               CASE WHEN draft.previous_updated_at > now() - interval '1 day'
                    THEN draft.previous_content END AS previous
        FROM message_drafts draft
        JOIN conversations conversation ON conversation.id = draft.conversation_id
        WHERE draft.account_id = $1
          AND ($2::uuid IS NULL OR draft.workspace_id = $2)
          AND ($3::uuid IS NULL OR draft.conversation_id = $3)
          AND actor_can_access_project($1, $5, draft.space_id)
          AND ($6::uuid IS NULL OR draft.space_id = $6)
          AND conversation_readable_by(draft.conversation_id, $1)
        ORDER BY draft.updated_at DESC
        LIMIT $4
        "#,
    )
    .bind(account_id)
    .bind(workspace_id)
    .bind(conversation_id)
    .bind(MAX_LISTED)
    .bind(reach.connection_id)
    .bind(reach.confined_space)
    .fetch_all(&mut *connection)
    .await?)
}

const PRUNE_KIND: &str = "message_draft.prune";

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register(PRUNE_KIND, |context| async move {
        prune(&context.state.pool).await?;
        Ok(())
    });
    registry.every(PRUNE_KIND, std::time::Duration::from_secs(60 * 60));
}

/// Forgets drafts nobody touched for 30 days, and recoverable copies older
/// than a day. Returns how many drafts went.
pub async fn prune(pool: &sqlx::PgPool) -> Result<u64, AppError> {
    let pruned = crate::db::scoped_one(pool, async |connection| {
        sqlx::query(
            r#"
            UPDATE message_drafts SET previous_content = NULL, previous_updated_at = NULL
            WHERE previous_updated_at < now() - interval '1 day'
            "#,
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query("DELETE FROM message_drafts WHERE updated_at < now() - interval '30 days'")
            .execute(&mut *connection)
            .await
    })
    .await?;
    Ok(pruned.rows_affected())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::checked;

    #[test]
    fn a_draft_is_an_object_with_text() {
        assert!(checked(&json!({"text": "Hello"})).expect("valid").is_some());
        assert!(checked(&json!({"text": "   "})).expect("valid").is_none());
        assert!(checked(&json!("Hello")).is_err());
        assert!(checked(&json!({"text": "x".repeat(70_000)})).is_err());
    }
}

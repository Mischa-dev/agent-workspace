//! The link store's reader (K3.14): one query over `links` and the thing
//! registry gives every link of a thing, both ways, whatever made it.
//!
//! What the reader says, by what is stored:
//! - containment (`things.parent_id`): a subtask is part of its task;
//! - `blocks`, `relates_to`, `duplicates` and `made_from` as stored;
//! - an explicit or derived `about` as stored;
//! - a link found in text (`mentions`) or held in a link field reads as
//!   `about`, the words "Referenced in" use; an `embeds` stays an embed;
//! - a link from a message reads as one from its conversation, which is
//!   what a reader can open.
//!
//! Ends that are deleted (a removed message, a deleted task) don't count,
//! nor do archived subtasks or archived records' field values. Nothing
//! here checks the reader's access: the caller resolves each other end
//! with the reader's access and drops what they can't see.

use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::error::AppError;

/// One link of a thing, before the other end is checked for the reader.
#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct StoredLink {
    /// `part_of`, `blocks`, `about`, `made_from`, `relates_to`,
    /// `duplicates` or `embeds`.
    pub link: String,
    /// `outgoing` (this thing links to the other) or `incoming`.
    pub direction: String,
    /// The other end's kind in the registry.
    pub other_kind: String,
    pub other_id: Uuid,
}

const FOR_THING: &str = r#"
    WITH me AS (SELECT id, kind::text AS kind, parent_id FROM things WHERE id = $1)
    SELECT 'part_of' AS link, 'outgoing' AS direction, parent.kind::text AS other_kind,
           parent.id AS other_id
    FROM me JOIN things parent ON parent.id = me.parent_id
    WHERE me.kind = 'task' AND parent.kind = 'task' AND parent.deleted_at IS NULL
    UNION ALL
    SELECT 'part_of', 'incoming', child.kind::text, child.id
    FROM me JOIN things child ON child.parent_id = me.id
    WHERE me.kind = 'task' AND child.kind = 'task'
      AND child.archived_at IS NULL AND child.deleted_at IS NULL
    UNION ALL
    SELECT CASE WHEN link.kind = 'mentions' OR link.origin = 'field' THEN 'about'
                ELSE link.kind END,
           'outgoing', other.kind::text, other.id
    FROM links link
    JOIN things other ON other.id = link.to_thing_id
    WHERE link.from_thing_id = $1 AND other.deleted_at IS NULL
    UNION ALL
    SELECT CASE WHEN link.kind = 'mentions' OR link.origin = 'field' THEN 'about'
                ELSE link.kind END,
           'incoming', holder.kind::text, holder.id
    FROM links link
    JOIN things source ON source.id = link.from_thing_id
    JOIN things holder
      ON holder.id = CASE WHEN source.kind = 'message' THEN source.parent_id ELSE source.id END
    WHERE link.to_thing_id = $1
      AND source.deleted_at IS NULL AND holder.deleted_at IS NULL
      AND NOT (link.origin = 'field' AND source.archived_at IS NOT NULL)
      AND holder.id <> $1
"#;

/// Every link of one thing, both ways, deduplicated (the same kind,
/// direction and other end once), at most `limit`.
pub async fn for_thing(
    connection: &mut PgConnection,
    thing: Uuid,
    limit: usize,
) -> Result<Vec<StoredLink>, AppError> {
    let rows = sqlx::query_as::<_, StoredLink>(FOR_THING)
        .bind(thing)
        .fetch_all(&mut *connection)
        .await?;
    let mut seen = std::collections::BTreeSet::new();
    Ok(rows
        .into_iter()
        .filter(|row| seen.insert((row.link.clone(), row.direction.clone(), row.other_id)))
        .take(limit)
        .collect())
}

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
//! Explicit links (someone linked two things: a dependency, a relation)
//! are written here too, with `put_explicit` and `drop_explicit`, which
//! emit `link.created` and `link.deleted` on the `from` thing (K3.15).
//!
//! Ends that are deleted (a removed message, a deleted task) don't count,
//! nor do archived subtasks or archived records' field values. Nothing
//! here checks the reader's access: the caller resolves each other end
//! with the reader's access and drops what they can't see.

pub mod ops;

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

/// Why an explicit link can't be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// One end isn't a thing (yet, or any more).
    NotThings,
    /// The ends are in different workspaces.
    OtherWorkspace,
}

#[derive(FromRow)]
struct Ends {
    workspace_id: Uuid,
    other_workspace_id: Uuid,
    space_id: Option<Uuid>,
    kind: String,
}

async fn ends(
    connection: &mut PgConnection,
    from: Uuid,
    to: Uuid,
) -> Result<Option<Ends>, AppError> {
    Ok(sqlx::query_as::<_, Ends>(
        "SELECT source.workspace_id, target.workspace_id AS other_workspace_id, \
                source.space_id, source.kind::text AS kind \
         FROM things source, things target WHERE source.id = $1 AND target.id = $2",
    )
    .bind(from)
    .bind(to)
    .fetch_optional(&mut *connection)
    .await?)
}

async fn emit(
    connection: &mut PgConnection,
    ty: &'static crate::events::EventType,
    ends: &Ends,
    from: Uuid,
    payload: serde_json::Value,
) -> Result<(), AppError> {
    use crate::events::NewEvent;
    let event = match ends.space_id {
        Some(space) => NewEvent::thing(ty, space, &ends.kind, from),
        None => NewEvent {
            space_id: None,
            ..NewEvent::thing(ty, Uuid::nil(), &ends.kind, from)
        },
    }
    .workspace(ends.workspace_id)
    .payload(payload);
    crate::events::emit(connection, event).await?;
    Ok(())
}

/// Link `from` to `to` with `kind`, as someone's own act. `Ok(Ok(true))`
/// when the link is new, `Ok(Ok(false))` when it was there already.
pub async fn put_explicit(
    connection: &mut PgConnection,
    from: Uuid,
    to: Uuid,
    kind: &str,
    by: Option<Uuid>,
) -> Result<Result<bool, Refused>, AppError> {
    let Some(ends) = ends(connection, from, to).await? else {
        return Ok(Err(Refused::NotThings));
    };
    // A DM or group lives in the direct scope: a link between it and a
    // workspace's thing belongs to that workspace.
    let direct = crate::spaces::DIRECT;
    if ends.workspace_id != ends.other_workspace_id
        && ends.workspace_id != direct
        && ends.other_workspace_id != direct
    {
        return Ok(Err(Refused::OtherWorkspace));
    }
    let ends = if ends.workspace_id == direct {
        Ends {
            workspace_id: ends.other_workspace_id,
            ..ends
        }
    } else {
        ends
    };
    let made = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO links (workspace_id, from_thing_id, to_thing_id, kind, origin, \
                            created_by_account_id) \
         VALUES ($1, $2, $3, $4, 'explicit', (SELECT id FROM actors WHERE id = $5)) \
         ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(ends.workspace_id)
    .bind(from)
    .bind(to)
    .bind(kind)
    .bind(by)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(link) = made else {
        return Ok(Ok(false));
    };
    emit(
        connection,
        &crate::events::catalog::LINK_CREATED,
        &ends,
        from,
        serde_json::json!({ "linkId": link, "kind": kind, "toThingId": to }),
    )
    .await?;
    Ok(Ok(true))
}

/// Take away an explicit link of `kind` from `from` to `to` (either way
/// when `either_way`, for symmetric kinds). Whether one went.
pub async fn drop_explicit(
    connection: &mut PgConnection,
    from: Uuid,
    to: Uuid,
    kind: &str,
    either_way: bool,
) -> Result<bool, AppError> {
    let gone = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        "DELETE FROM links \
         WHERE origin = 'explicit' AND kind = $3 \
           AND ((from_thing_id = $1 AND to_thing_id = $2) \
                OR ($4 AND from_thing_id = $2 AND to_thing_id = $1)) \
         RETURNING id, from_thing_id, to_thing_id",
    )
    .bind(from)
    .bind(to)
    .bind(kind)
    .bind(either_way)
    .fetch_all(&mut *connection)
    .await?;
    for (link, source, target) in &gone {
        if let Some(ends) = ends(connection, *source, *target).await? {
            emit(
                connection,
                &crate::events::catalog::LINK_DELETED,
                &ends,
                *source,
                serde_json::json!({ "linkId": link, "kind": kind, "toThingId": target }),
            )
            .await?;
        }
    }
    Ok(!gone.is_empty())
}

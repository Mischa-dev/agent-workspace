//! Change sets (C1.9): every write to a document records what it touched,
//! by lasting id, and what it replaced, so the editor can outline recent
//! edits and anyone who edits the thing can undo one in a step.
//!
//! Pages write theirs from `page_docs::store::save` and
//! `memory::update_page`; the block diff and the undo itself are in
//! `page_docs::change_sets`. An undo is a change set of its own
//! (`undoes_change_set_id`), so it can be undone too. Sheets, boards and
//! records join through their own write paths.

pub mod ops;

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    access,
    error::AppError,
    memory::Writer,
    page_docs::{
        change_sets::{self as page_changes, Inverse},
        store, ydoc,
    },
};

/// How long a writer's typing in the editor keeps joining one change set.
const JOIN_SECONDS: i32 = 60;

/// Records that `writer` changed a page from `before` to `after` (blocks
/// with ids), taking it from `versions.0` to `versions.1`. With `join`
/// (typing in the editor), it joins the writer's change set still open on
/// the page. A change that touched no block (a title, a flag) records none.
#[allow(clippy::too_many_arguments)]
pub async fn record_page(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    space_id: Uuid,
    page_id: Uuid,
    versions: (i64, i64),
    before: &[crate::page_docs::tree::Block],
    after: &[crate::page_docs::tree::Block],
    join: bool,
) -> Result<Option<Uuid>, AppError> {
    let inverse = page_changes::diff(before, after);
    if inverse.blocks.is_empty() {
        return Ok(None);
    }
    if join
        && let Some((id, sqlx::types::Json(open))) =
            sqlx::query_as::<_, (Uuid, sqlx::types::Json<Inverse>)>(
                r#"
                SELECT id, inverse FROM document_change_sets
                WHERE thing_kind = 'page' AND thing_id = $1 AND actor_id = $2
                  AND version_to = $3 AND open_until > now() AND inverse IS NOT NULL
                  AND undone_by_change_set_id IS NULL
                  AND via_oauth_grant_id IS NOT DISTINCT FROM $4
                ORDER BY created_at DESC LIMIT 1
                FOR UPDATE
                "#,
            )
            .bind(page_id)
            .bind(writer.actor_id)
            .bind(versions.0)
            .bind(writer.via_grant_id)
            .fetch_optional(&mut **transaction)
            .await?
    {
        let joined = page_changes::join(open, inverse);
        sqlx::query(
            r#"
            UPDATE document_change_sets
            SET version_to = $2, touched = $3, inverse = $4, updated_at = now(),
                open_until = now() + make_interval(secs => $5)
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(versions.1)
        .bind(joined.touched())
        .bind(joined.keepable().then_some(sqlx::types::Json(&joined)))
        .bind(JOIN_SECONDS)
        .execute(&mut **transaction)
        .await?;
        return Ok(Some(id));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO document_change_sets (
            id, workspace_id, space_id, thing_kind, thing_id, actor_id, via_oauth_grant_id,
            version_from, version_to, touched, inverse, open_until
        )
        SELECT $1, project.organization_id, project.id, 'page', $3, $4, $5, $6, $7, $8, $9,
               CASE WHEN $10 THEN now() + make_interval(secs => $11) END
        FROM projects project WHERE project.id = $2
        "#,
    )
    .bind(id)
    .bind(space_id)
    .bind(page_id)
    .bind(writer.actor_id)
    .bind(writer.via_grant_id)
    .bind(versions.0)
    .bind(versions.1)
    .bind(inverse.touched())
    .bind(inverse.keepable().then_some(sqlx::types::Json(&inverse)))
    .bind(join)
    .bind(JOIN_SECONDS)
    .execute(&mut **transaction)
    .await?;
    Ok(Some(id))
}

/// The change set a write to `thing` made at `version`, if it recorded one.
pub async fn at_version(
    connection: &mut PgConnection,
    thing_kind: &str,
    thing_id: Uuid,
    version: i64,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM document_change_sets \
         WHERE thing_kind = $1 AND thing_id = $2 AND version_to = $3",
    )
    .bind(thing_kind)
    .bind(thing_id)
    .bind(version)
    .fetch_optional(connection)
    .await?)
}

/// A change set as it's listed.
#[derive(Clone, Debug, Serialize, FromRow, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSetView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub thing_kind: String,
    #[schemars(with = "String")]
    pub thing_id: Uuid,
    #[schemars(with = "String")]
    pub actor_id: Uuid,
    /// Who made it, as the space shows them.
    pub actor: Option<String>,
    pub version_from: i64,
    pub version_to: i64,
    /// What it touched, by lasting id: `{"blocks": [...]}` for a page.
    #[schemars(with = "serde_json::Value")]
    pub touched: sqlx::types::Json<serde_json::Value>,
    /// Whether it can be undone in a step (what it replaced was kept and
    /// it hasn't been undone).
    pub undoable: bool,
    #[schemars(with = "Option<String>")]
    pub undoes: Option<Uuid>,
    #[schemars(with = "Option<String>")]
    pub undone_by: Option<Uuid>,
    #[schemars(with = "Option<String>")]
    pub proposal_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
}

const SELECT: &str = r#"
    SELECT change.id, change.thing_kind, change.thing_id, change.actor_id,
           member.display_name AS actor, change.version_from, change.version_to,
           change.touched,
           (change.inverse IS NOT NULL AND change.undone_by_change_set_id IS NULL) AS undoable,
           change.undoes_change_set_id AS undoes, change.undone_by_change_set_id AS undone_by,
           change.proposal_id,
           to_char(change.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
           to_char(change.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at
    FROM document_change_sets change
    LEFT JOIN project_memberships member
      ON member.project_id = change.space_id AND member.actor_id = change.actor_id
"#;

/// A thing's change sets, newest first, if the viewer can see it.
pub async fn list(
    connection: &mut PgConnection,
    viewer: Uuid,
    thing_kind: &str,
    thing_id: Uuid,
    since_hours: i32,
    limit: i64,
) -> Result<Vec<ChangeSetView>, AppError> {
    Ok(sqlx::query_as::<_, ChangeSetView>(&format!(
        "{SELECT}
         WHERE change.thing_kind = $1 AND change.thing_id = $2
           AND change.updated_at > now() - make_interval(hours => $3)
           AND subject_visible_to(change.thing_kind, change.thing_id, $4)
         ORDER BY change.created_at DESC, change.id DESC LIMIT $5"
    ))
    .bind(thing_kind)
    .bind(thing_id)
    .bind(since_hours)
    .bind(viewer)
    .bind(limit)
    .fetch_all(connection)
    .await?)
}

pub async fn get(
    connection: &mut PgConnection,
    viewer: Uuid,
    id: Uuid,
) -> Result<Option<ChangeSetView>, AppError> {
    Ok(sqlx::query_as::<_, ChangeSetView>(&format!(
        "{SELECT} WHERE change.id = $1 \
         AND subject_visible_to(change.thing_kind, change.thing_id, $2)"
    ))
    .bind(id)
    .bind(viewer)
    .fetch_optional(connection)
    .await?)
}

/// Why an undo wasn't done.
#[derive(Debug)]
pub enum Refusal {
    NotFound,
    /// Undo needs edit on the thing.
    NotYours,
    AlreadyUndone,
    /// What it replaced was too large to keep: restore a version instead.
    NotKept {
        version_from: i64,
    },
    Archived,
    /// Everything it changed has been changed again since.
    NothingLeft,
}

/// What an undo did.
pub struct Undid {
    pub change_set_id: Uuid,
    pub version: i64,
    /// The touched items left as they are because they changed since.
    pub skipped: Vec<String>,
}

/// Undoes change set `id` as `caller`: puts back what it replaced for the
/// items nobody has changed since, as a new change set of the caller's.
pub async fn undo(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &access::Caller,
    writer: Writer,
    id: Uuid,
) -> Result<Result<Undid, Refusal>, AppError> {
    let Some((kind, thing_id, space_id, inverse, undone_by, version_from)) = sqlx::query_as::<
        _,
        (
            String,
            Uuid,
            Uuid,
            Option<sqlx::types::Json<Inverse>>,
            Option<Uuid>,
            i64,
        ),
    >(
        "SELECT thing_kind, thing_id, space_id, inverse, undone_by_change_set_id, version_from \
             FROM document_change_sets WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    if kind != "page" {
        return Ok(Err(Refusal::NotFound));
    }
    let creator = crate::page_docs::suggestions::page_creator(transaction, thing_id).await?;
    let may_edit = access::write_mode(
        transaction,
        caller,
        "page",
        space_id,
        creator == Some(caller.account_id),
        Some("pages"),
    )
    .await?
    .is_ok_and(|grant| grant.mode == access::WriteMode::Direct);
    if !may_edit {
        return Ok(Err(Refusal::NotYours));
    }
    if undone_by.is_some() {
        return Ok(Err(Refusal::AlreadyUndone));
    }
    let Some(sqlx::types::Json(inverse)) = inverse else {
        return Ok(Err(Refusal::NotKept { version_from }));
    };
    let Some(page) = store::lock_page(transaction, thing_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    if page.archived {
        return Ok(Err(Refusal::Archived));
    }
    let stored = store::document(transaction, thing_id, &page).await?;
    let current = store::read_blocks(&stored.state)?;
    let undone = page_changes::undo(&current, &inverse);
    if undone.applied == 0 {
        return Ok(Err(Refusal::NothingLeft));
    }
    let sent: Vec<&str> = inverse
        .blocks
        .iter()
        .filter_map(|entry| entry.before.as_deref())
        .collect();
    let change = store::change(&stored.state, &page.body, &sent, |doc, _| {
        ydoc::replace(doc, &undone.blocks);
        Ok(())
    })?;
    let (version, _) = store::save(
        transaction,
        writer,
        thing_id,
        &page,
        &change,
        Some("Undid a change"),
        store::History::Version,
    )
    .await?;
    let Some(undo_id) = at_version(transaction, "page", thing_id, version).await? else {
        return Ok(Err(Refusal::NothingLeft));
    };
    sqlx::query(
        "UPDATE document_change_sets SET undoes_change_set_id = $2, open_until = NULL WHERE id = $1",
    )
    .bind(undo_id)
    .bind(id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE document_change_sets SET undone_by_change_set_id = $2, open_until = NULL, \
         updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(undo_id)
    .execute(&mut **transaction)
    .await?;
    Ok(Ok(Undid {
        change_set_id: undo_id,
        version,
        skipped: undone.skipped,
    }))
}

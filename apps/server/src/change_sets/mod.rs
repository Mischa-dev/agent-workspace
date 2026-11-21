//! Change sets (C1.9): every write to a document records what it touched,
//! by lasting id, and what it replaced, so the editor can outline recent
//! edits and anyone who edits the thing can undo one in a step.
//!
//! Pages write theirs from `page_docs::store::save` and
//! `memory::update_page`; the block diff and the undo itself are in
//! `page_docs::change_sets`. An undo is a change set of its own
//! (`undoes_change_set_id`), so it can be undone too. Sheets record theirs
//! from `sheets::apply` (the cells whose input changed, and what each held)
//! and boards from `whiteboards::store::write_elements` (the elements, and
//! their stored copies before); records join through their own write path.

pub mod ops;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

/// The most items a change set names as touched; the rest are counted.
const MAX_TOUCHED: usize = 2_000;

/// The most a change set keeps of what it replaced (the column allows a
/// little more).
const MAX_INVERSE_BYTES: usize = 1_000_000;

/// A sheet cell a change set touched, and its input before (none when it
/// was empty).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CellBefore {
    pub tab_id: Uuid,
    pub row: i32,
    pub column: i32,
    pub before: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SheetInverse {
    cells: Vec<CellBefore>,
}

/// A board element a change set touched: its stored copy before (none for
/// one it added) and the version and nonce the change left it at. An edit
/// can win a tie at the same version on its nonce, so both say whether the
/// element is still as the change left it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementBefore {
    pub id: String,
    pub before: Option<Value>,
    pub version: i32,
    /// Missing on change sets kept before the nonce was.
    #[serde(default)]
    pub nonce: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct BoardInverse {
    elements: Vec<ElementBefore>,
}

/// What was replaced, if it is small enough to keep.
fn keep<T: Serialize>(inverse: &T) -> Option<Value> {
    serde_json::to_value(inverse)
        .ok()
        .filter(|value| value.to_string().len() <= MAX_INVERSE_BYTES)
}

/// Writes a new change set; `open_seconds` lets the writer's next edits
/// join it.
#[allow(clippy::too_many_arguments)]
async fn insert(
    connection: &mut PgConnection,
    kind: &str,
    space_id: Uuid,
    thing_id: Uuid,
    actor_id: Uuid,
    versions: (i64, i64),
    touched: Value,
    inverse: Option<Value>,
    open_seconds: Option<i32>,
) -> Result<Uuid, AppError> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO document_change_sets (
            id, workspace_id, space_id, thing_kind, thing_id, actor_id,
            version_from, version_to, touched, inverse, open_until
        )
        SELECT $1, project.organization_id, project.id, $3, $4, $5, $6, $7, $8, $9,
               now() + make_interval(secs => $10)
        FROM projects project WHERE project.id = $2
        "#,
    )
    .bind(id)
    .bind(space_id)
    .bind(kind)
    .bind(thing_id)
    .bind(actor_id)
    .bind(versions.0)
    .bind(versions.1)
    .bind(touched)
    .bind(inverse)
    .bind(open_seconds)
    .execute(connection)
    .await?;
    Ok(id)
}

/// Records that `actor_id` changed the inputs of `cells` on a sheet,
/// taking it from revision `versions.0` to `versions.1`. `undoable` is
/// false when the write also moved rows, columns or tabs, so the cells'
/// places no longer say what they held: such a change is outlined, and
/// undone by restoring the revision before it.
pub async fn record_sheet(
    connection: &mut PgConnection,
    space_id: Uuid,
    sheet_id: Uuid,
    actor_id: Uuid,
    versions: (i64, i64),
    cells: Vec<CellBefore>,
    undoable: bool,
) -> Result<(), AppError> {
    if cells.is_empty() {
        return Ok(());
    }
    let touched = json!({
        "cells": cells
            .iter()
            .take(MAX_TOUCHED)
            .map(|cell| json!({"tabId": cell.tab_id, "row": cell.row, "column": cell.column}))
            .collect::<Vec<_>>(),
        "more": cells.len().saturating_sub(MAX_TOUCHED),
    });
    let inverse = if undoable {
        keep(&SheetInverse { cells })
    } else {
        None
    };
    insert(
        connection, "sheet", space_id, sheet_id, actor_id, versions, touched, inverse, None,
    )
    .await?;
    Ok(())
}

fn board_touched(elements: &[ElementBefore]) -> Value {
    json!({
        "elements": elements
            .iter()
            .take(MAX_TOUCHED)
            .map(|element| element.id.as_str())
            .collect::<Vec<_>>(),
        "more": elements.len().saturating_sub(MAX_TOUCHED),
    })
}

/// Records that `actor_id` wrote `elements` to a board, taking its change
/// counter from `versions.0` to `versions.1`. Drawing in the editor sends
/// many small writes, so with `join` they join the writer's change set
/// still open on the board. A board kept outside any space records none.
pub async fn record_board(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    via_grant_id: Option<Uuid>,
    board_id: Uuid,
    versions: (i64, i64),
    elements: Vec<ElementBefore>,
    join: bool,
) -> Result<(), AppError> {
    if elements.is_empty() {
        return Ok(());
    }
    let Some(space_id) =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT project_id FROM whiteboards WHERE id = $1")
            .bind(board_id)
            .fetch_one(&mut **transaction)
            .await?
    else {
        return Ok(());
    };
    if join
        && let Some((id, sqlx::types::Json(open))) =
            sqlx::query_as::<_, (Uuid, sqlx::types::Json<BoardInverse>)>(
                r#"
            SELECT id, inverse FROM document_change_sets
            WHERE thing_kind = 'whiteboard' AND thing_id = $1 AND actor_id = $2
              AND version_to = $3 AND open_until > now() AND inverse IS NOT NULL
              AND undone_by_change_set_id IS NULL
              AND via_oauth_grant_id IS NOT DISTINCT FROM $4
            ORDER BY created_at DESC LIMIT 1
            FOR UPDATE
            "#,
            )
            .bind(board_id)
            .bind(actor_id)
            .bind(versions.0)
            .bind(via_grant_id)
            .fetch_optional(&mut **transaction)
            .await?
    {
        // An element drawn again keeps what it was before the first write.
        let mut joined = open.elements;
        for element in elements {
            match joined.iter_mut().find(|known| known.id == element.id) {
                Some(known) => {
                    known.version = element.version;
                    known.nonce = element.nonce;
                }
                None => joined.push(element),
            }
        }
        let touched = board_touched(&joined);
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
        .bind(touched)
        .bind(keep(&BoardInverse { elements: joined }))
        .bind(JOIN_SECONDS)
        .execute(&mut **transaction)
        .await?;
        return Ok(());
    }
    let touched = board_touched(&elements);
    insert(
        transaction,
        "whiteboard",
        space_id,
        board_id,
        actor_id,
        versions,
        touched,
        keep(&BoardInverse { elements }),
        join.then_some(JOIN_SECONDS),
    )
    .await?;
    Ok(())
}

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
    /// What it touched, by lasting id: `{"blocks": [...]}` for a page,
    /// `{"cells": [{tabId, row, column}], "more": n}` for a sheet and
    /// `{"elements": [...], "more": n}` for a board.
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
    /// Putting it back was refused, in words for the caller.
    Refused(String),
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
/// `board_caller` is the same caller as a board's write path takes it.
pub async fn undo(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &access::Caller,
    board_caller: &crate::whiteboards::Caller,
    writer: Writer,
    id: Uuid,
) -> Result<Result<Undid, Refusal>, AppError> {
    let Some((kind, thing_id, space_id, inverse, undone_by, version_from, version_to)) =
        sqlx::query_as::<
            _,
            (
                String,
                Uuid,
                Uuid,
                Option<sqlx::types::Json<Value>>,
                Option<Uuid>,
                i64,
                i64,
            ),
        >(
            "SELECT thing_kind, thing_id, space_id, inverse, undone_by_change_set_id, \
                    version_from, version_to \
             FROM document_change_sets WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    let (creator, module) = match kind.as_str() {
        "page" => (
            crate::page_docs::suggestions::page_creator(transaction, thing_id).await?,
            "pages",
        ),
        "sheet" => (
            sqlx::query_scalar::<_, Option<Uuid>>(
                "SELECT created_by_actor_id FROM sheets WHERE id = $1",
            )
            .bind(thing_id)
            .fetch_optional(&mut **transaction)
            .await?
            .flatten(),
            "sheets",
        ),
        "whiteboard" => (
            sqlx::query_scalar::<_, Uuid>(
                "SELECT created_by_actor_id FROM whiteboards WHERE id = $1",
            )
            .bind(thing_id)
            .fetch_optional(&mut **transaction)
            .await?,
            "whiteboards",
        ),
        _ => return Ok(Err(Refusal::NotFound)),
    };
    let may_edit = access::write_mode(
        transaction,
        caller,
        &kind,
        space_id,
        creator == Some(caller.account_id),
        Some(module),
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
    // The undo is a change of its own: it never joins one still open.
    sqlx::query(
        "UPDATE document_change_sets SET open_until = NULL \
         WHERE thing_kind = $1 AND thing_id = $2 AND open_until IS NOT NULL",
    )
    .bind(&kind)
    .bind(thing_id)
    .execute(&mut **transaction)
    .await?;
    let done = match kind.as_str() {
        "page" => undo_page(transaction, writer, thing_id, inverse).await?,
        "sheet" => undo_sheet(transaction, writer, space_id, thing_id, inverse, version_to).await?,
        _ => undo_board(transaction, board_caller, thing_id, inverse).await?,
    };
    let (version, skipped) = match done {
        Ok(done) => done,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let Some(undo_id) = at_version(transaction, &kind, thing_id, version).await? else {
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
    crate::events::emit(
        transaction,
        crate::events::NewEvent::thing(
            &crate::events::catalog::CHANGE_UNDONE,
            space_id,
            &kind,
            thing_id,
        )
        .actor(writer.actor_id)
        .version(version)
        .payload(json!({"changeSetId": id, "undoneBy": undo_id})),
    )
    .await?;
    Ok(Ok(Undid {
        change_set_id: undo_id,
        version,
        skipped,
    }))
}

/// The thing's version after an undo, and the items it left as they are.
type Done = Result<(i64, Vec<String>), Refusal>;

async fn undo_page(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    page_id: Uuid,
    inverse: Value,
) -> Result<Done, AppError> {
    let inverse: Inverse =
        serde_json::from_value(inverse).map_err(|_| AppError::Internal("page change set"))?;
    let Some(page) = store::lock_page(transaction, page_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    if page.archived {
        return Ok(Err(Refusal::Archived));
    }
    let stored = store::document(transaction, page_id, &page).await?;
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
        page_id,
        &page,
        &change,
        Some("Undid a change"),
        store::History::Version,
    )
    .await?;
    Ok(Ok((version, undone.skipped)))
}

/// Puts back the inputs of the cells nobody has changed since: a cell
/// still at the change's revision (or still cleared by it), on a tab whose
/// rows and columns haven't moved since.
async fn undo_sheet(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    space_id: Uuid,
    sheet_id: Uuid,
    inverse: Value,
    version_to: i64,
) -> Result<Done, AppError> {
    use crate::sheets::{Change, address::column_name};
    let SheetInverse { cells } =
        serde_json::from_value(inverse).map_err(|_| AppError::Internal("sheet change set"))?;
    let Some(sheet) = crate::sheets::lock(transaction, space_id, sheet_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    if sheet.archived {
        return Ok(Err(Refusal::Archived));
    }
    let tabs = cells.iter().map(|cell| cell.tab_id).collect::<Vec<_>>();
    let rows = cells.iter().map(|cell| cell.row).collect::<Vec<_>>();
    let columns = cells.iter().map(|cell| cell.column).collect::<Vec<_>>();
    let states = sqlx::query_as::<_, (Option<String>, Option<i64>, Option<i64>, Option<i64>)>(
        r#"
        SELECT tab.name, tab.structure_revision, cell.revision, cleared.revision
        FROM UNNEST($1::uuid[], $2::int[], $3::int[]) WITH ORDINALITY
             AS wanted(tab_id, row_number, column_number, ord)
        LEFT JOIN sheet_tabs tab ON tab.id = wanted.tab_id AND tab.sheet_id = $4
        LEFT JOIN sheet_cells cell
          ON cell.tab_id = wanted.tab_id AND cell.row_number = wanted.row_number
         AND cell.column_number = wanted.column_number
        LEFT JOIN sheet_cleared_cells cleared
          ON cleared.tab_id = wanted.tab_id AND cleared.row_number = wanted.row_number
         AND cleared.column_number = wanted.column_number
        ORDER BY wanted.ord
        "#,
    )
    .bind(&tabs)
    .bind(&rows)
    .bind(&columns)
    .bind(sheet_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    for (cell, (tab, structure, revision, cleared)) in cells.into_iter().zip(states) {
        let unchanged = structure.is_some_and(|structure| structure <= version_to)
            && match revision {
                Some(revision) => revision == version_to,
                None => cleared == Some(version_to),
            };
        if unchanged {
            changes.push(Change::Cell {
                tab: Some(cell.tab_id.to_string()),
                row: cell.row,
                column: cell.column,
                input: cell.before.unwrap_or_default(),
                if_revision: None,
            });
        } else {
            skipped.push(format!(
                "{}!{}{}",
                tab.as_deref().unwrap_or("a removed tab"),
                column_name(cell.column),
                cell.row
            ));
        }
    }
    if changes.is_empty() {
        return Ok(Err(Refusal::NothingLeft));
    }
    let applied =
        match crate::sheets::apply_restoring(transaction, &sheet, writer.actor_id, &changes).await?
        {
            Ok(applied) => applied,
            Err(problem) => return Ok(Err(Refusal::Refused(problem.message))),
        };
    if applied.revision == sheet.revision {
        return Ok(Err(Refusal::NothingLeft));
    }
    crate::outbox::enqueue_sheet_updated(transaction, space_id, sheet_id, applied.revision).await?;
    Ok(Ok((applied.revision, skipped)))
}

/// Puts back the elements nobody has changed since (still at the version
/// the change left them): each takes its copy from before, and one the
/// change added is deleted.
async fn undo_board(
    transaction: &mut Transaction<'_, Postgres>,
    board_caller: &crate::whiteboards::Caller,
    board_id: Uuid,
    inverse: Value,
) -> Result<Done, AppError> {
    use crate::whiteboards::{elements, store as boards};
    let BoardInverse { elements: entries } =
        serde_json::from_value(inverse).map_err(|_| AppError::Internal("board change set"))?;
    let scene_seq = boards::lock(transaction, board_id).await?;
    let deleted = sqlx::query_scalar::<_, bool>(
        "SELECT deleted_at IS NOT NULL FROM whiteboards WHERE id = $1",
    )
    .bind(board_id)
    .fetch_one(&mut **transaction)
    .await?;
    if deleted {
        return Ok(Err(Refusal::Archived));
    }
    let ids = entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    let now = sqlx::query_as::<_, (String, i32, i64, Value)>(
        "SELECT element_id, version, version_nonce, data FROM whiteboard_elements \
         WHERE whiteboard_id = $1 AND element_id = ANY($2)",
    )
    .bind(board_id)
    .bind(&ids)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|(id, version, nonce, data)| (id, (version, nonce, data)))
    .collect::<std::collections::HashMap<_, _>>();
    let mut writes = Vec::new();
    let mut skipped = Vec::new();
    for entry in entries {
        match now.get(&entry.id) {
            Some((version, nonce, data))
                if *version == entry.version && entry.nonce.is_none_or(|kept| kept == *nonce) =>
            {
                let mut copy = match entry.before {
                    Some(before) => before,
                    None => {
                        let mut added = data.clone();
                        added["isDeleted"] = json!(true);
                        added
                    }
                };
                copy["version"] = json!(version.saturating_add(1));
                copy["versionNonce"] = json!(i64::from(
                    rand_core::RngCore::next_u32(&mut rand_core::OsRng) >> 1
                ));
                match elements::parse(copy) {
                    Ok(element) => writes.push(element),
                    Err(_) => skipped.push(format!("element {}", entry.id)),
                }
            }
            _ => skipped.push(format!("element {}", entry.id)),
        }
    }
    if writes.is_empty() {
        return Ok(Err(Refusal::NothingLeft));
    }
    let saved = boards::save_revision(transaction, board_caller, board_id, false, None).await?;
    let outcome =
        boards::write_elements(transaction, board_caller, board_id, scene_seq, writes).await?;
    if outcome.scene_seq == scene_seq {
        return Ok(Err(Refusal::NothingLeft));
    }
    if let Some(revision) = saved {
        boards::updated(transaction, board_caller, board_id, revision).await?;
    }
    Ok(Ok((outcome.scene_seq, skipped)))
}

/// C1.22: whether `thread_id` is one of the page's comment threads that
/// `viewer` can see, so a change can say it answers it.
pub async fn is_page_thread(
    connection: &mut PgConnection,
    page_id: Uuid,
    thread_id: Uuid,
    viewer: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM conversations thread \
           JOIN conversations comments ON comments.id = thread.parent_channel_id \
          WHERE thread.id = $1 AND thread.kind = 'thread' AND comments.kind = 'comments' \
            AND comments.subject_kind = 'page' AND comments.subject_id = $2 \
            AND conversation_visible_to(thread.id, $3))",
    )
    .bind(thread_id)
    .bind(page_id)
    .bind(viewer)
    .fetch_one(connection)
    .await?)
}

/// C1.22: names `thread_id` as the thread that asked for what `actor` did
/// to the thing in this transaction: the change sets it wrote (or joined)
/// and any proposal it made. Returns how many it named.
pub async fn answer_thread(
    connection: &mut PgConnection,
    thing_kind: &str,
    thing_id: Uuid,
    actor: Uuid,
    thread_id: Uuid,
) -> Result<u64, AppError> {
    // now() is the transaction's start, so it picks out exactly the rows
    // this transaction wrote.
    let changes = sqlx::query(
        "UPDATE document_change_sets SET thread_id = $4 \
         WHERE thing_kind = $1 AND thing_id = $2 AND actor_id = $3 AND updated_at = now()",
    )
    .bind(thing_kind)
    .bind(thing_id)
    .bind(actor)
    .bind(thread_id)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    let proposals = sqlx::query(
        "UPDATE proposals SET thread_id = $4 \
         WHERE thing_kind = $1 AND thing_id = $2 AND author_actor_id = $3 AND created_at = now()",
    )
    .bind(thing_kind)
    .bind(thing_id)
    .bind(actor)
    .bind(thread_id)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    Ok(changes + proposals)
}

/// A change made because a comment thread asked (C1.22), as the thread
/// shows it.
#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ThreadChange {
    pub thread_id: Uuid,
    pub change_set_id: Uuid,
    pub actor: Option<String>,
    pub created_at: String,
    pub undone: bool,
}

/// The changes to a page that answered its comment threads, oldest first.
/// The caller has checked the reader may read the page.
pub async fn page_thread_changes(
    executor: impl sqlx::PgExecutor<'_>,
    page_id: Uuid,
) -> Result<Vec<ThreadChange>, AppError> {
    Ok(sqlx::query_as::<_, ThreadChange>(
        "SELECT change.thread_id, change.id AS change_set_id, member.display_name AS actor, \
                to_char(change.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') \
                  AS created_at, \
                change.undone_by_change_set_id IS NOT NULL AS undone \
         FROM document_change_sets change \
         LEFT JOIN project_memberships member \
           ON member.project_id = change.space_id AND member.actor_id = change.actor_id \
         WHERE change.thing_kind = 'page' AND change.thing_id = $1 \
           AND change.thread_id IS NOT NULL \
         ORDER BY change.created_at, change.id LIMIT 500",
    )
    .bind(page_id)
    .fetch_all(executor)
    .await?)
}

//! The Docs tree (AWD-223): a space's pages, sheets, whiteboards,
//! collections and dashboards in one tree of folders.
//!
//! `doc_folders` holds the folders and `doc_places` says where each
//! document or folder sits and whether it is restricted (migration
//! `20261127233000_docs_tree`). A thing with no place is at the top, open
//! to its space. A page inside a page sits under that page; its place
//! follows the page's own row.
//!
//! One rule decides who sees what, `doc_open_to` in the database: a
//! document is open to everyone in its space unless it, or something above
//! it, is restricted; then only to the people named on it, or on anything
//! between it and the nearest restricted thing above. Row rules on the
//! documents' tables hold it wherever they are read, so nothing here (or in
//! any list, search or lookup elsewhere) filters by hand.

pub mod ops;

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, ops::Perm};

/// Folders this deep and no deeper, so a tree stays readable.
pub const MAX_FOLDER_DEPTH: usize = 8;
/// The most entries one listing returns.
const MAX_ENTRIES: i64 = 5000;

/// What the tree holds.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    Folder,
    Page,
    Sheet,
    Whiteboard,
    Collection,
    Dashboard,
}

impl DocKind {
    pub const DOCUMENTS: [Self; 5] = [
        Self::Page,
        Self::Sheet,
        Self::Whiteboard,
        Self::Collection,
        Self::Dashboard,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Page => "page",
            Self::Sheet => "sheet",
            Self::Whiteboard => "whiteboard",
            Self::Collection => "collection",
            Self::Dashboard => "dashboard",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [Self::Folder]
            .into_iter()
            .chain(Self::DOCUMENTS)
            .find(|kind| kind.key() == key)
    }

    /// The manifest id of the module that makes it; a folder is the
    /// tree's own.
    pub const fn module(self) -> Option<&'static str> {
        match self {
            Self::Folder => None,
            Self::Page => Some(PAGES),
            Self::Sheet => Some(SHEETS),
            Self::Whiteboard => Some(WHITEBOARDS),
            Self::Collection => Some(COLLECTIONS),
            Self::Dashboard => Some(DASHBOARDS),
        }
    }

    /// The permission that changes it: moving it and saying who sees it
    /// need what editing it needs. A folder takes any of them.
    pub const fn edit(self) -> Option<Perm> {
        match self {
            Self::Folder => None,
            Self::Page => Some(Perm::PagesEdit),
            Self::Sheet => Some(Perm::SheetsEdit),
            Self::Whiteboard => Some(Perm::WhiteboardsEdit),
            Self::Collection => Some(Perm::CollectionsEdit),
            Self::Dashboard => Some(Perm::DashboardsEdit),
        }
    }

    /// The permission that lists it, where its kind has one of its own.
    pub const fn see(self) -> Option<Perm> {
        match self {
            Self::Page => Some(Perm::PagesSee),
            Self::Dashboard => Some(Perm::DashboardsView),
            _ => None,
        }
    }

    /// The tool that opens one, for whoever reads a listing.
    pub const fn opens_with(self) -> &'static str {
        match self {
            Self::Folder => "doc_list",
            Self::Page => "page_get",
            Self::Sheet => "sheet_get",
            Self::Whiteboard => "whiteboard_read",
            Self::Collection => "collection_get",
            Self::Dashboard => "dashboard_get",
        }
    }
}

const PAGES: &str = "pages";
const SHEETS: &str = "sheets";
const WHITEBOARDS: &str = "whiteboards";
const COLLECTIONS: &str = "collections";
const DASHBOARDS: &str = "dashboards";

/// Every folder and document of a space (`$1`) that is in the tree, for
/// the viewer `$2`: what each list of its kind shows. A conversation's
/// memory page, a template, a built-in collection and someone else's own
/// dashboard aren't documents of the tree.
const SOURCES: &str = r#"
    SELECT folder.id, 'folder' AS kind, folder.workspace_id, folder.space_id, folder.title,
           folder.updated_at
    FROM doc_folders folder
    WHERE folder.archived_at IS NULL
    UNION ALL
    SELECT page.id, 'page', page.workspace_id, page.project_id, page.title, page.updated_at
    FROM project_pages page
    WHERE page.archived_at IS NULL AND NOT page.is_template
      AND page.memory_for_kind IS DISTINCT FROM 'conversation'
      AND module_part_on('page', page.id) AND module_part_on('skill', page.id)
    UNION ALL
    SELECT sheet.id, 'sheet', sheet.workspace_id, sheet.project_id, sheet.name, sheet.updated_at
    FROM sheets sheet
    WHERE sheet.archived_at IS NULL
    UNION ALL
    SELECT board.id, 'whiteboard', board.organization_id, board.project_id, board.title,
           board.updated_at
    FROM whiteboards board
    WHERE board.deleted_at IS NULL AND NOT board.is_template
    UNION ALL
    SELECT collection.id, 'collection', collection.workspace_id, collection.project_id,
           collection.name, collection.updated_at
    FROM collections collection
    WHERE collection.archived_at IS NULL AND collection.builtin IS NULL
      AND collection.role IS NULL AND module_part_on('collection', collection.id)
    UNION ALL
    SELECT dashboard.id, 'dashboard', dashboard.workspace_id, dashboard.space_id,
           dashboard.name, dashboard.updated_at
    FROM dashboards dashboard
    WHERE dashboard.owner_actor_id IS NULL OR dashboard.owner_actor_id = $2
"#;

/// One folder or document in the tree.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocEntry {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub kind: DocKind,
    pub title: String,
    /// The folder it is in or, for a page inside a page, that page. Left
    /// out at the top of the tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub parent_id: Option<Uuid>,
    /// Only the people named on it, or on what it is inside, can see it.
    pub restricted: bool,
    /// The restriction is set on this one itself (doc_access_get says who).
    pub restricted_here: bool,
    #[schemars(with = "String")]
    pub updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct EntryRow {
    id: Uuid,
    kind: String,
    title: String,
    updated_at: DateTime<Utc>,
    parent_id: Option<Uuid>,
    restricted_here: bool,
    restricted: bool,
}

impl EntryRow {
    fn entry(self) -> Option<DocEntry> {
        Some(DocEntry {
            id: self.id,
            kind: DocKind::from_key(&self.kind)?,
            title: self.title,
            parent_id: self.parent_id,
            restricted: self.restricted,
            restricted_here: self.restricted_here,
            updated_at: self.updated_at,
        })
    }
}

/// The tree of `space` as `viewer` sees it, holding only `kinds`: folders
/// first, then by title. Something whose parent the viewer can't see (a
/// page named to them inside a page that isn't) shows at the top.
pub async fn entries(
    connection: &mut PgConnection,
    space: Uuid,
    viewer: Uuid,
    kinds: &[DocKind],
) -> Result<Vec<DocEntry>, AppError> {
    let keys = kinds.iter().map(|kind| kind.key()).collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, EntryRow>(&format!(
        "SELECT doc.id, doc.kind, doc.title, doc.updated_at, place.parent_id, \
                COALESCE(place.restricted, false) AS restricted_here, \
                NOT doc_open_to(doc.id, NULL) AS restricted \
         FROM ({SOURCES}) doc \
         LEFT JOIN doc_places place ON place.thing_id = doc.id \
         WHERE doc.space_id = $1 AND doc.kind = ANY($3) AND doc_open_to(doc.id, $2) \
         ORDER BY doc.kind <> 'folder', lower(doc.title), doc.id \
         LIMIT {MAX_ENTRIES}"
    ))
    .bind(space)
    .bind(viewer)
    .bind(&keys)
    .fetch_all(&mut *connection)
    .await?;
    let mut entries = rows
        .into_iter()
        .filter_map(EntryRow::entry)
        .collect::<Vec<_>>();
    let shown = entries
        .iter()
        .map(|entry| entry.id)
        .collect::<std::collections::HashSet<_>>();
    for entry in &mut entries {
        if entry
            .parent_id
            .is_some_and(|parent| !shown.contains(&parent))
        {
            entry.parent_id = None;
        }
    }
    Ok(entries)
}

/// A folder or document found by id.
#[derive(Clone, Debug)]
pub struct Found {
    pub id: Uuid,
    pub kind: DocKind,
    pub workspace_id: Uuid,
    pub space_id: Uuid,
    pub title: String,
    pub parent_id: Option<Uuid>,
    pub restricted_here: bool,
}

/// The folder or document `id` of the tree, if `viewer` may see it.
pub async fn find(
    connection: &mut PgConnection,
    id: Uuid,
    viewer: Uuid,
) -> Result<Option<Found>, AppError> {
    let row = sqlx::query_as::<_, (String, Uuid, Uuid, String, Option<Uuid>, bool)>(&format!(
        "SELECT doc.kind, doc.workspace_id, doc.space_id, doc.title, place.parent_id, \
                COALESCE(place.restricted, false) \
         FROM ({SOURCES}) doc \
         LEFT JOIN doc_places place ON place.thing_id = doc.id \
         WHERE doc.id = $1 AND doc_open_to(doc.id, $2)"
    ))
    .bind(id)
    .bind(viewer)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.and_then(
        |(kind, workspace_id, space_id, title, parent_id, restricted_here)| {
            Some(Found {
                id,
                kind: DocKind::from_key(&kind)?,
                workspace_id,
                space_id,
                title,
                parent_id,
                restricted_here,
            })
        },
    ))
}

/// Whether the Docs rule leaves `thing` open to `viewer`: for code that reads
/// for an account outside that account's own scope, where the row rules
/// don't apply. It only ever takes away; membership is checked elsewhere.
pub async fn open_to(pool: &sqlx::PgPool, thing: Uuid, viewer: Uuid) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar("SELECT doc_open_to($1, $2)")
        .bind(thing)
        .bind(viewer)
        .fetch_one(crate::db::scoped(pool))
        .await?)
}

/// One writer at a time changes a space's tree, so two moves can't close a
/// loop between them.
pub async fn lock_tree(tx: &mut Transaction<'_, Postgres>, space: Uuid) -> Result<(), AppError> {
    // Pages inside pages are part of this tree, and a page moved to another
    // space leaves it: the page tree's own lock first, so neither changes
    // under a Docs write.
    crate::memory::lock_page_trees(tx, &[space]).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('doc-tree:' || $1::text, 0))")
        .bind(space)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The folders from `folder` up to the top, nearest first. Stops at
/// anything that isn't a folder place, and at the depth no tree reaches.
pub async fn folders_above(
    connection: &mut PgConnection,
    folder: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "WITH RECURSIVE up(id, depth) AS ( \
             SELECT $1::uuid, 1 \
             UNION ALL \
             SELECT place.parent_id, up.depth + 1 \
             FROM up JOIN doc_places place ON place.thing_id = up.id \
             WHERE place.parent_id IS NOT NULL AND up.depth < 64) \
         SELECT id FROM up ORDER BY depth",
    )
    .bind(folder)
    .fetch_all(&mut *connection)
    .await?)
}

/// How many folders deep the deepest folder under `folder` is, counting
/// `folder` itself as one.
pub async fn height(connection: &mut PgConnection, folder: Uuid) -> Result<usize, AppError> {
    let height = sqlx::query_scalar::<_, i32>(
        "WITH RECURSIVE down(id, depth) AS ( \
             SELECT $1::uuid, 1 \
             UNION ALL \
             SELECT place.thing_id, down.depth + 1 \
             FROM down JOIN doc_places place ON place.parent_id = down.id \
             WHERE place.thing_kind = 'folder' AND down.depth < 64) \
         SELECT max(depth) FROM down",
    )
    .bind(folder)
    .fetch_one(&mut *connection)
    .await?;
    Ok(usize::try_from(height).unwrap_or(usize::MAX))
}

/// Puts `thing` under `parent` (a folder, or `None` for the top), keeping
/// whether it is restricted.
pub async fn place(
    tx: &mut Transaction<'_, Postgres>,
    thing: &Found,
    parent: Option<Uuid>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO doc_places (thing_id, thing_kind, workspace_id, space_id, parent_id) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (thing_id) DO UPDATE \
             SET parent_id = EXCLUDED.parent_id, updated_at = now()",
    )
    .bind(thing.id)
    .bind(thing.kind.key())
    .bind(thing.workspace_id)
    .bind(thing.space_id)
    .bind(parent)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Sets whether `thing` is restricted, keeping where it sits.
pub async fn restrict(
    tx: &mut Transaction<'_, Postgres>,
    thing: &Found,
    restricted: bool,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO doc_places (thing_id, thing_kind, workspace_id, space_id, restricted) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (thing_id) DO UPDATE \
             SET restricted = EXCLUDED.restricted, updated_at = now()",
    )
    .bind(thing.id)
    .bind(thing.kind.key())
    .bind(thing.workspace_id)
    .bind(thing.space_id)
    .bind(restricted)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

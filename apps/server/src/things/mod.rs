//! The thing registry (K3 3.1, K3.10). Every task, page, decision, board,
//! sheet, collection, record, conversation and project has one row in
//! `things` with its own id, kept current by a trigger on its kind's table
//! (migration `20261114192000_thing_registry`). Here: the kinds and what each
//! supports, finding a thing by id alone (`locate`), opening things by id
//! (`get`, `get_many`) and the ids a viewer may see (`visible_ids`).
//!
//! Reading what a thing shows (its title, status name, owner, links) is still
//! `modules::things`, per kind, with the viewer's access; this module finds
//! the kind first, so callers no longer need it.

pub mod backfill;
pub mod kind;

use std::collections::{BTreeMap, HashMap};

use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{error::AppError, modules::contract::Thing};

pub use crate::modules::things::Viewer;
pub use kind::ThingKind;

/// Whether a kind has a piece yet, and if not, why.
#[derive(Clone, Copy, Debug)]
pub enum Support<T> {
    Yes(T),
    Not(&'static str),
}

impl<T> Support<T> {
    pub const fn is_yes(&self) -> bool {
        matches!(self, Self::Yes(_))
    }
}

/// One kind of thing: the seam M1 later fills from manifests.
#[derive(Debug)]
pub struct ThingKindSpec {
    pub kind: ThingKind,
    /// The module (manifest id) that makes it.
    pub module: &'static str,
    /// The table its rows live in, and the view the registry copies from.
    pub table: &'static str,
    /// Event types it emits (the catalogue's names).
    pub events: Support<&'static [&'static str]>,
    /// Its `search::SearchKind` name.
    pub search: Support<&'static str>,
    /// Who keeps its versions (K3.19).
    pub history: Support<&'static str>,
    /// H1's exporter id.
    pub export: Support<&'static str>,
    /// May be a link end (K3.12).
    pub links: Support<()>,
}

const NOT_YET_HISTORY: &str = "K3.19 adds history adapters";
const NOT_YET_EXPORT: &str = "H1 declares exporters";
const NOT_YET_LINKS: &str = "K3.12 adds the link store";

macro_rules! spec {
    ($kind:ident, $module:literal, $table:literal, $events:expr, $search:expr) => {
        ThingKindSpec {
            kind: ThingKind::$kind,
            module: $module,
            table: $table,
            events: $events,
            search: $search,
            history: Support::Not(NOT_YET_HISTORY),
            export: Support::Not(NOT_YET_EXPORT),
            links: Support::Not(NOT_YET_LINKS),
        }
    };
}

static REGISTRY: [ThingKindSpec; 11] = [
    spec!(
        PROJECT,
        "projects",
        "projects",
        Support::Yes(&["space.member.changed"]),
        Support::Not("projects are found by name in the project list")
    ),
    spec!(
        CHANNEL,
        "channels",
        "conversations",
        Support::Yes(&["conversation.created", "conversation.updated"]),
        Support::Yes("messages")
    ),
    spec!(
        THREAD,
        "channels",
        "conversations",
        Support::Yes(&["conversation.created", "conversation.updated"]),
        Support::Yes("threads")
    ),
    spec!(
        CONVERSATION,
        "messages",
        "conversations",
        Support::Yes(&["conversation.created", "conversation.updated"]),
        Support::Yes("messages")
    ),
    spec!(
        TASK,
        "tasks",
        "work_items",
        Support::Yes(&["task.created", "task.updated", "task.placement_changed"]),
        Support::Yes("tasks")
    ),
    spec!(
        PAGE,
        "pages",
        "project_pages",
        Support::Yes(&["page.content_saved", "page.discussion_changed"]),
        Support::Yes("pages")
    ),
    spec!(
        DECISION,
        "pages",
        "project_decisions",
        Support::Not("K3.8 adds decision events"),
        Support::Yes("decisions")
    ),
    spec!(
        WHITEBOARD,
        "whiteboards",
        "whiteboards",
        Support::Yes(&["whiteboard.scene_changed"]),
        Support::Yes("whiteboards")
    ),
    spec!(
        SHEET,
        "sheets",
        "sheets",
        Support::Yes(&["sheet.updated"]),
        Support::Yes("sheets")
    ),
    spec!(
        COLLECTION,
        "collections",
        "collections",
        Support::Yes(&["collection.updated"]),
        Support::Not("collections are found by name in their list")
    ),
    spec!(
        RECORD,
        "collections",
        "collection_records",
        Support::Yes(&["record.created", "record.updated"]),
        Support::Yes("records")
    ),
];

/// Every registered kind.
pub fn registry() -> &'static [ThingKindSpec] {
    &REGISTRY
}

/// The registration for `kind`.
pub fn spec(kind: &str) -> Option<&'static ThingKindSpec> {
    REGISTRY.iter().find(|spec| spec.kind == kind)
}

/// Where a thing lives, without access checks: for a caller that then asks
/// the access rule (K2's dispatcher, step 4). Reads only `things`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub id: Uuid,
    pub kind: ThingKind,
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
    /// Whose visibility governs it, when that is another thing.
    pub access_thing_id: Option<Uuid>,
    pub version: i64,
    pub deleted: bool,
}

#[derive(FromRow)]
struct LocatedRow {
    id: Uuid,
    kind: String,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    access_parent_id: Option<Uuid>,
    version: i64,
    deleted: bool,
}

impl From<LocatedRow> for Located {
    fn from(row: LocatedRow) -> Self {
        Self {
            id: row.id,
            kind: ThingKind::new(row.kind),
            workspace_id: row.workspace_id,
            space_id: row.space_id,
            access_thing_id: row.access_parent_id,
            version: row.version,
            deleted: row.deleted,
        }
    }
}

const LOCATE_SQL: &str = r#"
    SELECT id, kind, workspace_id, space_id, access_parent_id, version,
           deleted_at IS NOT NULL AS deleted
    FROM things WHERE id = ANY($1)
"#;

/// The thing `id`, wherever it is; `None` for an unknown id.
pub async fn locate(connection: &mut PgConnection, id: Uuid) -> Result<Option<Located>, AppError> {
    Ok(locate_many(connection, &[id]).await?.pop())
}

/// The things among `ids` that exist, in no particular order.
pub async fn locate_many(
    connection: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<Located>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, LocatedRow>(LOCATE_SQL)
        .bind(ids)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(Located::from)
        .collect())
}

/// One thing by id, if the viewer can see it now. Deleted things aren't.
pub async fn get(
    connection: &mut PgConnection,
    viewer: &Viewer<'_>,
    id: Uuid,
) -> Result<Option<Thing>, AppError> {
    Ok(get_many(connection, viewer, &[id]).await?.pop())
}

/// The things among `ids` the viewer can see, in the order asked.
pub async fn get_many(
    connection: &mut PgConnection,
    viewer: &Viewer<'_>,
    ids: &[Uuid],
) -> Result<Vec<Thing>, AppError> {
    let mut by_kind = BTreeMap::<ThingKind, Vec<Uuid>>::new();
    for located in locate_many(connection, ids).await? {
        if !located.deleted {
            by_kind.entry(located.kind).or_default().push(located.id);
        }
    }
    let mut found = HashMap::new();
    for (kind, ids) in by_kind {
        for thing in
            crate::modules::things::get_many(connection, viewer, kind.as_str(), &ids).await?
        {
            found.insert(thing.id, thing);
        }
    }
    Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
}

/// The ids among `ids` the viewer may see now. Until K1's SQL filter lands
/// this wraps today's per-kind rules (the resolvers in `modules::things`);
/// K1 replaces the body (K1.20). A named stopgap, not a second access rule.
pub async fn visible_ids(
    connection: &mut PgConnection,
    viewer: &Viewer<'_>,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    Ok(get_many(connection, viewer, ids)
        .await?
        .into_iter()
        .map(|thing| thing.id)
        .collect())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The migration seeds `thing_kinds` with the registered kinds; this is
    /// that list, so the two can't drift.
    const SEEDED: &str = include_str!("../../migrations/20261114192000_thing_registry.sql");

    #[test]
    fn every_registered_kind_is_seeded_declared_and_openable() {
        let mut seen = BTreeSet::new();
        for spec in registry() {
            assert!(seen.insert(spec.kind.as_str()), "{} twice", spec.kind);
            assert!(
                SEEDED.contains(&format!("('{}', '{}')", spec.kind, spec.module)),
                "{} isn't seeded in thing_kinds with module {}",
                spec.kind,
                spec.module
            );
            assert!(
                SEEDED.contains(&format!("thing_source_{}", spec.table)),
                "{} has no source view",
                spec.kind
            );
            assert!(
                crate::modules::things::KINDS.contains(&spec.kind.as_str()),
                "{} can't be opened",
                spec.kind
            );
            if let Support::Yes(events) = spec.events {
                for event in events {
                    assert!(
                        crate::events::catalog::get(event).is_some(),
                        "{event} isn't in the catalogue"
                    );
                }
            }
            if let Support::Yes(search) = spec.search {
                assert!(
                    crate::search::SearchKind::parse(search).is_some(),
                    "{search} isn't a search kind"
                );
            }
        }
        for kind in crate::modules::things::KINDS {
            assert!(
                spec(kind).is_some(),
                "{kind} is openable but not registered"
            );
        }
    }
}

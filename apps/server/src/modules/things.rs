//! What exists today, as things (the shared contract's first wiring).
//!
//! Tasks, pages, decisions, whiteboards, threads, channels, conversations
//! and projects are [`Thing`]s here, and the links already stored between
//! them (subtasks, dependencies, related tasks, a task's pages and threads,
//! a whiteboard's links) read as typed [`ThingLink`]s from both sides. This
//! is what "Referenced in" (AWD-58) and the `thing_open` tool draw on; new
//! kinds join by adding a resolver here until modules bring their own.
//!
//! Every read is the viewer's: tasks, pages and decisions only in projects
//! they can open, conversations through `conversation_visible_to`,
//! whiteboards through `whiteboard_visible_to`, and nothing from a module
//! that's off for them.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::{
    ModuleSet,
    contract::{LinkDirection, LinkKind, Thing, ThingLink, ThingOwner},
    manifest,
    state::ProjectModules,
};
use crate::{
    addresses::{Addresses, Linker, Target},
    error::AppError,
};

/// The most links one thing shows.
const MAX_LINKS: usize = 200;

/// Who's asking, and what they can reach.
#[derive(Clone, Copy, Debug)]
pub struct Viewer<'a> {
    pub actor_id: Uuid,
    /// Projects they can open right now.
    pub projects: &'a [Uuid],
    /// Of those, the ones where they (and an app acting for them) may see
    /// tasks (`search::projects_showing_tasks`): a task is a thing only there.
    pub task_projects: &'a [Uuid],
    /// Modules on for them.
    pub modules: ModuleSet,
    /// The modules on for them in each workspace they're in (their workspace
    /// role narrowing it), for things across a workspace, which no project's
    /// modules decide.
    pub workspaces: &'a BTreeMap<Uuid, ModuleSet>,
    /// How the server writes the things' links.
    pub addresses: Addresses,
}

/// The kinds this server can open as things.
pub const KINDS: &[&str] = &[
    "task",
    "page",
    "decision",
    "whiteboard",
    "thread",
    "channel",
    "conversation",
    "project",
    "record",
    "sheet",
    "collection",
];

#[derive(Debug, FromRow)]
struct ThingRow {
    id: Uuid,
    kind: String,
    title: String,
    status: Option<String>,
    owner_id: Option<Uuid>,
    owner_name: Option<String>,
    project_id: Option<Uuid>,
    /// For a thing across a workspace (no project): that workspace.
    organization_id: Option<Uuid>,
    /// For conversations: whether they're a DM or group (in no space).
    direct: bool,
    /// For a thread: the conversation it was started in. For a record: its
    /// collection.
    #[sqlx(default)]
    parent_id: Option<Uuid>,
}

/// A thing's path from its manifest's template: `{id}`, `{projectId}`, and
/// `{parentId}` for one that opens inside another (a record's collection).
fn href(kind: &str, id: Uuid, project_id: Option<Uuid>, parent_id: Option<Uuid>) -> String {
    let template = manifest::thing_decl(kind).map_or("/", |decl| decl.href.as_str());
    template
        .replace("{id}", &id.to_string())
        .replace(
            "{projectId}",
            &project_id.map(|id| id.to_string()).unwrap_or_default(),
        )
        .replace(
            "{parentId}",
            &parent_id.map(|id| id.to_string()).unwrap_or_default(),
        )
}

impl ThingRow {
    /// The thing's path in today's shape.
    fn legacy_href(&self) -> String {
        match self.parent_id {
            // AWD-75: a DM or group's thread opens beside its conversation,
            // as talk::thread_path links it, not under Channels.
            Some(parent) if self.kind == "thread" && self.direct => Target::Direct {
                id: parent,
                thread: Some(self.id),
            }
            .legacy_path(),
            _ => href(&self.kind, self.id, self.project_id, self.parent_id),
        }
    }

    /// The thing's path as this server writes links.
    async fn href(
        &self,
        connection: &mut PgConnection,
        linker: &mut Linker,
    ) -> Result<String, AppError> {
        let legacy = self.legacy_href();
        match (self.project_id, self.organization_id) {
            (Some(space), _) => linker.legacy_in_space(connection, space, &legacy).await,
            (None, Some(workspace)) if self.kind == "whiteboard" => {
                linker
                    .path_in(connection, workspace, &Target::Whiteboard(self.id))
                    .await
            }
            (None, Some(workspace)) => {
                linker
                    .legacy_in_workspace(connection, workspace, &legacy)
                    .await
            }
            _ if linker.addresses().names_workspace() => Ok(crate::addresses::anywhere(&legacy)),
            _ => Ok(legacy),
        }
    }

    fn into_thing(self, href: String) -> Thing {
        let module = manifest::of_thing(&self.kind).map_or_else(String::new, |m| m.id.clone());
        Thing {
            href,
            id: self.id,
            module,
            kind: self.kind,
            title: self.title,
            status: self.status,
            owner: self.owner_id.map(|id| ThingOwner {
                id,
                name: self.owner_name.unwrap_or_else(|| "Someone".to_owned()),
            }),
            project_id: self.project_id,
            space_id: None,
            parent_id: None,
            version: None,
            updated_at: None,
            fields: serde_json::Map::new(),
        }
    }
}

const TASKS_SQL: &str = r#"
    SELECT item.id, 'task' AS kind,
           project.project_key || '-' || item.number::text || ' ' || item.title AS title,
           status.name AS status, item.assignee_actor_id AS owner_id,
           owner.display_name AS owner_name, item.project_id, NULL::uuid AS organization_id, false AS direct
    FROM task_rows item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_memberships owner
      ON owner.project_id = item.project_id AND owner.actor_id = item.assignee_actor_id
    WHERE item.id = ANY($1) AND item.project_id = ANY($2) AND item.archived_at IS NULL
"#;

const PAGES_SQL: &str = r#"
    SELECT page.id, 'page' AS kind, page.title, NULL::text AS status,
           page.updated_by_actor_id AS owner_id, editor.display_name AS owner_name,
           page.project_id, NULL::uuid AS organization_id, false AS direct
    FROM project_pages page
    LEFT JOIN project_memberships editor
      ON editor.project_id = page.project_id AND editor.actor_id = page.updated_by_actor_id
    WHERE page.id = ANY($1) AND page.project_id = ANY($2) AND page.archived_at IS NULL
      AND (page.memory_for_kind IS DISTINCT FROM 'conversation' OR page_visible_to(page.id, $3))
"#;

const DECISIONS_SQL: &str = r#"
    SELECT decision.id, 'decision' AS kind, decision.title, decision.status,
           decision.decided_by_actor_id AS owner_id, decider.display_name AS owner_name,
           decision.project_id, NULL::uuid AS organization_id, false AS direct
    FROM project_decisions decision
    LEFT JOIN project_memberships decider
      ON decider.project_id = decision.project_id
     AND decider.actor_id = decision.decided_by_actor_id
    WHERE decision.id = ANY($1) AND decision.project_id = ANY($2)
      AND (decision.source_conversation_id IS NULL
           OR conversation_visible_to(decision.source_conversation_id, $3))
"#;

const WHITEBOARDS_SQL: &str = r#"
    SELECT board.id, 'whiteboard' AS kind, board.title, NULL::text AS status,
           board.created_by_actor_id AS owner_id, maker.display_name AS owner_name,
           board.project_id, board.organization_id, false AS direct
    FROM whiteboards board
    LEFT JOIN users maker ON maker.actor_id = board.created_by_actor_id
    WHERE board.id = ANY($1) AND board.deleted_at IS NULL
      -- Every board lives in a space (K4.10), the workspace's own included.
      AND board.project_id = ANY($2)
      AND whiteboard_visible_to(board.id, $3)
"#;

const CONVERSATIONS_SQL: &str = r#"
    SELECT conversation.id,
           CASE conversation.kind
             WHEN 'thread' THEN 'thread'
             WHEN 'channel' THEN 'channel'
             ELSE 'conversation'
           END AS kind,
           CASE WHEN conversation.kind = 'channel' THEN '#' || conversation.name
                ELSE conversation.name END AS title,
           CASE WHEN conversation.kind = 'thread' THEN conversation.thread_status END AS status,
           NULL::uuid AS owner_id, NULL::text AS owner_name,
           conversation.project_id, NULL::uuid AS organization_id,
           conversation.project_id IS NULL AS direct, conversation.parent_channel_id AS parent_id
    FROM conversations conversation
    WHERE conversation.id = ANY($1)
      AND (conversation.project_id IS NULL OR conversation.project_id = ANY($2))
      AND conversation.archived_at IS NULL
      AND conversation_visible_to(conversation.id, $3)
"#;

const PROJECTS_SQL: &str = r#"
    SELECT project.id, 'project' AS kind, project.name AS title, NULL::text AS status,
           NULL::uuid AS owner_id, NULL::text AS owner_name, project.id AS project_id,
           NULL::uuid AS organization_id, false AS direct
    FROM projects project
    WHERE project.id = ANY($1) AND project.id = ANY($2) AND project.kind = 'standard'
"#;

/// A collection's records (AWD-110), in collections that aren't archived.
const RECORDS_SQL: &str = r#"
    SELECT record.id, 'record' AS kind,
           collection.name || ' › ' ||
               CASE WHEN record.title = '' THEN 'Untitled #' || record.number::text
                    ELSE record.title END AS title,
           NULL::text AS status, record.updated_by_actor_id AS owner_id,
           editor.display_name AS owner_name, record.project_id,
           NULL::uuid AS organization_id, false AS direct, record.collection_id AS parent_id
    FROM collection_records record
    JOIN collections collection
      ON collection.project_id = record.project_id AND collection.id = record.collection_id
    LEFT JOIN project_memberships editor
      ON editor.project_id = record.project_id AND editor.actor_id = record.updated_by_actor_id
    WHERE record.id = ANY($1) AND record.project_id = ANY($2)
      AND record.archived_at IS NULL AND collection.archived_at IS NULL
      -- M1.6: a built module switched off hides its collections' records.
      AND module_part_on('collection', collection.id)
      -- A task's record (the built-in Tasks collection) is the task.
      AND collection.builtin IS NULL
"#;

/// A project's sheets (AWD-54) that aren't archived.
const SHEETS_SQL: &str = r#"
    SELECT sheet.id, 'sheet' AS kind, sheet.name AS title, NULL::text AS status,
           sheet.updated_by_actor_id AS owner_id, editor.display_name AS owner_name,
           sheet.project_id, NULL::uuid AS organization_id, false AS direct
    FROM sheets sheet
    LEFT JOIN project_memberships editor
      ON editor.project_id = sheet.project_id AND editor.actor_id = sheet.updated_by_actor_id
    WHERE sheet.id = ANY($1) AND sheet.project_id = ANY($2) AND sheet.archived_at IS NULL
"#;

/// A project's collections that aren't archived. The built-in Tasks
/// collection is the task list, so it opens like any other.
const COLLECTIONS_SQL: &str = r#"
    SELECT collection.id, 'collection' AS kind, collection.name AS title, NULL::text AS status,
           collection.created_by_actor_id AS owner_id, maker.display_name AS owner_name,
           collection.project_id, NULL::uuid AS organization_id, false AS direct
    FROM collections collection
    LEFT JOIN project_memberships maker
      ON maker.project_id = collection.project_id AND maker.actor_id = collection.created_by_actor_id
    WHERE collection.id = ANY($1) AND collection.project_id = ANY($2)
      AND collection.archived_at IS NULL
"#;

fn resolver(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "task" => TASKS_SQL,
        "page" => PAGES_SQL,
        "decision" => DECISIONS_SQL,
        "whiteboard" => WHITEBOARDS_SQL,
        "thread" | "channel" | "conversation" => CONVERSATIONS_SQL,
        "project" => PROJECTS_SQL,
        "record" => RECORDS_SQL,
        "sheet" => SHEETS_SQL,
        "collection" => COLLECTIONS_SQL,
        _ => return None,
    })
}

/// Whether the viewer's modules show this thing.
fn shown(viewer: &Viewer<'_>, row: &ThingRow) -> bool {
    // A thing across a workspace follows that workspace's modules for them.
    let modules = match (row.project_id, row.organization_id) {
        (None, Some(workspace)) => viewer
            .workspaces
            .get(&workspace)
            .copied()
            .unwrap_or_default(),
        _ => viewer.modules,
    };
    shown_in(modules, &row.kind, row.direct)
}

/// Whether a thing of `kind` shows with `modules` on: a conversation by
/// where it lives (Messages in the direct space, Channels in a project),
/// anything else by the module that makes it.
fn shown_in(modules: ModuleSet, kind: &str, direct: bool) -> bool {
    match kind {
        "conversation" | "channel" | "thread" => {
            modules.allows_conversations_in(if direct { "direct" } else { "standard" })
        }
        kind => manifest::of_thing(kind).is_none_or(|module| modules.contains(&module.id)),
    }
}

/// The highest `$n` placeholder a resolver query declares, ignoring
/// placeholders mentioned in `--` comments and string literals. Every
/// resolver query declares a prefix of the shared binds (see `resolve`),
/// so this is also the number of values `resolve` must bind: Postgres
/// rejects both a missing bind and an extra one.
fn declared_params(sql: &str) -> usize {
    let bytes = sql.as_bytes();
    let mut highest = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // A line comment runs to the newline.
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // A string literal runs to its closing quote ('' escapes one).
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'$' => {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    end += 1;
                }
                if end > start {
                    let n: usize = sql[start..end].parse().unwrap_or(0);
                    highest = highest.max(n);
                    i = end;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    highest
}

/// Things of one kind, by id, that the viewer can see. Unknown kinds and
/// hidden things are left out.
async fn resolve(
    connection: &mut PgConnection,
    viewer: &Viewer<'_>,
    kind: &str,
    ids: &[Uuid],
) -> Result<Vec<Thing>, AppError> {
    let Some(sql) = resolver(kind) else {
        return Ok(Vec::new());
    };
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // The binds are positional and shared by every resolver: $1 the ids,
    // $2 the viewer's projects, $3 the viewer. Each query declares the
    // prefix it uses — tasks, pages and projects stop at $2 (their
    // visibility is the project filter itself), decisions, conversations
    // and whiteboards also consult the viewer at $3 — and Postgres rejects
    // a bind the query doesn't declare, so bind exactly the prefix the
    // query asks for.
    let declared = declared_params(sql);
    let mut query = sqlx::query_as::<_, ThingRow>(sql)
        .bind(ids)
        .bind(if kind == "task" {
            viewer.task_projects
        } else {
            viewer.projects
        });
    if declared >= 3 {
        query = query.bind(viewer.actor_id);
    }
    let rows = query.fetch_all(&mut *connection).await?;
    let mut linker = Linker::new(viewer.addresses);
    let mut things = Vec::with_capacity(rows.len());
    for row in rows
        .into_iter()
        // A conversation id can come back as any conversation kind; keep
        // the kind asked for, except that "conversation" asks for any.
        .filter(|row| kind == "conversation" || row.kind == kind)
        .filter(|row| shown(viewer, row))
    {
        let href = row.href(connection, &mut linker).await?;
        things.push(row.into_thing(href));
    }
    with_envelope(connection, &mut things).await?;
    Ok(things)
}

#[derive(FromRow)]
struct Envelope {
    id: Uuid,
    space_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    version: i64,
    updated_at: String,
}

/// The registry's envelope (K3.10) on things already read with the
/// viewer's access: where each lives, what holds it, its version.
async fn with_envelope(
    connection: &mut PgConnection,
    things: &mut [Thing],
) -> Result<(), AppError> {
    if things.is_empty() {
        return Ok(());
    }
    let ids = things.iter().map(|thing| thing.id).collect::<Vec<_>>();
    let envelopes = sqlx::query_as::<_, Envelope>(
        "SELECT id, space_id, parent_id, version, updated_at::text AS updated_at \
         FROM things WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|envelope| (envelope.id, envelope))
    .collect::<BTreeMap<_, _>>();
    for thing in things {
        if let Some(envelope) = envelopes.get(&thing.id) {
            thing.space_id = envelope.space_id;
            thing.parent_id = envelope.parent_id;
            thing.version = Some(envelope.version);
            thing.updated_at = Some(envelope.updated_at.clone());
        }
    }
    Ok(())
}

/// Things of one kind, by id, that the viewer can see (for
/// `things::get_many`, which finds the kinds).
pub async fn get_many(
    connection: &mut PgConnection,
    viewer: &Viewer<'_>,
    kind: &str,
    ids: &[Uuid],
) -> Result<Vec<Thing>, AppError> {
    resolve(connection, viewer, kind, ids).await
}

/// One thing, if the viewer can see it.
pub async fn get(
    connection: &mut PgConnection,
    viewer: &Viewer<'_>,
    kind: &str,
    id: Uuid,
) -> Result<Option<Thing>, AppError> {
    Ok(resolve(connection, viewer, kind, &[id]).await?.pop())
}

fn link_kind(key: &str) -> Option<LinkKind> {
    LinkKind::ALL.into_iter().find(|kind| kind.key() == key)
}

/// A thing's links, both ways, to things the viewer can see. The caller has
/// checked that the viewer can see the thing itself.
pub async fn links(
    connection: &mut PgConnection,
    viewer: &Viewer<'_>,
    thing: &Thing,
) -> Result<Vec<ThingLink>, AppError> {
    if !KINDS.contains(&thing.kind.as_str()) {
        return Ok(Vec::new());
    }
    let rows = crate::links::for_thing(connection, thing.id, MAX_LINKS).await?;
    // Each linked thing follows the modules of its own project (the viewer's
    // role there narrowing them), not the thing it's linked from: resolve
    // them all, then keep those whose module is on where they are. One across
    // a workspace was already held to that workspace's modules.
    let unfiltered = Viewer {
        actor_id: viewer.actor_id,
        projects: viewer.projects,
        task_projects: viewer.task_projects,
        modules: ModuleSet::all(),
        workspaces: viewer.workspaces,
        addresses: viewer.addresses,
    };
    let mut others = Vec::new();
    for kind in KINDS {
        let ids = rows
            .iter()
            .filter(|row| row.other_kind == *kind)
            .map(|row| row.other_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        others.extend(resolve(connection, &unfiltered, kind, &ids).await?);
    }
    let places = others
        .iter()
        .filter_map(|other| other.project_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let in_places = ProjectModules::load(&mut *connection, viewer.actor_id, &places).await?;
    others.retain(|other| match other.project_id {
        Some(project) => !in_places
            .keep(&[project], |set, direct| shown_in(set, &other.kind, direct))
            .is_empty(),
        None => true,
    });
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let kind = link_kind(&row.link)?;
            let direction = if row.direction == "incoming" {
                LinkDirection::Incoming
            } else {
                LinkDirection::Outgoing
            };
            let other = others
                .iter()
                .find(|other| other.id == row.other_id)?
                .clone();
            let (outgoing, incoming) = kind.words();
            Some(ThingLink {
                kind,
                direction,
                label: match direction {
                    LinkDirection::Outgoing => outgoing,
                    LinkDirection::Incoming => incoming,
                }
                .to_owned(),
                other,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_kind_this_server_opens_is_declared_by_a_module() {
        for kind in super::KINDS {
            assert!(super::manifest::thing_decl(kind).is_some(), "{kind}");
            assert!(super::resolver(kind).is_some(), "{kind}");
        }
    }

    #[test]
    fn resolver_queries_declare_a_prefix_of_the_shared_binds() {
        // resolve() binds, in order: the ids, the viewer's projects, the
        // viewer, and whether workspace boards show. A query must declare
        // exactly the prefix it uses: Postgres rejects a bind past the
        // highest placeholder, and a gap would shift the later values
        // onto the wrong placeholders.
        let expected = [
            ("task", 2),
            ("page", 3),
            ("decision", 3),
            ("whiteboard", 3),
            ("thread", 3),
            ("channel", 3),
            ("conversation", 3),
            ("project", 2),
            ("record", 2),
        ];
        for (kind, count) in expected {
            let sql = super::resolver(kind).expect(kind);
            assert_eq!(super::declared_params(sql), count, "{kind}");
            for n in 1..=count {
                assert!(sql.contains(&format!("${n}")), "{kind} skips ${n}");
            }
        }
    }

    #[test]
    fn declared_params_ignores_comments_and_literals() {
        assert_eq!(super::declared_params("SELECT $1, $2 -- not $9\n"), 2);
        assert_eq!(super::declared_params("SELECT '$7', $1"), 1);
        assert_eq!(super::declared_params("SELECT $1, $10"), 10);
    }

    #[test]
    fn hrefs_fill_in_their_ids() {
        let id = uuid::Uuid::nil();
        let project = uuid::Uuid::max();
        assert_eq!(
            super::href("task", id, Some(project), None),
            format!("/projects/{project}/work-items/{id}")
        );
        assert_eq!(
            super::href("whiteboard", id, None, None),
            format!("/whiteboards/{id}")
        );
        let collection = uuid::Uuid::from_u128(7);
        assert_eq!(
            super::href("record", id, Some(project), Some(collection)),
            format!("/projects/{project}/collections/{collection}?record={id}")
        );
    }
}

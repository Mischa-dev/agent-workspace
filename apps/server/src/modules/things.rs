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
use crate::error::AppError;

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
    /// Whether things across a workspace (boards with no project) can show:
    /// not for a credential confined to one project.
    pub workspace_boards: bool,
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
    /// For conversations: whether they're in the hidden direct project.
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
    fn into_thing(self) -> Thing {
        let module = manifest::of_thing(&self.kind).map_or_else(String::new, |m| m.id.clone());
        Thing {
            // AWD-75: a DM or group's thread opens beside its conversation,
            // as talk::thread_path links it, not under Channels.
            href: match self.parent_id {
                Some(parent) if self.kind == "thread" && self.direct => {
                    format!("/messages/{parent}?thread={}", self.id)
                }
                _ => href(&self.kind, self.id, self.project_id, self.parent_id),
            },
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
        }
    }
}

const TASKS_SQL: &str = r#"
    SELECT item.id, 'task' AS kind,
           project.project_key || '-' || item.number::text || ' ' || item.title AS title,
           status.name AS status, item.assignee_actor_id AS owner_id,
           owner.display_name AS owner_name, item.project_id, NULL::uuid AS organization_id, false AS direct
    FROM work_items item
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
      -- A board across a workspace only for a viewer not confined to one
      -- project ($4), as whiteboards::access has it.
      AND ((board.project_id IS NULL AND $4) OR board.project_id = ANY($2))
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
           project.kind = 'direct' AS direct, conversation.parent_channel_id AS parent_id
    FROM conversations conversation
    JOIN projects project ON project.id = conversation.project_id
    WHERE conversation.id = ANY($1) AND conversation.project_id = ANY($2)
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
    // $2 the viewer's projects, $3 the viewer, $4 whether things across a
    // workspace show. Each query declares the prefix it uses — tasks,
    // pages and projects stop at $2 (their visibility is the project
    // filter itself), decisions and conversations also consult the
    // viewer at $3, and whiteboards take $4 as well — and Postgres
    // rejects a bind the query doesn't declare, so bind exactly the
    // prefix the query asks for.
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
    if declared >= 4 {
        query = query.bind(viewer.workspace_boards);
    }
    let rows = query.fetch_all(&mut *connection).await?;
    Ok(rows
        .into_iter()
        // A conversation id can come back as any conversation kind; keep
        // the kind asked for, except that "conversation" asks for any.
        .filter(|row| kind == "conversation" || row.kind == kind)
        .filter(|row| shown(viewer, row))
        .map(ThingRow::into_thing)
        .collect())
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

#[derive(Debug, FromRow)]
struct LinkRow {
    link: String,
    direction: String,
    other_kind: String,
    other_id: Uuid,
}

/// The links stored for each kind, as (link kind, direction, other kind,
/// other id). `$1` is the thing.
fn links_sql(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "task" => {
            r#"
            SELECT 'part_of' AS link, 'outgoing' AS direction, 'task' AS other_kind,
                   parent_work_item_id AS other_id
            FROM work_items WHERE id = $1 AND parent_work_item_id IS NOT NULL
            UNION ALL
            SELECT 'part_of', 'incoming', 'task', id
            FROM work_items WHERE parent_work_item_id = $1 AND archived_at IS NULL
            UNION ALL
            SELECT 'blocks', 'outgoing', 'task', blocked_work_item_id
            FROM work_item_dependencies WHERE blocking_work_item_id = $1
            UNION ALL
            SELECT 'blocks', 'incoming', 'task', blocking_work_item_id
            FROM work_item_dependencies WHERE blocked_work_item_id = $1
            UNION ALL
            SELECT kind, 'outgoing', 'task', other_work_item_id
            FROM work_item_relations WHERE work_item_id = $1
            UNION ALL
            SELECT kind, 'incoming', 'task', work_item_id
            FROM work_item_relations WHERE other_work_item_id = $1
            UNION ALL
            SELECT 'about', 'outgoing',
                   CASE target_kind WHEN 'conversation' THEN 'conversation' ELSE target_kind END,
                   target_id
            FROM work_item_links
            WHERE work_item_id = $1 AND target_kind IN ('page', 'decision', 'conversation')
            UNION ALL
            SELECT 'about', 'outgoing', 'conversation', conversation_id
            FROM work_item_conversation_links WHERE work_item_id = $1
            UNION ALL
            SELECT 'about', 'incoming', 'whiteboard', whiteboard_id
            FROM whiteboard_links WHERE target_kind = 'work_item' AND target_id = $1
            UNION ALL
            SELECT 'about', 'incoming', 'task', comment.work_item_id
            FROM work_item_comment_references reference
            JOIN work_item_comments comment ON comment.id = reference.comment_id
            WHERE reference.referenced_work_item_id = $1 AND comment.removed_at IS NULL
              AND comment.work_item_id <> $1
            UNION ALL
            SELECT 'about', 'incoming', 'conversation', message.conversation_id
            FROM message_work_item_references reference
            JOIN messages message ON message.id = reference.message_id
            WHERE reference.work_item_id = $1 AND message.removed_at IS NULL
            UNION ALL
            SELECT 'about', 'incoming', thing_kind, thing_id
            FROM thing_references WHERE target_id = $1 AND target_kind = 'task'
            UNION ALL
            SELECT 'about', 'outgoing', target_kind, target_id
            FROM thing_references WHERE thing_id = $1 AND thing_kind = 'task'
            UNION ALL
            SELECT 'about', 'outgoing', 'task', reference.referenced_work_item_id
            FROM work_item_comment_references reference
            JOIN work_item_comments comment ON comment.id = reference.comment_id
            WHERE reference.source_work_item_id = $1 AND comment.removed_at IS NULL
              AND reference.referenced_work_item_id <> $1
            "#
        }
        "page" | "decision" => {
            r#"
            SELECT 'about' AS link, 'incoming' AS direction, 'task' AS other_kind,
                   work_item_id AS other_id
            FROM work_item_links WHERE target_kind IN ('page', 'decision') AND target_id = $1
            UNION ALL
            SELECT 'about', 'incoming', 'whiteboard', whiteboard_id
            FROM whiteboard_links WHERE target_kind = 'page' AND target_id = $1
            UNION ALL
            SELECT 'about', 'incoming', thing_kind, thing_id
            FROM thing_references WHERE target_id = $1 AND target_kind = 'page'
            UNION ALL
            SELECT 'about', 'outgoing', target_kind, target_id
            FROM thing_references WHERE thing_id = $1 AND thing_kind = 'page'
            "#
        }
        "whiteboard" => {
            r#"
            SELECT 'about' AS link, 'outgoing' AS direction,
                   CASE target_kind WHEN 'work_item' THEN 'task' ELSE target_kind END AS other_kind,
                   target_id AS other_id
            FROM whiteboard_links WHERE whiteboard_id = $1
            UNION ALL
            SELECT 'about', 'incoming', thing_kind, thing_id
            FROM thing_references WHERE target_id = $1 AND target_kind = 'whiteboard'
            "#
        }
        "thread" | "channel" | "conversation" => {
            r#"
            SELECT 'about' AS link, 'incoming' AS direction, 'task' AS other_kind,
                   work_item_id AS other_id
            FROM work_item_conversation_links WHERE conversation_id = $1
            UNION ALL
            SELECT 'about', 'incoming', 'task', work_item_id
            FROM work_item_links WHERE target_kind = 'conversation' AND target_id = $1
            UNION ALL
            SELECT 'about', 'incoming', thing_kind, thing_id
            FROM thing_references
            WHERE target_id = $1 AND target_kind IN ('thread', 'channel', 'conversation')
            "#
        }
        "project" => {
            r#"
            SELECT 'about' AS link, 'incoming' AS direction, 'whiteboard' AS other_kind,
                   whiteboard_id AS other_id
            FROM whiteboard_links WHERE target_kind = 'project' AND target_id = $1
            "#
        }
        // What a record's link fields point at (AWD-110).
        "record" => {
            r#"
            SELECT 'about' AS link, 'outgoing' AS direction,
                   definition.config->>'target' AS other_kind,
                   (record.field_values->>definition.id::text)::uuid AS other_id
            FROM collection_records record
            JOIN field_definitions definition
              ON definition.project_id = record.project_id
             AND definition.collection_id = record.collection_id
             AND definition.field_type = 'link' AND definition.archived_at IS NULL
            WHERE record.id = $1
              AND definition.config->>'target' IN ('task', 'page')
              AND record.field_values->>definition.id::text ~ '^[0-9a-fA-F-]{36}$'
            "#
        }
        _ => return None,
    })
}

/// Records whose link fields point at a task or page (AWD-110): read apart
/// from `links_sql`, so a thing's stored links and these change on their own.
/// `$1` is the thing, `$2` its kind.
const RECORDS_LINKING_SQL: &str = r#"
    SELECT 'about' AS link, 'incoming' AS direction, 'record' AS other_kind, record.id AS other_id
    FROM field_definitions definition
    JOIN collection_records record
      ON record.project_id = definition.project_id
     AND record.collection_id = definition.collection_id
    WHERE definition.field_type = 'link' AND definition.archived_at IS NULL
      AND definition.collection_id IS NOT NULL
      AND definition.config->>'target' = $2
      AND record.field_values->>definition.id::text = $1::text
      AND record.archived_at IS NULL
    LIMIT 200
"#;

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
    let Some(sql) = links_sql(&thing.kind) else {
        return Ok(Vec::new());
    };
    let mut rows = sqlx::query_as::<_, LinkRow>(sql)
        .bind(thing.id)
        .fetch_all(&mut *connection)
        .await?;
    if matches!(thing.kind.as_str(), "task" | "page") {
        rows.extend(
            sqlx::query_as::<_, LinkRow>(RECORDS_LINKING_SQL)
                .bind(thing.id)
                .bind(&thing.kind)
                .fetch_all(&mut *connection)
                .await?,
        );
    }
    let mut seen = BTreeSet::new();
    let rows = rows
        .into_iter()
        .filter(|row| seen.insert((row.link.clone(), row.direction.clone(), row.other_id)))
        .take(MAX_LINKS)
        .collect::<Vec<_>>();
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
        workspace_boards: viewer.workspace_boards,
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
            assert!(super::links_sql(kind).is_some(), "{kind}");
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
            ("page", 2),
            ("decision", 3),
            ("whiteboard", 4),
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

//! How tasks relate to each other and to milestones, for the web app and MCP
//! alike (docs/plans/roadmap.md, T1 to T3).
//!
//! - Dependencies: one task blocks another. The blocker may be in another
//!   project of the same workspace. A change writes an activity entry on both
//!   tasks, bumps both versions and sends a live update for each, so every
//!   open screen shows it. Cycles are found with a recursive query over the
//!   edges reachable from the new edge, never by loading a whole project.
//! - Relations: relates-to (either way round) and duplicates, with closing a
//!   duplicate in the workflow's canceled status.
//! - Watchers: who hears about a task's changes.
//! - What a reader sees of all this: other tasks appear only when the reader
//!   can reach their project (`actor_can_access_project`, migration 0042).

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::NaiveDate;
use serde::Serialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    conversations::ActorView,
    error::AppError,
    work_item_commands::{
        ActivityRecord, AuditAttribution, CommandActor, TaskFieldChanges, UpdateWorkItemCommand,
        UpdateWorkItemOutcome, finalize_changes, lock_work_item, update_locked_work_item,
    },
};

/// Most dependencies or relations one call changes.
pub const MAX_LINKS_PER_CALL: usize = 50;
/// The most blockers, blocked tasks or relations one task has; task details
/// show them all.
pub const MAX_LINKS_PER_TASK: i64 = 200;

/// Who is reading: a person (no connection) or an agent through its
/// connection, as `actor_can_access_project` takes them.
#[derive(Clone, Copy, Debug)]
pub struct Reader {
    pub actor_id: Uuid,
    pub connection_id: Option<Uuid>,
}

/// The agent connection an actor writes through, as `actor_can_access_project` takes it.
fn connection_of(actor: &CommandActor) -> Option<Uuid> {
    match &actor.audit {
        AuditAttribution::Mcp {
            external_agent_connection_id,
            ..
        } => *external_agent_connection_id,
        AuditAttribution::Browser { .. } => None,
    }
}

/// Another task, as a reader who may see it sees it.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRef {
    pub id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    /// Completed or canceled.
    pub finished: bool,
    pub archived: bool,
    #[sqlx(skip)]
    pub href: String,
}

impl TaskRef {
    fn with_href(mut self) -> Self {
        self.href = format!("/projects/{}/work-items/{}", self.project_id, self.id);
        self
    }
}

const TASK_REF_COLUMNS: &str = r#"
    item.id, item.project_id, project.project_key || '-' || item.number AS key,
    item.title, status.name AS status_name, status.semantic_category AS status_category,
    status.semantic_category IN ('completed', 'canceled') AS finished,
    item.archived_at IS NOT NULL AS archived
"#;

const TASK_REF_JOINS: &str = r#"
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
"#;

/// A task's links, as one reader sees them.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLinks {
    pub blocked_by: Vec<TaskRef>,
    pub blocks: Vec<TaskRef>,
    pub relations: Vec<RelationView>,
    /// Not finished, not marked blocked, and every blocker finished.
    pub ready: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationView {
    /// relates_to, duplicates (this task duplicates the other) or
    /// duplicated_by.
    pub kind: String,
    pub task: TaskRef,
}

/// The blockers, blocked tasks and relations of a task that `reader` can
/// see. Archived tasks are left out of the dependency lists.
pub async fn task_links(
    connection: &mut PgConnection,
    reader: Reader,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<TaskLinks, AppError> {
    let visible = "(item.project_id = $3 OR actor_can_access_project($4, $5, item.project_id))";
    let blocked_by = sqlx::query_as::<_, TaskRef>(&format!(
        r#"SELECT {TASK_REF_COLUMNS}
        FROM work_item_dependencies dependency
        JOIN work_items item
          ON item.project_id = dependency.blocking_project_id
         AND item.id = dependency.blocking_work_item_id
        {TASK_REF_JOINS}
        WHERE dependency.project_id = $1 AND dependency.blocked_work_item_id = $2
          AND item.archived_at IS NULL AND {visible}
        ORDER BY finished, project.project_key, item.number
        LIMIT {MAX_LINKS_PER_TASK}"#
    ))
    .bind(project_id)
    .bind(task_id)
    .bind(project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_all(&mut *connection)
    .await?;
    let blocks = sqlx::query_as::<_, TaskRef>(&format!(
        r#"SELECT {TASK_REF_COLUMNS}
        FROM work_item_dependencies dependency
        JOIN work_items item
          ON item.project_id = dependency.project_id
         AND item.id = dependency.blocked_work_item_id
        {TASK_REF_JOINS}
        WHERE dependency.blocking_project_id = $1 AND dependency.blocking_work_item_id = $2
          AND item.archived_at IS NULL AND {visible}
        ORDER BY finished, project.project_key, item.number
        LIMIT {MAX_LINKS_PER_TASK}"#
    ))
    .bind(project_id)
    .bind(task_id)
    .bind(project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_all(&mut *connection)
    .await?;
    let relations = sqlx::query_as::<_, RelationRow>(&format!(
        r#"SELECT CASE
                  WHEN relation.kind = 'duplicates' AND relation.work_item_id = $2 THEN 'duplicates'
                  WHEN relation.kind = 'duplicates' THEN 'duplicated_by'
                  ELSE 'relates_to' END AS relation_kind,
               {TASK_REF_COLUMNS}
        FROM work_item_relations relation
        JOIN work_items item
          ON item.id = CASE WHEN relation.work_item_id = $2
                            THEN relation.other_work_item_id ELSE relation.work_item_id END
        {TASK_REF_JOINS}
        WHERE (relation.work_item_id = $2 OR relation.other_work_item_id = $2)
          AND {visible}
          AND $1::uuid IS NOT NULL
        ORDER BY relation.created_at, item.id
        LIMIT {MAX_LINKS_PER_TASK}"#
    ))
    .bind(project_id)
    .bind(task_id)
    .bind(project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_all(&mut *connection)
    .await?;
    // Ready reads every blocker, including those the reader can't see.
    let (finished, marked_blocked, open_blockers) = sqlx::query_as::<_, (bool, bool, i64)>(
        r#"
        SELECT status.semantic_category IN ('completed', 'canceled'),
               item.blocked_reason IS NOT NULL,
               (SELECT count(*)
                  FROM work_item_dependencies dependency
                  JOIN work_items blocker
                    ON blocker.project_id = dependency.blocking_project_id
                   AND blocker.id = dependency.blocking_work_item_id
                  JOIN work_item_statuses blocker_status
                    ON blocker_status.project_id = blocker.project_id
                   AND blocker_status.id = blocker.status_id
                 WHERE dependency.project_id = item.project_id
                   AND dependency.blocked_work_item_id = item.id
                   AND blocker.archived_at IS NULL
                   AND blocker_status.semantic_category NOT IN ('completed', 'canceled'))
        FROM work_items item
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or((false, false, 0));
    Ok(TaskLinks {
        blocked_by: blocked_by.into_iter().map(TaskRef::with_href).collect(),
        blocks: blocks.into_iter().map(TaskRef::with_href).collect(),
        relations: relations
            .into_iter()
            .map(|row| RelationView {
                kind: row.relation_kind,
                task: row.task.with_href(),
            })
            .collect(),
        ready: !finished && !marked_blocked && open_blockers == 0,
    })
}

#[derive(Debug, FromRow)]
struct RelationRow {
    relation_kind: String,
    #[sqlx(flatten)]
    task: TaskRef,
}

/// SQL for a task row aliased `item`: how many unfinished tasks block it.
pub const OPEN_BLOCKER_COUNT: &str = r#"
    (SELECT count(*)
       FROM work_item_dependencies dependency
       JOIN work_items blocker
         ON blocker.project_id = dependency.blocking_project_id
        AND blocker.id = dependency.blocking_work_item_id
       JOIN work_item_statuses blocker_status
         ON blocker_status.project_id = blocker.project_id
        AND blocker_status.id = blocker.status_id
      WHERE dependency.project_id = item.project_id
        AND dependency.blocked_work_item_id = item.id
        AND blocker.archived_at IS NULL
        AND blocker_status.semantic_category NOT IN ('completed', 'canceled'))
"#;

/// SQL condition for a task row aliased `item`: some unfinished task blocks it.
pub const HAS_OPEN_BLOCKER: &str = r#"
    EXISTS (SELECT 1
       FROM work_item_dependencies dependency
       JOIN work_items blocker
         ON blocker.project_id = dependency.blocking_project_id
        AND blocker.id = dependency.blocking_work_item_id
       JOIN work_item_statuses blocker_status
         ON blocker_status.project_id = blocker.project_id
        AND blocker_status.id = blocker.status_id
      WHERE dependency.project_id = item.project_id
        AND dependency.blocked_work_item_id = item.id
        AND blocker.archived_at IS NULL
        AND blocker_status.semantic_category NOT IN ('completed', 'canceled'))
"#;

/// Finds a task by ID or key (such as AW-12) in the reader's workspace,
/// among projects the reader can reach. Returns (task, project).
pub async fn resolve_reference(
    connection: &mut PgConnection,
    reader: Reader,
    organization_project_id: Uuid,
    reference: &str,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    let reference = reference.trim();
    let reference = reference
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(reference);
    let (id, key) = match reference.parse::<Uuid>() {
        Ok(id) => (Some(id), None),
        Err(_) => {
            let Some((project_key, number)) = reference.rsplit_once('-') else {
                return Ok(None);
            };
            let Ok(number) = number.parse::<i64>() else {
                return Ok(None);
            };
            if project_key.is_empty() || project_key.len() > 32 {
                return Ok(None);
            }
            (None, Some((project_key.to_owned(), number)))
        }
    };
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT item.id, item.project_id
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN projects home ON home.id = $1
        WHERE project.organization_id = home.organization_id
          AND (($2::uuid IS NOT NULL AND item.id = $2)
               OR ($3::text IS NOT NULL AND upper(project.project_key) = upper($3)
                   AND item.number = $4))
          AND (item.project_id = $1 OR actor_can_access_project($5, $6, item.project_id))
        LIMIT 1
        "#,
    )
    .bind(organization_project_id)
    .bind(id)
    .bind(key.as_ref().map(|(project_key, _)| project_key.clone()))
    .bind(key.as_ref().map(|(_, number)| *number))
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .fetch_optional(&mut *connection)
    .await?)
}

// ---------------------------------------------------------------------------
// Dependencies

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// The other task blocks this one.
    BlockedBy,
    /// This task blocks the other.
    Blocks,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkRefusal {
    NotFound,
    SelfLink,
    /// The other task is in another workspace.
    OtherWorkspace,
    /// Adding it would close a cycle; the keys along it.
    Cycle {
        path: Vec<String>,
    },
    /// Only one task can be the original of a duplicate.
    AlreadyDuplicate {
        original: String,
    },
    /// Closing a duplicate isn't allowed from its status.
    CantClose {
        status: String,
    },
    /// One of the two tasks already has as many links of this kind as a
    /// task can have.
    TooMany {
        key: String,
    },
}

impl LinkRefusal {
    pub fn message(&self, reference: &str) -> String {
        match self {
            Self::NotFound => format!("Task {reference} wasn't found, or you can't see it."),
            Self::SelfLink => "A task can't be linked to itself.".to_owned(),
            Self::OtherWorkspace => format!(
                "{reference} is in another workspace; tasks link only within one workspace."
            ),
            Self::Cycle { path } => format!(
                "That would make a cycle: {} already leads back. Remove one of those dependencies first.",
                path.join(" blocks ")
            ),
            Self::AlreadyDuplicate { original } => {
                format!("This task is already marked a duplicate of {original}. Remove that first.")
            }
            Self::CantClose { status } => format!(
                "It can't be closed from {status} in this project's workflow. Move it first, or link it without closing."
            ),
            Self::TooMany { key } => format!(
                "{key} already has {MAX_LINKS_PER_TASK} links of this kind, the most a task can have. Remove some first."
            ),
        }
    }
}

#[derive(Debug, FromRow)]
struct LockedEnd {
    id: Uuid,
    project_id: Uuid,
    key: String,
    version: i64,
}

/// Locks tasks across projects in ID order.
/// How `task`'s history names `other`: by key within one project. Across
/// projects the entry is read by members of `task`'s project, who may not
/// see the other one, so it isn't named; readers who can see it get the
/// task itself from the payload and `task_links`.
fn named(other: &LockedEnd, task: &LockedEnd) -> String {
    if other.project_id == task.project_id {
        other.key.clone()
    } else {
        "a task in another project".to_owned()
    }
}

async fn lock_ends(
    transaction: &mut Transaction<'_, Postgres>,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, LockedEnd>, AppError> {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    Ok(sqlx::query_as::<_, LockedEnd>(
        r#"
        SELECT item.id, item.project_id, project.project_key || '-' || item.number AS key,
               item.version
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        WHERE item.id = ANY($1)
        ORDER BY item.id
        FOR UPDATE OF item
        "#,
    )
    .bind(&sorted)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|end| (end.id, end))
    .collect())
}

/// Serializes link writes in one workspace, so two concurrent writers can't
/// each add half of a cycle.
async fn lock_workspace_links(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('work_item_links:' || organization_id::text, 0)) FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The path of tasks from `from` to `to` following "blocks", if one exists.
/// The recursive query walks each edge reachable from `from` once.
async fn blocking_path(
    transaction: &mut Transaction<'_, Postgres>,
    from: Uuid,
    to: Uuid,
) -> Result<Option<Vec<Uuid>>, AppError> {
    let edges = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        WITH RECURSIVE reach(node, via) AS (
            SELECT dependency.blocked_work_item_id, dependency.blocking_work_item_id
            FROM work_item_dependencies dependency
            WHERE dependency.blocking_work_item_id = $1
            UNION
            SELECT dependency.blocked_work_item_id, dependency.blocking_work_item_id
            FROM work_item_dependencies dependency
            JOIN reach ON dependency.blocking_work_item_id = reach.node
        )
        SELECT via, node FROM reach
        "#,
    )
    .bind(from)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(shortest_path(&edges, from, to))
}

/// Breadth-first path along directed `edges`, start and end included.
pub fn shortest_path(edges: &[(Uuid, Uuid)], from: Uuid, to: Uuid) -> Option<Vec<Uuid>> {
    let mut graph: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (start, end) in edges {
        graph.entry(*start).or_default().push(*end);
    }
    let mut previous = HashMap::from([(from, from)]);
    let mut queue = VecDeque::from([from]);
    while let Some(node) = queue.pop_front() {
        if node == to {
            break;
        }
        for next in graph.get(&node).into_iter().flatten() {
            if let std::collections::hash_map::Entry::Vacant(entry) = previous.entry(*next) {
                entry.insert(node);
                queue.push_back(*next);
            }
        }
    }
    if !previous.contains_key(&to) {
        return None;
    }
    let mut path = vec![to];
    let mut node = to;
    while node != from {
        node = previous[&node];
        path.push(node);
    }
    path.reverse();
    Some(path)
}

/// What a dependency or relation change did.
#[derive(Clone, Debug, Default)]
pub struct LinkOutcome {
    pub changed: usize,
    /// The task's version after the change.
    pub version: i64,
}

/// Adds or removes dependencies between `task_id` and `others`. The caller
/// has resolved `others` for the actor (`resolve_reference`) and checked the
/// actor may edit tasks in `actor.project_id`, the task's project.
pub async fn change_dependencies(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
    others: &[(Uuid, Direction)],
    remove: bool,
) -> Result<Result<LinkOutcome, (usize, LinkRefusal)>, AppError> {
    lock_workspace_links(transaction, actor.project_id).await?;
    let mut ids = others.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    ids.push(task_id);
    let ends = lock_ends(transaction, &ids).await?;
    let Some(task) = ends.get(&task_id) else {
        return Ok(Err((0, LinkRefusal::NotFound)));
    };
    if task.project_id != actor.project_id {
        return Ok(Err((0, LinkRefusal::NotFound)));
    }
    let mut activities: HashMap<Uuid, Vec<(String, String, serde_json::Value)>> = HashMap::new();
    let mut changed = 0;
    for (index, (other_id, direction)) in others.iter().enumerate() {
        if *other_id == task_id {
            return Ok(Err((index, LinkRefusal::SelfLink)));
        }
        let Some(other) = ends.get(other_id) else {
            return Ok(Err((index, LinkRefusal::NotFound)));
        };
        let (blocking, blocked) = match direction {
            Direction::BlockedBy => (other, task),
            Direction::Blocks => (task, other),
        };
        let rows = if remove {
            sqlx::query(
                "DELETE FROM work_item_dependencies WHERE project_id = $1 AND blocking_work_item_id = $2 AND blocked_work_item_id = $3",
            )
            .bind(blocked.project_id)
            .bind(blocking.id)
            .bind(blocked.id)
            .execute(&mut **transaction)
            .await?
            .rows_affected()
        } else {
            if let Some(path) = blocking_path(transaction, blocked.id, blocking.id).await? {
                let keys = keys_for(transaction, actor, &path).await?;
                return Ok(Err((index, LinkRefusal::Cycle { path: keys })));
            }
            let (blockers, blocking_count) = sqlx::query_as::<_, (i64, i64)>(
                r#"
                SELECT
                    (SELECT count(*) FROM work_item_dependencies
                      WHERE blocked_work_item_id = $1 AND blocking_work_item_id <> $2),
                    (SELECT count(*) FROM work_item_dependencies
                      WHERE blocking_work_item_id = $2 AND blocked_work_item_id <> $1)
                "#,
            )
            .bind(blocked.id)
            .bind(blocking.id)
            .fetch_one(&mut **transaction)
            .await?;
            if blockers >= MAX_LINKS_PER_TASK || blocking_count >= MAX_LINKS_PER_TASK {
                let full = if blockers >= MAX_LINKS_PER_TASK {
                    blocked
                } else {
                    blocking
                };
                return Ok(Err((
                    index,
                    LinkRefusal::TooMany {
                        key: named(full, task),
                    },
                )));
            }
            let inserted = sqlx::query(
                r#"
                INSERT INTO work_item_dependencies (
                    project_id, blocking_work_item_id, blocked_work_item_id,
                    created_by_actor_id, blocking_project_id
                ) VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(blocked.project_id)
            .bind(blocking.id)
            .bind(blocked.id)
            .bind(actor.actor_id)
            .bind(blocking.project_id)
            .execute(&mut **transaction)
            .await;
            match inserted {
                Ok(result) => result.rows_affected(),
                Err(error) if is_check_violation(&error) => {
                    return Ok(Err((index, LinkRefusal::OtherWorkspace)));
                }
                Err(error) => return Err(error.into()),
            }
        };
        if rows == 0 {
            continue;
        }
        changed += 1;
        let (kind, blocked_words, blocking_words) = if remove {
            (
                "dependency_removed",
                "No longer blocked by",
                "No longer blocks",
            )
        } else {
            ("dependency_added", "Blocked by", "Blocks")
        };
        let payload = serde_json::json!({
            "blockingWorkItemId": blocking.id,
            "blockedWorkItemId": blocked.id,
        });
        activities.entry(blocked.id).or_default().push((
            kind.to_owned(),
            format!("{blocked_words} {}", named(blocking, blocked)),
            payload.clone(),
        ));
        activities.entry(blocking.id).or_default().push((
            kind.to_owned(),
            format!("{blocking_words} {}", named(blocked, blocking)),
            payload,
        ));
    }
    let version = record_link_activities(
        transaction,
        actor,
        &ends,
        activities,
        if remove {
            "work_item.dependencies_removed"
        } else {
            "work_item.dependencies_added"
        },
    )
    .await?
    .get(&task_id)
    .copied()
    .unwrap_or(task.version);
    Ok(Ok(LinkOutcome { changed, version }))
}

/// Writes each task's link activities under one version bump, live update
/// and audit row. A task in a project the actor isn't a member of still gets
/// its version and live update, without an activity in its name.
async fn record_link_activities(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    ends: &HashMap<Uuid, LockedEnd>,
    activities: HashMap<Uuid, Vec<(String, String, serde_json::Value)>>,
    audit_action: &str,
) -> Result<HashMap<Uuid, i64>, AppError> {
    let mut versions = HashMap::new();
    let mut ordered = activities.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(id, _)| *id);
    for (work_item_id, records) in ordered {
        let Some(end) = ends.get(&work_item_id) else {
            continue;
        };
        let version = sqlx::query_scalar::<_, i64>(
            "UPDATE work_items SET version = version + 1, updated_at = now() WHERE id = $1 RETURNING version",
        )
        .bind(work_item_id)
        .fetch_one(&mut **transaction)
        .await?;
        versions.insert(work_item_id, version);
        let member = end.project_id == actor.project_id
            || sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM project_memberships WHERE project_id = $1 AND actor_id = $2)",
            )
            .bind(end.project_id)
            .bind(actor.actor_id)
            .fetch_one(&mut **transaction)
            .await?;
        if !member {
            crate::work_item_commands::enqueue_update(
                transaction,
                end.project_id,
                work_item_id,
                version,
            )
            .await?;
            continue;
        }
        let scoped = CommandActor {
            project_id: end.project_id,
            ..actor.clone()
        };
        let activity_records = records
            .iter()
            .map(|(kind, summary, payload)| ActivityRecord {
                kind,
                summary,
                payload: payload.clone(),
            })
            .collect::<Vec<_>>();
        finalize_changes(
            transaction,
            &scoped,
            work_item_id,
            &activity_records,
            audit_action,
            serde_json::json!({ "changes": records.len(), "version": version }),
            version,
        )
        .await?;
    }
    Ok(versions)
}

/// The keys of `ids` in order; a task the actor can't see is named only as
/// one in another project.
async fn keys_for(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    ids: &[Uuid],
) -> Result<Vec<String>, AppError> {
    let keys = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT item.id,
               CASE
                   WHEN item.project_id = $2 OR actor_can_access_project($3, $4, item.project_id)
                   THEN project.project_key || '-' || item.number
                   ELSE 'a task in another project'
               END
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.id = ANY($1)
        "#,
    )
    .bind(ids)
    .bind(actor.project_id)
    .bind(actor.actor_id)
    .bind(connection_of(actor))
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .collect::<HashMap<_, _>>();
    Ok(ids
        .iter()
        .map(|id| keys.get(id).cloned().unwrap_or_else(|| id.to_string()))
        .collect())
}

fn is_check_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23514")
}

// ---------------------------------------------------------------------------
// Relations

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationKind {
    RelatesTo,
    Duplicates,
}

impl RelationKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "relates_to" | "relates" | "related" => Some(Self::RelatesTo),
            "duplicates" | "duplicate" | "duplicate_of" => Some(Self::Duplicates),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::RelatesTo => "relates_to",
            Self::Duplicates => "duplicates",
        }
    }
}

/// Adds or removes a relation between `task_id` and `other_id`; with `close`
/// a new duplicate also moves to the workflow's first canceled status.
pub async fn change_relation(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
    other_id: Uuid,
    kind: RelationKind,
    remove: bool,
    close: bool,
) -> Result<Result<LinkOutcome, LinkRefusal>, AppError> {
    if task_id == other_id {
        return Ok(Err(LinkRefusal::SelfLink));
    }
    lock_workspace_links(transaction, actor.project_id).await?;
    let ends = lock_ends(transaction, &[task_id, other_id]).await?;
    let (Some(task), Some(other)) = (ends.get(&task_id), ends.get(&other_id)) else {
        return Ok(Err(LinkRefusal::NotFound));
    };
    if task.project_id != actor.project_id {
        return Ok(Err(LinkRefusal::NotFound));
    }
    let rows = if remove {
        sqlx::query(
            r#"
            DELETE FROM work_item_relations
            WHERE kind = $1
              AND ((work_item_id = $2 AND other_work_item_id = $3)
                   OR ($1 = 'relates_to' AND work_item_id = $3 AND other_work_item_id = $2))
            "#,
        )
        .bind(kind.as_str())
        .bind(task_id)
        .bind(other_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        if kind == RelationKind::Duplicates
            && let Some(original) = sqlx::query_scalar::<_, String>(
                r#"
                SELECT CASE
                    WHEN item.project_id = $3 OR actor_can_access_project($4, $5, item.project_id)
                    THEN project.project_key || '-' || item.number
                    ELSE 'a task in another project'
                END
                FROM work_item_relations relation
                JOIN work_items item ON item.id = relation.other_work_item_id
                JOIN projects project ON project.id = item.project_id
                WHERE relation.kind = 'duplicates' AND relation.work_item_id = $1
                  AND relation.other_work_item_id <> $2
                "#,
            )
            .bind(task_id)
            .bind(other_id)
            .bind(task.project_id)
            .bind(actor.actor_id)
            .bind(connection_of(actor))
            .fetch_optional(&mut **transaction)
            .await?
        {
            return Ok(Err(LinkRefusal::AlreadyDuplicate { original }));
        }
        for end in [task, other] {
            let count = sqlx::query_scalar::<_, i64>(
                r#"
                SELECT count(*) FROM work_item_relations
                WHERE (work_item_id = $1 OR other_work_item_id = $1)
                  AND NOT ((work_item_id = $1 AND other_work_item_id = $2)
                           OR (work_item_id = $2 AND other_work_item_id = $1))
                "#,
            )
            .bind(end.id)
            .bind(if end.id == task.id { other.id } else { task.id })
            .fetch_one(&mut **transaction)
            .await?;
            if count >= MAX_LINKS_PER_TASK {
                return Ok(Err(LinkRefusal::TooMany {
                    key: named(end, task),
                }));
            }
        }
        let inserted = sqlx::query(
            r#"
            INSERT INTO work_item_relations (
                id, kind, project_id, work_item_id, other_project_id, other_work_item_id,
                created_by_actor_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(kind.as_str())
        .bind(task.project_id)
        .bind(task.id)
        .bind(other.project_id)
        .bind(other.id)
        .bind(actor.actor_id)
        .execute(&mut **transaction)
        .await;
        match inserted {
            Ok(result) => result.rows_affected(),
            Err(error) if is_check_violation(&error) => {
                return Ok(Err(LinkRefusal::OtherWorkspace));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let mut activities: HashMap<Uuid, Vec<(String, String, serde_json::Value)>> = HashMap::new();
    if rows > 0 {
        let kind_name = if remove {
            "relation_removed"
        } else {
            "relation_added"
        };
        let payload = serde_json::json!({
            "relation": kind.as_str(),
            "workItemId": task.id,
            "otherWorkItemId": other.id,
        });
        let (task_words, other_words) = match (kind, remove) {
            (RelationKind::RelatesTo, false) => ("Related to", "Related to"),
            (RelationKind::RelatesTo, true) => ("No longer related to", "No longer related to"),
            (RelationKind::Duplicates, false) => ("Marked a duplicate of", "Duplicated by"),
            (RelationKind::Duplicates, true) => {
                ("No longer a duplicate of", "No longer duplicated by")
            }
        };
        activities.entry(task.id).or_default().push((
            kind_name.to_owned(),
            format!("{task_words} {}", named(other, task)),
            payload.clone(),
        ));
        activities.entry(other.id).or_default().push((
            kind_name.to_owned(),
            format!("{other_words} {}", named(task, other)),
            payload,
        ));
    }
    let versions = record_link_activities(
        transaction,
        actor,
        &ends,
        activities,
        if remove {
            "work_item.relation_removed"
        } else {
            "work_item.relation_added"
        },
    )
    .await?;
    let mut version = versions.get(&task_id).copied().unwrap_or(task.version);
    if kind == RelationKind::Duplicates && close && !remove {
        match close_as_duplicate(transaction, actor, task_id).await? {
            Ok(Some(closed)) => version = closed,
            Ok(None) => {}
            Err(refusal) => return Ok(Err(refusal)),
        }
    }
    Ok(Ok(LinkOutcome {
        changed: usize::try_from(rows).unwrap_or(0),
        version,
    }))
}

/// Moves a task to the workflow's first canceled status, unless it's already
/// finished. Some(version) when it moved.
async fn close_as_duplicate(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
) -> Result<Result<Option<i64>, LinkRefusal>, AppError> {
    let Some(current) = lock_work_item(transaction, actor.project_id, task_id).await? else {
        return Ok(Err(LinkRefusal::NotFound));
    };
    let (category, status_name) = sqlx::query_as::<_, (String, String)>(
        "SELECT semantic_category, name FROM work_item_statuses WHERE project_id = $1 AND id = $2",
    )
    .bind(actor.project_id)
    .bind(current.status_id)
    .fetch_one(&mut **transaction)
    .await?;
    if matches!(category.as_str(), "completed" | "canceled") {
        return Ok(Ok(None));
    }
    let Some(canceled) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_statuses WHERE project_id = $1 AND enabled AND semantic_category = 'canceled' ORDER BY position, id LIMIT 1",
    )
    .bind(actor.project_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Err(LinkRefusal::CantClose {
            status: status_name,
        }));
    };
    let expected_version = current.version;
    let outcome = update_locked_work_item(
        transaction,
        actor,
        current,
        UpdateWorkItemCommand {
            expected_version,
            title: None,
            description: None,
            type_id: None,
            status_id: Some(canceled),
            priority: None,
            assignee_actor_id: None,
            parent_work_item_id: None,
            board_placement: None,
            labels: None,
            fields: TaskFieldChanges::default(),
            summary: "Closed as a duplicate",
            audit_action: "work_item.closed_as_duplicate",
            audit_version_metadata: crate::work_item_commands::AuditVersionMetadata::Include,
        },
    )
    .await?;
    match outcome {
        UpdateWorkItemOutcome::Applied { version, .. } => Ok(Ok(Some(version))),
        UpdateWorkItemOutcome::Noop(_) => Ok(Ok(None)),
        UpdateWorkItemOutcome::InvalidTransition { .. } => Ok(Err(LinkRefusal::CantClose {
            status: status_name,
        })),
        _ => Err(AppError::Internal("close a duplicate")),
    }
}

// ---------------------------------------------------------------------------
// Watchers

/// Starts or stops `actor_id` watching a task. Returns whether anything
/// changed.
pub async fn set_watching(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    watching: bool,
) -> Result<bool, AppError> {
    let rows = if watching {
        sqlx::query(
            r#"
            INSERT INTO work_item_watchers (project_id, work_item_id, actor_id)
            SELECT $1, item.id, $3 FROM work_items item WHERE item.project_id = $1 AND item.id = $2
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(project_id)
        .bind(task_id)
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?
    } else {
        sqlx::query(
            "DELETE FROM work_item_watchers WHERE project_id = $1 AND work_item_id = $2 AND actor_id = $3",
        )
        .bind(project_id)
        .bind(task_id)
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?
    };
    Ok(rows.rows_affected() > 0)
}

pub async fn watchers(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<ActorView>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String, String, String)>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM work_item_watchers watcher
        JOIN actors actor ON actor.id = watcher.actor_id
        JOIN project_memberships membership
          ON membership.project_id = watcher.project_id AND membership.actor_id = watcher.actor_id
        WHERE watcher.project_id = $1 AND watcher.work_item_id = $2 AND membership.enabled
        ORDER BY lower(membership.display_name), actor.id
        LIMIT 100
        "#,
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, kind, display_name, handle)| ActorView {
        id,
        kind,
        display_name,
        handle,
        avatar_url: None,
    })
    .collect())
}

// ---------------------------------------------------------------------------
// Milestones

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneView {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub target_date: Option<NaiveDate>,
    pub archived: bool,
    pub total: i64,
    pub finished: i64,
    pub open: i64,
    /// Finished over total, 0 when it has no tasks.
    pub progress: f64,
    /// Past its target date with tasks still open.
    pub overdue: bool,
    pub version: i64,
}

pub const MILESTONE_SELECT: &str = r#"
    SELECT milestone.id, milestone.name, milestone.description, milestone.target_date,
           milestone.archived_at IS NOT NULL AS archived,
           count(item.id) AS total,
           count(item.id) FILTER (WHERE status.semantic_category IN ('completed', 'canceled')) AS finished,
           count(item.id) FILTER (WHERE status.semantic_category NOT IN ('completed', 'canceled')) AS open,
           COALESCE(
               (count(item.id) FILTER (WHERE status.semantic_category IN ('completed', 'canceled')))::float8
                   / NULLIF(count(item.id), 0)::float8,
               0)::float8 AS progress,
           COALESCE(milestone.target_date < current_date
               AND count(item.id) FILTER (WHERE status.semantic_category NOT IN ('completed', 'canceled')) > 0,
               false) AS overdue,
           milestone.version
    FROM project_milestones milestone
    LEFT JOIN work_items item
      ON item.project_id = milestone.project_id AND item.milestone_id = milestone.id
     AND item.archived_at IS NULL
    LEFT JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
"#;

/// The most milestones a project has, archived ones included; every list
/// shows them all.
pub const MAX_MILESTONES: i64 = 500;

pub async fn list_milestones(
    connection: &mut PgConnection,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<MilestoneView>, AppError> {
    Ok(sqlx::query_as::<_, MilestoneView>(&format!(
        r#"{MILESTONE_SELECT}
        WHERE milestone.project_id = $1 AND ($2 OR milestone.archived_at IS NULL)
        GROUP BY milestone.id
        ORDER BY milestone.archived_at IS NOT NULL, milestone.target_date NULLS LAST,
                 lower(milestone.name), milestone.id
        LIMIT $3"#
    ))
    .bind(project_id)
    .bind(include_archived)
    .bind(MAX_MILESTONES)
    .fetch_all(&mut *connection)
    .await?)
}

pub async fn milestone(
    connection: &mut PgConnection,
    project_id: Uuid,
    milestone_id: Uuid,
) -> Result<Option<MilestoneView>, AppError> {
    Ok(sqlx::query_as::<_, MilestoneView>(&format!(
        "{MILESTONE_SELECT} WHERE milestone.project_id = $1 AND milestone.id = $2 GROUP BY milestone.id"
    ))
    .bind(project_id)
    .bind(milestone_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A milestone change. Name and description are already checked.
#[derive(Clone, Debug, Default)]
pub struct MilestoneChange {
    pub name: Option<String>,
    pub description: Option<String>,
    pub target_date: Option<Option<NaiveDate>>,
    pub archived: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MilestoneRefusal {
    NotFound,
    VersionConflict { current_version: i64 },
    NameTaken,
}

pub fn normalize_milestone_name(value: &str) -> Option<String> {
    let name = value.trim();
    (!name.is_empty() && name.chars().count() <= 120 && !name.chars().any(char::is_control))
        .then(|| name.to_owned())
}

pub fn normalize_milestone_description(value: &str) -> Option<String> {
    let text = value.trim();
    (text.chars().count() <= 2000).then(|| text.to_owned())
}

/// Creates a milestone, or returns the active one with that name.
pub async fn create_milestone(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    name: &str,
    description: &str,
    target_date: Option<NaiveDate>,
) -> Result<(Uuid, bool), AppError> {
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM project_milestones WHERE project_id = $1 AND archived_at IS NULL AND lower(name) = lower($2)",
    )
    .bind(actor.project_id)
    .bind(name)
    .fetch_optional(&mut **transaction)
    .await?
    {
        return Ok((id, false));
    }
    // Lists show every milestone at once, so a project keeps at most that
    // many rather than have some go missing from them.
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM project_milestones WHERE project_id = $1",
    )
    .bind(actor.project_id)
    .fetch_one(&mut **transaction)
    .await?;
    if existing >= MAX_MILESTONES {
        return Err(AppError::Conflict(
            "milestone",
            "A project can have at most 500 milestones, archived ones included.",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO project_milestones (id, project_id, name, description, target_date, created_by_actor_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(actor.project_id)
    .bind(name)
    .bind(description)
    .bind(target_date)
    .bind(actor.actor_id)
    .execute(&mut **transaction)
    .await?;
    crate::work_item_commands::record_audit_for(
        transaction,
        actor,
        "milestone",
        "milestone.created",
        id,
        serde_json::json!({}),
    )
    .await?;
    Ok((id, true))
}

pub async fn update_milestone(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    milestone_id: Uuid,
    expected_version: Option<i64>,
    change: &MilestoneChange,
) -> Result<Result<(), MilestoneRefusal>, AppError> {
    let Some(version) = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_milestones WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(milestone_id)
    .bind(actor.project_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Err(MilestoneRefusal::NotFound));
    };
    if let Some(expected) = expected_version
        && expected != version
    {
        return Ok(Err(MilestoneRefusal::VersionConflict {
            current_version: version,
        }));
    }
    let updated = sqlx::query(
        r#"
        UPDATE project_milestones
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            target_date = CASE WHEN $5 THEN $6 ELSE target_date END,
            archived_at = CASE WHEN $7::boolean IS NULL THEN archived_at
                               WHEN $7 THEN COALESCE(archived_at, now())
                               ELSE NULL END,
            version = version + 1, updated_at = now()
        WHERE id = $1 AND project_id = $2
        "#,
    )
    .bind(milestone_id)
    .bind(actor.project_id)
    .bind(&change.name)
    .bind(&change.description)
    .bind(change.target_date.is_some())
    .bind(change.target_date.flatten())
    .bind(change.archived)
    .execute(&mut **transaction)
    .await;
    if let Err(error) = updated {
        if error
            .as_database_error()
            .and_then(|database| database.constraint())
            == Some("project_milestones_active_name_unique")
        {
            return Ok(Err(MilestoneRefusal::NameTaken));
        }
        return Err(error.into());
    }
    crate::work_item_commands::record_audit_for(
        transaction,
        actor,
        "milestone",
        "milestone.updated",
        milestone_id,
        serde_json::json!({
            "renamed": change.name.is_some(),
            "targetDate": change.target_date.is_some(),
            "archived": change.archived,
        }),
    )
    .await?;
    Ok(Ok(()))
}

/// Puts tasks into a milestone (`Some`) or takes them out of `milestone_id`
/// (`None` as target), each as an ordinary task change with its history
/// entry. Tasks already there, or archived, are skipped. Returns how many
/// changed.
pub async fn move_tasks_to_milestone(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_ids: &[Uuid],
    target: Option<Uuid>,
    only_from: Option<Uuid>,
) -> Result<usize, AppError> {
    let mut sorted = task_ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut changed = 0;
    for task_id in sorted {
        let Some(current) = lock_work_item(transaction, actor.project_id, task_id).await? else {
            continue;
        };
        if current.archived
            || current.milestone_id == target
            || only_from.is_some_and(|from| current.milestone_id != Some(from))
        {
            continue;
        }
        let expected_version = current.version;
        let outcome = update_locked_work_item(
            transaction,
            actor,
            current,
            UpdateWorkItemCommand {
                expected_version,
                title: None,
                description: None,
                type_id: None,
                status_id: None,
                priority: None,
                assignee_actor_id: None,
                parent_work_item_id: None,
                board_placement: None,
                labels: None,
                fields: TaskFieldChanges {
                    milestone_id: Some(target),
                    ..TaskFieldChanges::default()
                },
                summary: "Milestone changed",
                audit_action: match actor.audit {
                    AuditAttribution::Browser { .. } => "work_item.milestone_changed",
                    AuditAttribution::Mcp { .. } => "work_item.mcp_milestone_changed",
                },
                audit_version_metadata: crate::work_item_commands::AuditVersionMetadata::Include,
            },
        )
        .await?;
        if matches!(outcome, UpdateWorkItemOutcome::Applied { .. }) {
            changed += 1;
        }
    }
    Ok(changed)
}

/// The IDs a set of task references named but that weren't found, for a
/// precise refusal.
pub fn missing<'a>(references: &'a [String], found: &[Option<(Uuid, Uuid)>]) -> Option<&'a str> {
    references
        .iter()
        .zip(found)
        .find(|(_, found)| found.is_none())
        .map(|(reference, _)| reference.as_str())
}

/// Distinct project IDs of resolved references.
pub fn projects_of(found: &[Option<(Uuid, Uuid)>]) -> HashSet<Uuid> {
    found
        .iter()
        .flatten()
        .map(|(_, project)| *project)
        .collect()
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{RelationKind, normalize_milestone_name, shortest_path};

    #[test]
    fn finds_the_shortest_blocking_path() {
        let ids = (0..5).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
        let edges = vec![
            (ids[0], ids[1]),
            (ids[1], ids[2]),
            (ids[2], ids[3]),
            (ids[0], ids[3]),
            (ids[3], ids[4]),
        ];
        assert_eq!(
            shortest_path(&edges, ids[0], ids[4]),
            Some(vec![ids[0], ids[3], ids[4]])
        );
        assert_eq!(shortest_path(&edges, ids[4], ids[0]), None);
    }

    #[test]
    fn relation_kinds_parse_in_plain_words() {
        assert_eq!(
            RelationKind::parse("relates_to"),
            Some(RelationKind::RelatesTo)
        );
        assert_eq!(
            RelationKind::parse("duplicate_of"),
            Some(RelationKind::Duplicates)
        );
        assert_eq!(RelationKind::parse("blocks"), None);
    }

    #[test]
    fn milestone_names_are_one_trimmed_line() {
        assert_eq!(normalize_milestone_name("  Beta "), Some("Beta".to_owned()));
        assert_eq!(normalize_milestone_name(""), None);
        assert_eq!(normalize_milestone_name("a\nb"), None);
    }
}

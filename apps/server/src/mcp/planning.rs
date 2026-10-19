//! Planning tools (docs/plans/mcp-tools.md, Phase 5; roadmap T1 to T3):
//! which tasks block which, how tasks relate, who watches a task, and
//! milestones that group tasks toward a target date. Every change goes
//! through `work_item_planning`, the same code the web app calls, so a
//! dependency or a milestone move writes the same history either way.
//!
//! A blocker may be in another project of the same workspace. Other tasks
//! are found and shown only among projects the caller can reach right now
//! (`accessible_project_ids`, which goes through `actor_can_access_project`),
//! so a task in a project the caller isn't in reads as not found.

use chrono::NaiveDate;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, accessible_project_ids_in, archived_task_message, parse_id, public_url},
    tasks::{command_actor, resolve_tasks},
    writes::{WriteAccess, check_access_for, check_write_access},
};
use crate::{
    AppState,
    error::AppError,
    policy::Action,
    work_item_commands::valid_estimate,
    work_item_planning::{
        self, Direction, LinkRefusal, MAX_LINKS_PER_CALL, MilestoneChange, MilestoneRefusal,
        MilestoneView, Reader, RelationKind,
    },
};

pub(super) const DATE_MESSAGE: &str = "Use YYYY-MM-DD, or \"\" to clear it.";
pub(super) const ESTIMATE_MESSAGE: &str =
    "Use an estimate from 0 to 100000 (points or hours, as the project counts them).";

fn access_refusal<T>(access: WriteAccess) -> Option<ToolResult<T>> {
    match access {
        WriteAccess::Allowed => None,
        WriteAccess::ReadOnly => Some(ToolResult::read_only()),
        WriteAccess::NotFound => Some(ToolResult::not_found("That project")),
        WriteAccess::ProjectArchived => Some(ToolResult::conflict("That project is archived.")),
        WriteAccess::Denied => Some(ToolResult::denied(
            "You can't change tasks in that project.",
        )),
    }
}

// ---------------------------------------------------------------------------
// Shared argument readers, also used by task_create, task_update and
// task_update_many.

/// A date argument: YYYY-MM-DD, or "" (or "none") to clear it. None when it
/// isn't a date.
pub(super) fn date_change(value: &str) -> Option<Option<NaiveDate>> {
    match value.trim() {
        "" => Some(None),
        value if value.eq_ignore_ascii_case("none") => Some(None),
        value => NaiveDate::parse_from_str(value, "%Y-%m-%d").ok().map(Some),
    }
}

/// An estimate and its clear flag: `Some(None)` clears it. The message names
/// what's wrong.
pub(super) fn estimate_change(
    estimate: Option<f64>,
    clear: Option<bool>,
) -> Result<Option<Option<f64>>, &'static str> {
    match (estimate, clear == Some(true)) {
        (Some(_), true) => Err("Pass estimate or clearEstimate, not both."),
        (None, true) => Ok(Some(None)),
        (Some(estimate), false) if valid_estimate(estimate) => Ok(Some(Some(estimate))),
        (Some(_), false) => Err(ESTIMATE_MESSAGE),
        (None, false) => Ok(None),
    }
}

/// A milestone named by an agent: an active milestone of the project by ID
/// or name (any case), or "none" (or "") to take the task out of its
/// milestone. None when nothing matches.
pub(super) async fn milestone_change(
    connection: &mut PgConnection,
    project_id: Uuid,
    value: &str,
) -> Result<Option<Option<Uuid>>, AppError> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Ok(Some(None));
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM project_milestones
        WHERE project_id = $1 AND archived_at IS NULL
          AND (id = $2 OR lower(name) = lower($3))
        ORDER BY id = $2 DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(parse_id(value))
    .bind(value)
    .fetch_optional(&mut *connection)
    .await?
    .map(Some))
}

// ---------------------------------------------------------------------------
// What a caller sees of a task's links

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskRef {
    pub id: String,
    pub key: String,
    pub title: String,
    pub status: String,
    /// backlog, unstarted, started, review, verification, completed, or
    /// canceled.
    #[serde(default)]
    pub status_group: String,
    /// True once it's done or canceled.
    pub finished: bool,
}

impl From<work_item_planning::TaskRef> for TaskRef {
    fn from(task: work_item_planning::TaskRef) -> Self {
        Self {
            id: task.id.to_string(),
            key: task.key,
            title: task.title,
            status: task.status_name,
            status_group: task.status_category,
            finished: task.finished,
        }
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RelationRef {
    /// relates_to, duplicates (this task duplicates the other) or
    /// duplicated_by.
    pub kind: String,
    pub task: TaskRef,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WatcherRef {
    pub name: String,
    pub handle: String,
    /// person or agent.
    pub kind: String,
}

/// A task's blockers, the tasks it blocks and its relations, limited to
/// projects the caller can reach. `ready` counts every blocker, seen or not.
pub(super) struct VisibleLinks {
    pub blocked_by: Vec<TaskRef>,
    pub blocks: Vec<TaskRef>,
    pub relations: Vec<RelationRef>,
    pub ready: bool,
}

pub(super) fn reader(principal: &McpPrincipal) -> Reader {
    Reader {
        actor_id: principal.actor_id,
        connection_id: principal.external_agent_connection_id,
    }
}

pub(super) async fn visible_links(
    connection: &mut PgConnection,
    principal: &McpPrincipal,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<VisibleLinks, AppError> {
    let links =
        work_item_planning::task_links(connection, reader(principal), project_id, task_id).await?;
    let reachable = accessible_project_ids_in(&mut *connection, principal).await?;
    let seen = |task: &work_item_planning::TaskRef| {
        task.project_id == project_id || reachable.contains(&task.project_id)
    };
    Ok(VisibleLinks {
        blocked_by: links
            .blocked_by
            .into_iter()
            .filter(|task| seen(task))
            .map(TaskRef::from)
            .collect(),
        blocks: links
            .blocks
            .into_iter()
            .filter(|task| seen(task))
            .map(TaskRef::from)
            .collect(),
        relations: links
            .relations
            .into_iter()
            .filter(|relation| seen(&relation.task))
            .map(|relation| RelationRef {
                kind: relation.kind,
                task: relation.task.into(),
            })
            .collect(),
        ready: links.ready,
    })
}

pub(super) async fn visible_watchers(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<WatcherRef>, AppError> {
    Ok(
        work_item_planning::watchers(connection, project_id, task_id)
            .await?
            .into_iter()
            .map(|actor| WatcherRef {
                name: actor.display_name,
                handle: actor.handle,
                kind: match actor.kind.as_str() {
                    "human" => "person".to_owned(),
                    "external_agent" => "agent".to_owned(),
                    other => other.to_owned(),
                },
            })
            .collect(),
    )
}

/// One task of this project as a reference.
pub(super) async fn own_task_ref(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Option<TaskRef>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String, String, String, String)>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number, item.title,
               status.name, status.semantic_category
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_optional(&mut *connection)
    .await?
    .map(|(id, key, title, status, category)| TaskRef {
        id: id.to_string(),
        key,
        title,
        status,
        finished: matches!(category.as_str(), "completed" | "canceled"),
        status_group: category,
    }))
}

/// Finds other tasks for a link, in this workspace and among projects the
/// caller can reach. Err names the first reference that matched nothing.
async fn resolve_linked(
    connection: &mut PgConnection,
    principal: &McpPrincipal,
    project_id: Uuid,
    references: &[String],
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    let reachable = accessible_project_ids_in(&mut *connection, principal).await?;
    let mut found = Vec::with_capacity(references.len());
    for reference in references {
        match work_item_planning::resolve_reference(
            &mut *connection,
            reader(principal),
            project_id,
            reference,
        )
        .await?
        {
            Some((task, project)) if project == project_id || reachable.contains(&project) => {
                found.push(task);
            }
            _ => return Ok(Err(reference.clone())),
        }
    }
    Ok(Ok(found))
}

/// Why a task can't take link changes, if it can't: missing from the
/// project, or archived (an archived task refuses every change but a restore).
async fn link_target_refusal<T>(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Option<ToolResult<T>>, AppError> {
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT project.project_key || '-' || item.number::text, item.archived_at IS NOT NULL \
         FROM work_items item JOIN projects project ON project.id = item.project_id \
         WHERE item.id = $1 AND item.project_id = $2",
    )
    .bind(task_id)
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(match row {
        None => Some(ToolResult::not_found("That task")),
        Some((key, true)) => Some(ToolResult::conflict(archived_task_message(&key))),
        Some((_, false)) => None,
    })
}

fn link_refusal<T>(field: &str, reference: &str, refusal: &LinkRefusal) -> ToolResult<T> {
    let message = refusal.message(reference);
    match refusal {
        LinkRefusal::NotFound => ToolResult::NotFound { message },
        LinkRefusal::SelfLink | LinkRefusal::OtherWorkspace => ToolResult::invalid(field, message),
        LinkRefusal::Cycle { .. }
        | LinkRefusal::AlreadyDuplicate { .. }
        | LinkRefusal::CantClose { .. }
        | LinkRefusal::TooMany { .. }
        | LinkRefusal::Archived { .. } => ToolResult::conflict(message),
    }
}

// ---------------------------------------------------------------------------
// task_depend

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskDependArguments {
    /// The task: ID, link, or key such as AWD-12.
    pub task_id: String,
    /// Tasks that must be done before this one (keys or IDs). They may be in
    /// another project of the same workspace that you can reach.
    pub blocked_by: Option<Vec<String>>,
    /// Tasks that can't start until this one is done.
    pub blocks: Option<Vec<String>>,
    /// True to remove these dependencies instead of adding them.
    pub remove: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct DependencyValue {
    pub task: TaskRef,
    /// What blocks it, each with its status. Tasks in projects you can't
    /// reach are left out.
    pub blocked_by: Vec<TaskRef>,
    pub blocks: Vec<TaskRef>,
    /// True when it isn't finished, isn't marked blocked, and everything
    /// blocking it is finished.
    pub ready: bool,
    /// How many dependencies this call added or removed.
    #[serde(default)]
    pub changed: u32,
    /// The task's version now.
    #[serde(default)]
    pub version: i64,
}

pub(super) async fn task_depend(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskDependArguments,
) -> ToolResult<DependencyValue> {
    let blocked_by = arguments.blocked_by.clone().unwrap_or_default();
    let blocks = arguments.blocks.clone().unwrap_or_default();
    let remove = arguments.remove.unwrap_or(false);
    if blocked_by.len() + blocks.len() > MAX_LINKS_PER_CALL {
        return ToolResult::invalid(
            "blockedBy",
            format!("Change at most {MAX_LINKS_PER_CALL} dependencies in one call."),
        );
    }
    if blocked_by.is_empty() && blocks.is_empty() {
        return ToolResult::invalid(
            "blockedBy",
            "Name the tasks in blockedBy or blocks (keys such as AWD-3, or IDs).",
        );
    }
    let Some(task_id) = parse_id(&arguments.task_id) else {
        return ToolResult::not_found("That task");
    };
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) =
            access_refusal(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(refusal);
        }
        if let Some(refusal) = link_target_refusal(&mut transaction, project_id, task_id).await? {
            return Ok(refusal);
        }
        let references = blocked_by
            .iter()
            .chain(blocks.iter())
            .cloned()
            .collect::<Vec<_>>();
        let others =
            match resolve_linked(&mut transaction, principal, project_id, &references).await? {
                Ok(others) => others,
                Err(reference) => {
                    return Ok(ToolResult::NotFound {
                        message: LinkRefusal::NotFound.message(&reference),
                    });
                }
            };
        let edges = others
            .iter()
            .enumerate()
            .map(|(index, other)| {
                let direction = if index < blocked_by.len() {
                    Direction::BlockedBy
                } else {
                    Direction::Blocks
                };
                (*other, direction)
            })
            .collect::<Vec<_>>();
        let field = |index: usize| {
            if index < blocked_by.len() {
                "blockedBy"
            } else {
                "blocks"
            }
        };
        let actor = command_actor(principal, project_id);
        let outcome = match work_item_planning::change_dependencies(
            &mut transaction,
            &actor,
            task_id,
            &edges,
            remove,
        )
        .await?
        {
            Ok(outcome) => outcome,
            Err((index, refusal)) => {
                let reference = references.get(index).map_or("", String::as_str);
                return Ok(link_refusal(field(index), reference, &refusal));
            }
        };
        let task = own_task_ref(&mut transaction, project_id, task_id)
            .await?
            .ok_or(AppError::NotFound("work item"))?;
        let links = visible_links(&mut transaction, principal, project_id, task_id).await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(DependencyValue {
            task,
            blocked_by: links.blocked_by,
            blocks: links.blocks,
            ready: links.ready,
            changed: u32::try_from(outcome.changed).unwrap_or(u32::MAX),
            version: outcome.version,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_depend"))
}

// ---------------------------------------------------------------------------
// task_relate

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskRelateArguments {
    /// The task: ID, link, or key such as AWD-12.
    pub task_id: String,
    /// The other task (ID, link, or key). It may be in another project of
    /// the same workspace that you can reach.
    pub other_task_id: String,
    /// relates_to (the default) or duplicates: this task duplicates the
    /// other one.
    pub kind: Option<String>,
    /// True to remove the relation instead of adding it.
    pub remove: Option<bool>,
    /// With duplicates: also close this task in the workflow's canceled
    /// status.
    pub close: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RelationsValue {
    pub task: TaskRef,
    /// The task's relations now. Tasks in projects you can't reach are left
    /// out.
    pub relations: Vec<RelationRef>,
    /// Whether this call changed anything.
    pub changed: bool,
    /// The task's version now.
    pub version: i64,
}

pub(super) async fn task_relate(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskRelateArguments,
) -> ToolResult<RelationsValue> {
    let kind = match arguments.kind.as_deref() {
        None => RelationKind::RelatesTo,
        Some(value) => match RelationKind::parse(value) {
            Some(kind) => kind,
            None => return ToolResult::invalid("kind", "Use relates_to or duplicates."),
        },
    };
    let remove = arguments.remove == Some(true);
    let close = arguments.close == Some(true);
    if close && (kind != RelationKind::Duplicates || remove) {
        return ToolResult::invalid(
            "close",
            "close only goes with kind: duplicates, when adding the relation.",
        );
    }
    let Some(task_id) = parse_id(&arguments.task_id) else {
        return ToolResult::not_found("That task");
    };
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) =
            access_refusal(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(refusal);
        }
        if let Some(refusal) = link_target_refusal(&mut transaction, project_id, task_id).await? {
            return Ok(refusal);
        }
        let reference = arguments.other_task_id.clone();
        let other = match resolve_linked(
            &mut transaction,
            principal,
            project_id,
            std::slice::from_ref(&reference),
        )
        .await?
        {
            Ok(found) => found[0],
            Err(reference) => {
                return Ok(ToolResult::NotFound {
                    message: LinkRefusal::NotFound.message(&reference),
                });
            }
        };
        let actor = command_actor(principal, project_id);
        let outcome = match work_item_planning::change_relation(
            &mut transaction,
            &actor,
            task_id,
            other,
            kind,
            remove,
            close,
        )
        .await?
        {
            Ok(outcome) => outcome,
            Err(refusal) => return Ok(link_refusal("otherTaskId", &reference, &refusal)),
        };
        let task = own_task_ref(&mut transaction, project_id, task_id)
            .await?
            .ok_or(AppError::NotFound("work item"))?;
        let links = visible_links(&mut transaction, principal, project_id, task_id).await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(RelationsValue {
            task,
            relations: links.relations,
            changed: outcome.changed > 0,
            version: outcome.version,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_relate"))
}

// ---------------------------------------------------------------------------
// task_watch

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskWatchArguments {
    /// The task: ID, link, or key such as AWD-12.
    pub task_id: String,
    /// true (the default) to watch it, false to stop.
    pub watching: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WatchValue {
    pub task: TaskRef,
    /// Whether you watch it now.
    pub watching: bool,
    /// Whether this call changed anything.
    pub changed: bool,
    /// Everyone watching it.
    pub watchers: Vec<WatcherRef>,
}

pub(super) async fn task_watch(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskWatchArguments,
) -> ToolResult<WatchValue> {
    let watching = arguments.watching.unwrap_or(true);
    let Some(task_id) = parse_id(&arguments.task_id) else {
        return ToolResult::not_found("That task");
    };
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        // Anyone who can see a task may watch it, as in the app.
        if let Some(refusal) = access_refusal(
            check_access_for(&mut transaction, principal, Action::WorkItemView).await?,
        ) {
            return Ok(refusal);
        }
        let Some(task) = own_task_ref(&mut transaction, project_id, task_id).await? else {
            return Ok(ToolResult::not_found("That task"));
        };
        let changed = work_item_planning::set_watching(
            &mut transaction,
            project_id,
            task_id,
            principal.actor_id,
            watching,
        )
        .await?;
        if changed {
            super::common::insert_audit(
                &mut transaction,
                principal,
                project_id,
                if watching {
                    "work_item.mcp_watched"
                } else {
                    "work_item.mcp_unwatched"
                },
                "work_item",
                task_id,
                serde_json::json!({}),
            )
            .await?;
        }
        let watchers = visible_watchers(&mut transaction, project_id, task_id).await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(WatchValue {
            task,
            watching,
            changed,
            watchers,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_watch"))
}

// ---------------------------------------------------------------------------
// milestones

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MilestoneListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Include archived milestones.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MilestoneValue {
    pub id: String,
    pub name: String,
    pub description: String,
    /// YYYY-MM-DD, if it has one.
    pub target_date: Option<String>,
    pub archived: bool,
    /// Its tasks, not counting canceled ones.
    pub total: i64,
    /// Completed tasks.
    pub finished: i64,
    /// Canceled tasks, left out of total and progress.
    #[serde(default)]
    pub canceled: i64,
    /// Open tasks past the target date make it overdue.
    pub overdue: bool,
    /// Pass as expectedVersion to milestone_write.
    pub version: i64,
    pub url: String,
    /// Tasks not yet finished.
    #[serde(default)]
    pub open: i64,
    /// Completed over total, from 0 to 1.
    #[serde(default)]
    pub progress: f64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MilestoneListValue {
    /// Active ones by target date (none last), then name; archived ones
    /// after them.
    pub milestones: Vec<MilestoneValue>,
}

fn milestone_value(state: &AppState, project_id: Uuid, view: MilestoneView) -> MilestoneValue {
    MilestoneValue {
        id: view.id.to_string(),
        url: public_url(
            state,
            &format!("/projects/{project_id}/work-items?milestoneId={}", view.id),
        ),
        name: view.name,
        description: view.description,
        target_date: view.target_date.map(|date| date.to_string()),
        archived: view.archived,
        total: view.total,
        finished: view.finished,
        canceled: view.canceled,
        overdue: view.overdue,
        version: view.version,
        open: view.open,
        progress: view.progress,
    }
}

pub(super) async fn milestone_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MilestoneListArguments,
) -> ToolResult<MilestoneListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        crate::memory::project_access(
            &state.pool,
            principal.actor_id,
            principal.actor_kind,
            principal.project_id,
            crate::policy::Action::WorkItemView,
        )
        .await?;
        let mut connection = state.pool.acquire().await?;
        let rows = work_item_planning::list_milestones(
            &mut connection,
            principal.project_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(MilestoneListValue {
            milestones: rows
                .into_iter()
                .map(|row| milestone_value(state, principal.project_id, row))
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "milestone_list"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MilestoneWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The milestone to change. Leave it out to create one (an active one with
    /// the same name is returned instead).
    pub milestone_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// YYYY-MM-DD; "" clears it.
    pub target_date: Option<String>,
    /// True to archive, false to restore. Its tasks keep the milestone.
    pub archived: Option<bool>,
    /// Tasks to put in this milestone (keys or IDs). A task belongs to one
    /// milestone at a time, so this moves them from any other. Each move is
    /// in the task's history.
    pub add_tasks: Option<Vec<String>>,
    /// Tasks to take out of this milestone.
    pub remove_tasks: Option<Vec<String>>,
    /// The version you last read. Leave it out to change the current one.
    pub expected_version: Option<i64>,
}

pub(super) async fn milestone_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MilestoneWriteArguments,
) -> ToolResult<MilestoneValue> {
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let project_id = principal.project_id;
    let name = match arguments.name.as_deref() {
        Some(value) => match work_item_planning::normalize_milestone_name(value) {
            Some(name) => Some(name),
            None => {
                return ToolResult::invalid(
                    "name",
                    "Use a name of 1 to 120 characters, on one line.",
                );
            }
        },
        None => None,
    };
    let description = match arguments.description.as_deref() {
        Some(value) => match work_item_planning::normalize_milestone_description(value) {
            Some(text) => Some(text),
            None => {
                return ToolResult::invalid(
                    "description",
                    "Keep the description under 2,000 characters.",
                );
            }
        },
        None => None,
    };
    let target_date = match arguments.target_date.as_deref() {
        None => None,
        Some(value) => match date_change(value) {
            Some(date) => Some(date),
            None => return ToolResult::invalid("targetDate", DATE_MESSAGE),
        },
    };
    let add = arguments.add_tasks.clone().unwrap_or_default();
    let remove = arguments.remove_tasks.clone().unwrap_or_default();
    if add.len() + remove.len() > 200 {
        return ToolResult::invalid("addTasks", "Move at most 200 tasks in one call.");
    }
    let result = async {
        let references = add.iter().chain(remove.iter()).cloned().collect::<Vec<_>>();
        let found = resolve_tasks(state, principal, &references).await?;
        let mut task_ids = Vec::new();
        for (reference, item) in references.iter().zip(&found) {
            match item {
                Some((id, project)) if *project == project_id => task_ids.push(*id),
                Some(_) => {
                    return Ok(ToolResult::invalid(
                        "addTasks",
                        format!("{reference} is in another project; milestones hold tasks from their own project."),
                    ));
                }
                None => return Ok(ToolResult::not_found(&format!("Task {reference}"))),
            }
        }
        let (add_ids, remove_ids) = task_ids.split_at(add.len());
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) =
            access_refusal(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(refusal);
        }
        // An archived task takes no changes until it's restored; refuse
        // rather than skip it and report success. Locked so it stays so.
        let locked = sqlx::query_as::<_, (String, bool)>(
            "SELECT project.project_key || '-' || item.number::text, item.archived_at IS NOT NULL \
             FROM work_items item JOIN projects project ON project.id = item.project_id \
             WHERE item.id = ANY($1) AND item.project_id = $2 \
             ORDER BY item.id FOR UPDATE OF item",
        )
        .bind(task_ids.as_slice())
        .bind(project_id)
        .fetch_all(&mut *transaction)
        .await?;
        if let Some((key, _)) = locked.iter().find(|(_, archived)| *archived) {
            return Ok(ToolResult::conflict(archived_task_message(key)));
        }
        let actor = command_actor(principal, project_id);
        let milestone_id = match arguments.milestone_id.as_deref() {
            Some(reference) => {
                let Some(id) = parse_id(reference) else {
                    return Ok(ToolResult::invalid("milestoneId", "Pass the milestone's ID."));
                };
                let change = MilestoneChange {
                    name: name.clone(),
                    description: description.clone(),
                    target_date,
                    archived: arguments.archived,
                };
                match work_item_planning::update_milestone(
                    &mut transaction,
                    &actor,
                    id,
                    arguments.expected_version,
                    &change,
                )
                .await?
                {
                    Ok(()) => id,
                    Err(MilestoneRefusal::NotFound) => {
                        return Ok(ToolResult::not_found("That milestone"));
                    }
                    Err(MilestoneRefusal::VersionConflict { current_version }) => {
                        return Ok(ToolResult::conflict(format!(
                            "Someone changed this milestone first (now version {current_version}). Read it again, then retry."
                        )));
                    }
                    Err(MilestoneRefusal::NameTaken) => {
                        return Ok(ToolResult::conflict(
                            "Another active milestone already has that name.",
                        ));
                    }
                }
            }
            None => {
                let Some(name) = name.clone() else {
                    return Ok(ToolResult::invalid("name", "Give the new milestone a name."));
                };
                work_item_planning::create_milestone(
                    &mut transaction,
                    &actor,
                    &name,
                    description.as_deref().unwrap_or_default(),
                    target_date.flatten(),
                )
                .await?
                .0
            }
        };
        if !add_ids.is_empty() {
            // Tasks can't join an archived milestone; refuse rather than
            // archive it and quietly leave them out.
            let archived = work_item_planning::milestone(&mut transaction, project_id, milestone_id)
                .await?
                .is_some_and(|view| view.archived);
            if archived {
                return Ok(ToolResult::invalid(
                    "addTasks",
                    "Tasks can't be added to an archived milestone. Add them before archiving it, or restore it first.",
                ));
            }
            work_item_planning::move_tasks_to_milestone(
                &mut transaction,
                &actor,
                add_ids,
                Some(milestone_id),
                None,
            )
            .await?;
        }
        if !remove_ids.is_empty() {
            work_item_planning::move_tasks_to_milestone(
                &mut transaction,
                &actor,
                remove_ids,
                None,
                Some(milestone_id),
            )
            .await?;
        }
        let view = work_item_planning::milestone(&mut transaction, project_id, milestone_id)
            .await?
            .ok_or(AppError::NotFound("milestone"))?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(milestone_value(state, project_id, view)))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "milestone_write"))
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::{date_change, estimate_change};

    #[test]
    fn dates_read_as_agents_write_them() {
        assert_eq!(
            date_change(" 2026-10-20 "),
            Some(Some(NaiveDate::from_ymd_opt(2026, 10, 20).expect("date")))
        );
        assert_eq!(date_change(""), Some(None));
        assert_eq!(date_change("None"), Some(None));
        assert_eq!(date_change("20/10/2026"), None);
    }

    #[test]
    fn estimates_are_checked_before_the_database() {
        assert_eq!(estimate_change(Some(3.5), None), Ok(Some(Some(3.5))));
        assert_eq!(estimate_change(None, Some(true)), Ok(Some(None)));
        assert_eq!(estimate_change(None, None), Ok(None));
        assert!(estimate_change(Some(-1.0), None).is_err());
        assert!(estimate_change(Some(f64::NAN), None).is_err());
        assert!(estimate_change(Some(1.0), Some(true)).is_err());
    }
}

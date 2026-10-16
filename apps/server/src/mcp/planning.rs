//! Planning tools (docs/plans/mcp-tools.md, Phase 5): which tasks block
//! which, and milestones that group tasks toward a target date. Both stay
//! inside one project; a dependency that would make a cycle is refused with
//! the path that closes it.

use std::collections::{HashMap, VecDeque};

use chrono::NaiveDate;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, parse_id, public_url},
    tasks::resolve_tasks,
    writes::{WriteAccess, check_write_access},
};
use crate::{AppState, error::AppError};

const MAX_LINKS: usize = 50;

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
// task_depend

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskDependArguments {
    /// The task: ID, link, or key such as AWD-12.
    pub task_id: String,
    /// Tasks that must be done before this one (keys or IDs, same project).
    pub blocked_by: Option<Vec<String>>,
    /// Tasks that can't start until this one is done.
    pub blocks: Option<Vec<String>>,
    /// True to remove these dependencies instead of adding them.
    pub remove: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskRef {
    pub id: String,
    pub key: String,
    pub title: String,
    pub status: String,
    /// True once it's done or canceled.
    pub finished: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct DependencyValue {
    pub task: TaskRef,
    pub blocked_by: Vec<TaskRef>,
    pub blocks: Vec<TaskRef>,
    /// True when everything blocking it is finished.
    pub ready: bool,
}

const TASK_REF_SELECT: &str = r#"
    SELECT item.id::text AS id, project.project_key || '-' || item.number AS key,
           item.title, status.name AS status,
           status.semantic_category IN ('completed', 'canceled') AS finished
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
"#;

async fn dependency_value(
    executor: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<DependencyValue, AppError> {
    let task = sqlx::query_as::<_, TaskRef>(&format!("{TASK_REF_SELECT} WHERE item.id = $1"))
        .bind(task_id)
        .fetch_one(&mut **executor)
        .await?;
    let blocked_by = sqlx::query_as::<_, TaskRef>(&format!(
        r#"{TASK_REF_SELECT}
        JOIN work_item_dependencies dependency
          ON dependency.project_id = $1 AND dependency.blocking_work_item_id = item.id
        WHERE dependency.blocked_work_item_id = $2 AND item.archived_at IS NULL
        ORDER BY item.number"#
    ))
    .bind(project_id)
    .bind(task_id)
    .fetch_all(&mut **executor)
    .await?;
    let blocks = sqlx::query_as::<_, TaskRef>(&format!(
        r#"{TASK_REF_SELECT}
        JOIN work_item_dependencies dependency
          ON dependency.project_id = $1 AND dependency.blocked_work_item_id = item.id
        WHERE dependency.blocking_work_item_id = $2 AND item.archived_at IS NULL
        ORDER BY item.number"#
    ))
    .bind(project_id)
    .bind(task_id)
    .fetch_all(&mut **executor)
    .await?;
    let ready = blocked_by.iter().all(|task| task.finished);
    Ok(DependencyValue {
        task,
        blocked_by,
        blocks,
        ready,
    })
}

/// The keys along an existing path from `from` to `to` (following "blocks"),
/// if there is one: adding `to` blocks `from` would then close a cycle.
async fn path_between(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    from: Uuid,
    to: Uuid,
) -> Result<Option<Vec<String>>, AppError> {
    let edges = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT blocking_work_item_id, blocked_work_item_id
        FROM work_item_dependencies WHERE project_id = $1
        ORDER BY blocking_work_item_id, blocked_work_item_id
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    // Visit each task once. A depth cutoff can silently accept a long cycle;
    // enumerating every possible path also explodes on branching graphs.
    let mut graph: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (blocking, blocked) in edges {
        graph.entry(blocking).or_default().push(blocked);
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
        return Ok(None);
    }
    let mut path = vec![to];
    let mut node = to;
    while node != from {
        node = previous[&node];
        path.push(node);
    }
    path.reverse();
    let keys = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.id = ANY($1)
        "#,
    )
    .bind(&path)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(Some(
        path.iter()
            .map(|id| {
                keys.iter()
                    .find(|(key_id, _)| key_id == id)
                    .map_or_else(|| id.to_string(), |(_, key)| key.clone())
            })
            .collect(),
    ))
}

pub(super) async fn task_depend(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskDependArguments,
) -> ToolResult<DependencyValue> {
    let blocked_by = arguments.blocked_by.clone().unwrap_or_default();
    let blocks = arguments.blocks.clone().unwrap_or_default();
    let remove = arguments.remove.unwrap_or(false);
    if blocked_by.len() + blocks.len() > MAX_LINKS {
        return ToolResult::invalid(
            "blockedBy",
            format!("Change at most {MAX_LINKS} dependencies in one call."),
        );
    }
    let result = async {
        let Some(task_id) = parse_id(&arguments.task_id) else {
            return Ok(ToolResult::not_found("That task"));
        };
        let project_id = principal.project_id;
        let others = resolve_tasks(
            state,
            principal,
            &blocked_by.iter().chain(blocks.iter()).cloned().collect::<Vec<_>>(),
        )
        .await?;
        let mut edges = Vec::new();
        for (index, found) in others.iter().enumerate() {
            let reference = if index < blocked_by.len() {
                &blocked_by[index]
            } else {
                &blocks[index - blocked_by.len()]
            };
            let Some((other, other_project)) = found else {
                return Ok(ToolResult::not_found(&format!("Task {reference}")));
            };
            if *other_project != project_id {
                return Ok(ToolResult::invalid(
                    "blockedBy",
                    format!("{reference} is in another project; dependencies stay inside one project."),
                ));
            }
            if *other == task_id {
                return Ok(ToolResult::invalid("blockedBy", "A task can't block itself."));
            }
            // (blocking, blocked)
            edges.push(if index < blocked_by.len() {
                (*other, task_id, reference.clone())
            } else {
                (task_id, *other, reference.clone())
            });
        }
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) = access_refusal(check_write_access(&mut transaction, principal).await?) {
            return Ok(refusal);
        }
        let in_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_items WHERE id = $1 AND project_id = $2)",
        )
        .bind(task_id)
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !in_project {
            return Ok(ToolResult::not_found("That task"));
        }
        let mut changed = 0_u64;
        for (blocking, blocked, reference) in &edges {
            if remove {
                changed += sqlx::query(
                    "DELETE FROM work_item_dependencies WHERE project_id = $1 AND blocking_work_item_id = $2 AND blocked_work_item_id = $3",
                )
                .bind(project_id)
                .bind(blocking)
                .bind(blocked)
                .execute(&mut *transaction)
                .await?
                .rows_affected();
                continue;
            }
            if let Some(path) = path_between(&mut transaction, project_id, *blocked, *blocking).await? {
                return Ok(ToolResult::conflict(format!(
                    "That would make a cycle with {reference}: {} already leads back. Remove one of those dependencies first.",
                    path.join(" blocks ")
                )));
            }
            changed += sqlx::query(
                r#"
                INSERT INTO work_item_dependencies (
                    project_id, blocking_work_item_id, blocked_work_item_id, created_by_actor_id
                ) VALUES ($1, $2, $3, $4)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(project_id)
            .bind(blocking)
            .bind(blocked)
            .bind(principal.actor_id)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        }
        if changed > 0 {
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                if remove {
                    "work_item.mcp_dependencies_removed"
                } else {
                    "work_item.mcp_dependencies_added"
                },
                "work_item",
                task_id,
                serde_json::json!({ "changed": changed }),
            )
            .await?;
        }
        let value = dependency_value(&mut transaction, project_id, task_id).await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_depend"))
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

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct MilestoneValue {
    pub id: String,
    pub name: String,
    pub description: String,
    /// YYYY-MM-DD, if it has one.
    pub target_date: Option<String>,
    pub archived: bool,
    pub total: i64,
    pub finished: i64,
    /// Open tasks past the target date make it overdue.
    pub overdue: bool,
    /// Pass as expectedVersion to milestone_write.
    pub version: i64,
    #[sqlx(skip)]
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MilestoneListValue {
    /// By target date (none last), then name.
    pub milestones: Vec<MilestoneValue>,
}

const MILESTONE_SELECT: &str = r#"
    SELECT milestone.id::text AS id, milestone.name, milestone.description,
           milestone.target_date::text AS target_date,
           milestone.archived_at IS NOT NULL AS archived,
           count(item.id) AS total,
           count(item.id) FILTER (WHERE status.semantic_category IN ('completed', 'canceled')) AS finished,
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

fn with_url(state: &AppState, project_id: Uuid, mut value: MilestoneValue) -> MilestoneValue {
    value.url = public_url(
        state,
        &format!("/projects/{project_id}/work-items?milestone={}", value.id),
    );
    value
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
        let rows = sqlx::query_as::<_, MilestoneValue>(&format!(
            r#"{MILESTONE_SELECT}
            WHERE milestone.project_id = $1 AND ($2 OR milestone.archived_at IS NULL)
            GROUP BY milestone.id
            ORDER BY milestone.target_date NULLS LAST, lower(milestone.name)
            LIMIT 200"#
        ))
        .bind(principal.project_id)
        .bind(arguments.include_archived.unwrap_or(false))
        .fetch_all(&state.pool)
        .await?;
        Ok::<_, AppError>(ToolResult::ok(MilestoneListValue {
            milestones: rows
                .into_iter()
                .map(|row| with_url(state, principal.project_id, row))
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
    /// milestone at a time, so this moves them from any other.
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
    let name = match arguments.name.as_deref().map(str::trim) {
        Some(name)
            if name.is_empty()
                || name.chars().count() > 120
                || name.chars().any(char::is_control) =>
        {
            return ToolResult::invalid("name", "Use a name of 1 to 120 characters, on one line.");
        }
        other => other.map(str::to_owned),
    };
    let description = match arguments.description.as_deref() {
        Some(text) if text.chars().count() > 2000 => {
            return ToolResult::invalid(
                "description",
                "Keep the description under 2,000 characters.",
            );
        }
        other => other.map(|text| text.trim().to_owned()),
    };
    let target_date = match arguments.target_date.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(value) => match NaiveDate::parse_from_str(value, "%Y-%m-%d") {
            Ok(date) => Some(Some(date)),
            Err(_) => {
                return ToolResult::invalid("targetDate", "Use YYYY-MM-DD, or \"\" to clear it.");
            }
        },
    };
    let add = arguments.add_tasks.clone().unwrap_or_default();
    let remove = arguments.remove_tasks.clone().unwrap_or_default();
    if add.len() + remove.len() > 200 {
        return ToolResult::invalid("addTasks", "Move at most 200 tasks in one call.");
    }
    let result = async {
        let found = resolve_tasks(state, principal, &add.iter().chain(remove.iter()).cloned().collect::<Vec<_>>()).await?;
        let mut task_ids = Vec::new();
        for (index, item) in found.iter().enumerate() {
            let reference = add.iter().chain(remove.iter()).nth(index).cloned().unwrap_or_default();
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
        if let Some(refusal) = access_refusal(check_write_access(&mut transaction, principal).await?) {
            return Ok(refusal);
        }
        let milestone_id = match arguments.milestone_id.as_deref() {
            Some(reference) => {
                let Some(id) = parse_id(reference) else {
                    return Ok(ToolResult::invalid("milestoneId", "Pass the milestone's ID."));
                };
                let Some(version) = sqlx::query_scalar::<_, i64>(
                    "SELECT version FROM project_milestones WHERE id = $1 AND project_id = $2 FOR UPDATE",
                )
                .bind(id)
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?
                else {
                    return Ok(ToolResult::not_found("That milestone"));
                };
                if let Some(expected) = arguments.expected_version
                    && expected != version
                {
                    return Ok(ToolResult::conflict(format!(
                        "Someone changed this milestone first (now version {version}). Read it again, then retry."
                    )));
                }
                let changed = sqlx::query(
                    r#"
                    UPDATE project_milestones
                    SET name = COALESCE($3, name),
                        description = COALESCE($4, description),
                        target_date = CASE WHEN $5 THEN $6 ELSE target_date END,
                        archived_at = CASE WHEN $7 IS NULL THEN archived_at
                                           WHEN $7 THEN COALESCE(archived_at, now())
                                           ELSE NULL END,
                        version = version + 1, updated_at = now()
                    WHERE id = $1 AND project_id = $2
                    "#,
                )
                .bind(id)
                .bind(project_id)
                .bind(&name)
                .bind(&description)
                .bind(target_date.is_some())
                .bind(target_date.flatten())
                .bind(arguments.archived)
                .execute(&mut *transaction)
                .await;
                if let Err(error) = changed {
                    return Err(name_conflict(error));
                }
                id
            }
            None => {
                let Some(name) = name.clone() else {
                    return Ok(ToolResult::invalid("name", "Give the new milestone a name."));
                };
                let existing = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM project_milestones WHERE project_id = $1 AND archived_at IS NULL AND lower(name) = lower($2)",
                )
                .bind(project_id)
                .bind(&name)
                .fetch_optional(&mut *transaction)
                .await?;
                match existing {
                    Some(id) => id,
                    None => {
                        let id = Uuid::now_v7();
                        sqlx::query(
                            r#"
                            INSERT INTO project_milestones (id, project_id, name, description, target_date, created_by_actor_id)
                            VALUES ($1, $2, $3, $4, $5, $6)
                            "#,
                        )
                        .bind(id)
                        .bind(project_id)
                        .bind(&name)
                        .bind(description.clone().unwrap_or_default())
                        .bind(target_date.flatten())
                        .bind(principal.actor_id)
                        .execute(&mut *transaction)
                        .await?;
                        id
                    }
                }
            }
        };
        if !add_ids.is_empty() {
            sqlx::query("UPDATE work_items SET milestone_id = $3, version = version + 1, updated_at = now() WHERE project_id = $1 AND id = ANY($2) AND milestone_id IS DISTINCT FROM $3")
                .bind(project_id)
                .bind(add_ids)
                .bind(milestone_id)
                .execute(&mut *transaction)
                .await?;
        }
        if !remove_ids.is_empty() {
            sqlx::query("UPDATE work_items SET milestone_id = NULL, version = version + 1, updated_at = now() WHERE project_id = $1 AND id = ANY($2) AND milestone_id = $3")
                .bind(project_id)
                .bind(remove_ids)
                .bind(milestone_id)
                .execute(&mut *transaction)
                .await?;
        }
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "milestone.mcp_written",
            "milestone",
            milestone_id,
            serde_json::json!({ "added": add_ids.len(), "removed": remove_ids.len() }),
        )
        .await?;
        let value = sqlx::query_as::<_, MilestoneValue>(&format!(
            "{MILESTONE_SELECT} WHERE milestone.id = $1 GROUP BY milestone.id"
        ))
        .bind(milestone_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(with_url(state, project_id, value)))
    }
    .await;
    result.unwrap_or_else(|error| match error {
        AppError::Conflict(_, message) => ToolResult::conflict(message),
        error => ToolResult::from_error(error, "milestone_write"),
    })
}

fn name_conflict(error: sqlx::Error) -> AppError {
    if error
        .as_database_error()
        .and_then(|database| database.constraint())
        == Some("project_milestones_active_name_unique")
    {
        AppError::Conflict(
            "milestone",
            "another active milestone already has that name",
        )
    } else {
        error.into()
    }
}

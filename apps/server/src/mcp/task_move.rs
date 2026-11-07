//! Moving tasks between projects (docs/plans/mcp-tools.md, decision 1).
//!
//! A task can't change projects in place: its comments are immutable, and
//! its links and authors belong to its project. So a move carries the task
//! over into the target project (title, description, priority, status and
//! type by name, the assignee if they're there, start and due dates, the
//! estimate, the milestone by name if the target has it, acceptance
//! criteria, subtasks, labels, and every comment quoted with its author and
//! date), then
//! archives the original. The original keeps its history and links, and its
//! key keeps opening the new task (migration 0043).

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id, public_url},
    tasks::command_actor,
};
use crate::{
    AppState,
    error::AppError,
    message_content::MessageContent,
    policy::{Action, Decision},
    work_item_commands::{
        AllocatedNumberMetadata, ChangeFinalization, CommandActor, CreateWorkItemCommand,
        CreateWorkItemOutcome, MAX_SUBTASK_LEVELS, NewTaskFields, authorize_mutation,
        create_work_item, finalize_change, finalize_comment, finalize_copied_comment,
        insert_criteria, lock_membership,
    },
    work_item_labels::{self, LabelLimit, MAX_LABELS_PER_TASK, SkipReason},
};

/// The most tasks (a task and its subtasks) one move carries.
const MAX_TASKS: usize = 100;
/// The most comments carried over per task; the rest stay on the original.
const MAX_COMMENTS: usize = 200;
/// Comments are at most 20,000 characters; leave room for the quote header.
/// Each carried comment gets the clock time, so they keep their order within
/// the one transaction.
const MAX_QUOTED_CHARACTERS: usize = 19_000;

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskMoveArguments {
    /// The task's ID, link, or key such as AW-7. Its subtasks move with it.
    pub task_id: String,
    /// Where it goes: a project ID, link, or key such as "AWD".
    pub to_project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MovedTask {
    /// The old key, which now opens the new task.
    pub from: String,
    /// The new key.
    pub to: String,
    pub id: String,
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskMoveValue {
    /// The task first, then its subtasks.
    pub moved: Vec<MovedTask>,
    /// Anything that didn't carry over exactly, such as an assignee who isn't
    /// in the target project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// False when the task had already been moved; this is where it went.
    pub created: bool,
}

#[derive(Debug, FromRow)]
struct SourceTask {
    id: Uuid,
    parent_id: Option<Uuid>,
    key: String,
    number: i64,
    title: String,
    description: String,
    priority: String,
    type_name: String,
    status_name: String,
    status_category: String,
    assignee_actor_id: Option<Uuid>,
    assignee_name: Option<String>,
    blocked_reason: Option<String>,
    version: i64,
    start_date: Option<chrono::NaiveDate>,
    due_date: Option<chrono::NaiveDate>,
    estimate: Option<f64>,
    milestone_name: Option<String>,
}

#[derive(Debug, FromRow)]
struct SourceComment {
    author: String,
    created_at: String,
    plain_text: String,
}

#[derive(Debug, FromRow)]
struct SourceCriterion {
    text: String,
    completed: bool,
}

#[derive(Debug, FromRow)]
struct LinkLine {
    kind: String,
    label: String,
    /// Whether every member of the source project can see it. Private ones
    /// are only counted, never named, because the list is written into a
    /// comment that everyone in the target project reads.
    shared: bool,
}

pub(super) async fn task_move(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskMoveArguments,
) -> ToolResult<TaskMoveValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(task_id) = parse_id(&arguments.task_id) else {
        return ToolResult::not_found("That task");
    };
    let Some(target_project_id) = parse_id(&arguments.to_project_id) else {
        return ToolResult::not_found("That target project");
    };
    let source_project_id = principal.project_id;
    if super::common::confined_project(principal)
        .is_some_and(|project| project != target_project_id)
    {
        return ToolResult::not_found("That target project");
    }
    if target_project_id == source_project_id {
        return ToolResult::invalid("toProjectId", "The task is already in that project.");
    }
    let result = async {
        let target_reachable = sqlx::query_scalar::<_, bool>(
            "SELECT actor_can_access_project($1, $2, $3) AND kind = 'standard' FROM projects WHERE id = $3",
        )
        .bind(principal.actor_id)
        .bind(principal.external_agent_connection_id)
        .bind(target_project_id)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(false);
        if !target_reachable {
            return Ok(ToolResult::not_found("That target project"));
        }

        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let source_actor = command_actor(principal, source_project_id);
        let target_actor = command_actor(principal, target_project_id);
        // A move archives the source and creates in the target, so it takes
        // both of those permissions as well as changing tasks.
        let mut projects = [
            (
                &source_actor,
                source_project_id,
                "this task's project",
                Action::WorkItemArchive,
            ),
            (
                &target_actor,
                target_project_id,
                "the target project",
                Action::WorkItemCreate,
            ),
        ];
        // Opposite moves must take the same first project lock.
        projects.sort_by_key(|(_, project, _, _)| *project);
        for (actor, project, what, also) in projects {
            let Some(membership) =
                lock_membership(&mut transaction, principal.actor_id, project).await?
            else {
                return Ok(ToolResult::not_found("That project"));
            };
            if membership.project_archived_at.is_some() || !membership.enabled {
                return Ok(ToolResult::not_found("That project"));
            }
            let decision = authorize_mutation(actor, &membership, Action::McpWrite, None)?;
            let also = authorize_mutation(actor, &membership, also, None)?;
            if !matches!(decision, Decision::Allow) || !matches!(also, Decision::Allow) {
                return Ok(ToolResult::denied(format!(
                    "You can't change tasks in {what}."
                )));
            }
            // A move copies the task's description, criteria, comments and
            // notes into the target: only for someone who sees tasks where it
            // is now (their role, and an app's agent limit).
            if project == source_project_id
                && !matches!(
                    authorize_mutation(actor, &membership, Action::WorkItemView, None)?,
                    Decision::Allow
                )
            {
                return Ok(ToolResult::denied(
                    "You can't see tasks in this task's project, so you can't move them.",
                ));
            }
        }
        // AWD-75: the gate checked Tasks in this task's project; the move also
        // creates in the target, so Tasks must be on there too.
        if !super::tasks::tasks_on(&mut *transaction, target_project_id, principal.actor_id).await? {
            return Ok(ToolResult::module_off("task_move", "Tasks"));
        }

        // Moving a task that was already moved returns where it went, so a
        // retry never makes a second copy.
        let archived = sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NOT NULL FROM work_items WHERE project_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(source_project_id)
        .bind(task_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(archived) = archived else {
            return Ok(ToolResult::not_found("That task"));
        };
        if archived {
            let mut moved = already_moved(&mut transaction, state, principal, task_id).await?;
            transaction.commit().await?;
            // As on a fresh move: by key only without "see tasks" here.
            if let Some(moved) = moved.as_mut()
                && crate::search::projects_showing_tasks(
                    &state.pool,
                    principal.actor_id,
                    &[source_project_id],
                )
                .await?
                .is_empty()
            {
                for task in moved.iter_mut() {
                    task.title = String::new();
                }
            }
            return Ok(match moved {
                Some(moved) => ToolResult::ok(TaskMoveValue {
                    moved,
                    notes: Vec::new(),
                    created: false,
                }),
                None => ToolResult::conflict(
                    "That task is archived. Restore it before moving it.",
                ),
            });
        }

        let sees_code = !crate::search::projects_allowing(
            &state.pool,
            principal.actor_id,
            &[source_project_id],
            crate::policy::Permission::CodeView,
        )
        .await?
        .is_empty();
        let tasks = sqlx::query_as::<_, SourceTask>(
            r#"
            WITH RECURSIVE tree AS (
                SELECT id, 0 AS depth FROM work_items
                WHERE project_id = $1 AND id = $2
                UNION ALL
                SELECT child.id, tree.depth + 1
                FROM work_items child
                JOIN tree ON child.parent_work_item_id = tree.id
                WHERE child.project_id = $1 AND child.archived_at IS NULL AND tree.depth < $3
            )
            SELECT item.id,
                   CASE WHEN item.id = $2 THEN NULL ELSE item.parent_work_item_id END AS parent_id,
                   project.project_key || '-' || item.number AS key,
                   item.number, item.title, item.description, item.priority,
                   item_type.name AS type_name, status.name AS status_name,
                   status.semantic_category AS status_category,
                   item.assignee_actor_id, assignee.display_name AS assignee_name,
                   item.blocked_reason, item.version, item.start_date, item.due_date,
                   item.estimate, milestone.name AS milestone_name
            FROM tree
            JOIN work_items item ON item.id = tree.id
            LEFT JOIN project_milestones milestone
              ON milestone.project_id = item.project_id AND milestone.id = item.milestone_id
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_types item_type
              ON item_type.project_id = item.project_id AND item_type.id = item.type_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            LEFT JOIN project_memberships assignee
              ON assignee.project_id = item.project_id
             AND assignee.actor_id = item.assignee_actor_id
            ORDER BY tree.depth, item.number
            FOR UPDATE OF item
            "#,
        )
        .bind(source_project_id)
        .bind(task_id)
        // Depth 0 is the task itself, so this walks every level a tree can have.
        .bind(i32::try_from(MAX_SUBTASK_LEVELS - 1).unwrap_or(i32::MAX))
        .fetch_all(&mut *transaction)
        .await?;
        if tasks.len() > MAX_TASKS {
            return Ok(ToolResult::invalid(
                "taskId",
                format!(
                    "That task has {} subtasks; one move carries at most {MAX_TASKS} tasks. Move some subtasks first.",
                    tasks.len() - 1
                ),
            ));
        }
        let (target_key, organization_id) = sqlx::query_as::<_, (String, Uuid)>(
            "SELECT project_key, organization_id FROM projects WHERE id = $1",
        )
        .bind(target_project_id)
        .fetch_one(&mut *transaction)
        .await?;

        // An estimate means points or hours by its project's setting; between
        // projects that differ it isn't carried over.
        let (_, source_unit) =
            crate::work_item_workflow::task_settings(&mut transaction, source_project_id).await?;
        let (_, target_unit) =
            crate::work_item_workflow::task_settings(&mut transaction, target_project_id).await?;
        let same_unit = source_unit == target_unit;

        let mut notes = Vec::new();
        let mut moved = Vec::new();
        let mut new_ids = std::collections::HashMap::new();
        for task in &tasks {
            let status_id = matching_status(&mut transaction, target_project_id, task).await?;
            if status_id.is_none() {
                notes.push(format!(
                    "{target_key} has no status named {} and none in its group ({}), so {} starts in {target_key}'s default status.",
                    task.status_name, task.status_category, task.key
                ));
            }
            let type_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM work_item_types WHERE project_id = $1 AND enabled AND lower(name) = lower($2)",
            )
            .bind(target_project_id)
            .bind(&task.type_name)
            .fetch_optional(&mut *transaction)
            .await?;
            let assignee = match task.assignee_actor_id {
                Some(assignee) if is_member(&mut transaction, target_project_id, assignee).await? => {
                    Some(assignee)
                }
                Some(_) => {
                    notes.push(format!(
                        "{} isn't in {target_key}, so the task moved from {} is unassigned.",
                        task.assignee_name.as_deref().unwrap_or("Its assignee"),
                        task.key
                    ));
                    None
                }
                None => None,
            };
            let parent = task.parent_id.and_then(|parent| new_ids.get(&parent).copied());
            // Labels go by name: one the target already has keeps its color
            // there, and a missing one is created with the source's color.
            let mut wanted = work_item_labels::active_labels_with_colors(
                &mut *transaction,
                source_project_id,
                task.id,
            )
            .await?;
            let over_limit = wanted.split_off(wanted.len().min(MAX_LABELS_PER_TASK));
            let labels = work_item_labels::ensure_by_name(
                &mut transaction,
                target_project_id,
                principal.actor_id,
                &wanted,
            )
            .await?;
            for label in &labels.created {
                insert_audit(
                    &mut transaction,
                    principal,
                    target_project_id,
                    "work_item_label.mcp_created",
                    "work_item_label",
                    label.id,
                    serde_json::json!({
                        "changedFields": ["label"],
                        "name": label.name,
                        "movedWith": task.key,
                    }),
                )
                .await?;
            }
            // A milestone goes by name: the target's active one with the same
            // name, if it has one.
            let milestone_id = match &task.milestone_name {
                Some(name) => {
                    let found = sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM project_milestones WHERE project_id = $1 AND archived_at IS NULL AND lower(name) = lower($2)",
                    )
                    .bind(target_project_id)
                    .bind(name)
                    .fetch_optional(&mut *transaction)
                    .await?;
                    if found.is_none() {
                        notes.push(format!(
                            "{target_key} has no milestone called \"{name}\", so the task moved from {} isn't in one.",
                            task.key
                        ));
                    }
                    found
                }
                None => None,
            };
            if let (Some(estimate), false) = (task.estimate, same_unit) {
                notes.push(format!(
                    "{target_key} estimates in {target_unit}, not {source_unit}, so the estimate of {estimate} {source_unit} on {} wasn't carried over.",
                    task.key
                ));
            }
            let summary = format!("Work Item moved from {}", task.key);
            let outcome = create_work_item(
                &mut transaction,
                &target_actor,
                CreateWorkItemCommand {
                    title: &task.title,
                    description: &task.description,
                    type_id,
                    status_id,
                    priority: &task.priority,
                    assignee_actor_id: assignee,
                    parent_work_item_id: parent,
                    label_ids: &labels.ids,
                    acceptance_criteria: &[],
                    board_placement: None,
                    fields: NewTaskFields {
                        milestone_id,
                        start_date: task.start_date,
                        due_date: task.due_date,
                        estimate: if same_unit { task.estimate } else { None },
                    },
                    activity_summary: &summary,
                    activity_metadata: serde_json::json!({ "movedFrom": task.key }),
                    activity_number_metadata: AllocatedNumberMetadata::Include,
                    audit_action: "work_item.mcp_moved_in",
                    audit_metadata: serde_json::json!({
                        "moved_from": task.key,
                        "source_work_item_id": task.id,
                    }),
                },
            )
            .await?;
            let CreateWorkItemOutcome::Created {
                work_item_id: new_id,
                number,
                ..
            } = outcome
            else {
                return Err(AppError::Internal("moved task could not be created"));
            };
            let new_key = format!("{target_key}-{number}");
            new_ids.insert(task.id, new_id);
            for (name, reason) in &labels.skipped {
                notes.push(match reason {
                    SkipReason::Archived => format!(
                        "\"{name}\" is archived in {target_key}, so it wasn't put on {new_key}."
                    ),
                    SkipReason::ProjectFull(LabelLimit::Active) => format!(
                        "{target_key} already has {} labels, so \"{name}\" wasn't carried to {new_key}.",
                        work_item_labels::MAX_ACTIVE_LABELS_PER_PROJECT
                    ),
                    SkipReason::ProjectFull(LabelLimit::Total) => format!(
                        "{target_key} has no room for more labels, so \"{name}\" wasn't carried to {new_key}."
                    ),
                });
            }
            for (name, _) in &over_limit {
                notes.push(format!(
                    "{} had more than {MAX_LABELS_PER_TASK} labels, so \"{name}\" wasn't carried to {new_key}.",
                    task.key
                ));
            }

            carry_criteria(&mut transaction, principal, source_project_id, target_project_id, task.id, new_id)
                .await?;
            let links = links_left_behind(&mut transaction, source_project_id, task.id, principal.actor_id, sees_code)
                .await?;
            let mut provenance = format!(
                "Moved from {} by {}. The original is archived with its full history.",
                task.key,
                mover_name(&mut transaction, source_project_id, principal.actor_id).await?
            );
            if let Some(reason) = &task.blocked_reason {
                provenance.push_str(&format!("\n\nIt was blocked: {reason}"));
            }
            if !links.is_empty() {
                provenance.push_str(&format!("\n\nStill linked on {}:", task.key));
                provenance.push_str(&links_left_behind_text(&links));
            }
            add_comment(&mut transaction, &target_actor, new_id, &provenance, Notify::Yes).await?;
            let comments = sqlx::query_as::<_, SourceComment>(
                r#"
                SELECT COALESCE(author.display_name, 'Someone') AS author,
                       to_char(comment.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI "UTC"') AS created_at,
                       comment.plain_text
                FROM work_item_comments comment
                LEFT JOIN project_memberships author
                  ON author.project_id = comment.project_id
                 AND author.actor_id = comment.author_actor_id
                WHERE comment.project_id = $1 AND comment.work_item_id = $2
                  AND comment.removed_at IS NULL
                ORDER BY comment.created_at, comment.id
                LIMIT $3
                "#,
            )
            .bind(source_project_id)
            .bind(task.id)
            .bind(MAX_COMMENTS as i64)
            .fetch_all(&mut *transaction)
            .await?;
            let comment_count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM work_item_comments WHERE project_id = $1 AND work_item_id = $2 AND removed_at IS NULL",
            )
            .bind(source_project_id)
            .bind(task.id)
            .fetch_one(&mut *transaction)
            .await?;
            for comment in &comments {
                let text = comment.plain_text.chars().take(MAX_QUOTED_CHARACTERS).collect::<String>();
                let quoted = format!(
                    "{} wrote on {} (on {}):\n\n{}",
                    comment.author, comment.created_at, task.key, text
                );
                add_comment(&mut transaction, &target_actor, new_id, &quoted, Notify::No).await?;
            }
            if comment_count > MAX_COMMENTS as i64 {
                notes.push(format!(
                    "{} has {} comments; the first {MAX_COMMENTS} were carried over and the rest are on the original.",
                    task.key, comment_count
                ));
            }

            // Handoff notes go with the task, so whoever claims it in its new
            // project still reads where the last holder left it.
            sqlx::query(
                r#"
                INSERT INTO work_item_handoffs (
                    id, project_id, work_item_id, actor_id, via_oauth_grant_id, note, created_at
                )
                SELECT gen_random_uuid(), $3, $4, handoff.actor_id, handoff.via_oauth_grant_id,
                       handoff.note, handoff.created_at
                FROM work_item_handoffs handoff
                WHERE handoff.project_id = $1 AND handoff.work_item_id = $2
                "#,
            )
            .bind(source_project_id)
            .bind(task.id)
            .bind(target_project_id)
            .bind(new_id)
            .execute(&mut *transaction)
            .await?;

            // Point the original at its new home, then archive it.
            add_comment(
                &mut transaction,
                &source_actor,
                task.id,
                &format!(
                    "Moved to {new_key}: {}",
                    public_url(state, &format!("/projects/{target_project_id}/work-items/{new_id}"))
                ),
                Notify::Yes,
            )
            .await?;
            archive(&mut transaction, &source_actor, task, &new_key).await?;
            sqlx::query(
                r#"
                INSERT INTO work_item_key_aliases (
                    id, organization_id, project_key, number, source_work_item_id,
                    work_item_id, moved_by_actor_id
                )
                SELECT $1, $2, project.project_key, $4, $5, $6, $7
                FROM projects project WHERE project.id = $3
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(organization_id)
            .bind(source_project_id)
            .bind(task.number)
            .bind(task.id)
            .bind(new_id)
            .bind(principal.actor_id)
            .execute(&mut *transaction)
            .await?;
            // Anything that pointed at the old key follows it, including keys
            // the original had itself inherited from an earlier move.
            sqlx::query("UPDATE work_item_key_aliases SET work_item_id = $2 WHERE work_item_id = $1")
                .bind(task.id)
                .bind(new_id)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM work_item_claims WHERE work_item_id = $1")
                .bind(task.id)
                .execute(&mut *transaction)
                .await?;
            moved.push(MovedTask {
                from: task.key.clone(),
                to: new_key,
                id: new_id.to_string(),
                title: task.title.clone(),
                url: public_url(state, &format!("/projects/{target_project_id}/work-items/{new_id}")),
            });
        }
        transaction.commit().await?;
        // Moving takes archive and create, not "see tasks": without it, the
        // result names the tasks by key only.
        if crate::search::projects_showing_tasks(&state.pool, principal.actor_id, &[source_project_id])
            .await?
            .is_empty()
        {
            for task in &mut moved {
                task.title = String::new();
            }
        }
        Ok::<_, AppError>(ToolResult::ok(TaskMoveValue {
            moved,
            notes,
            created: true,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_move"))
}

/// The status with the same name in the target project, or else its first
/// status in the same category, or else its default.
async fn matching_status(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task: &SourceTask,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM work_item_statuses
        WHERE project_id = $1 AND enabled
          AND (lower(name) = lower($2) OR semantic_category = $3)
        ORDER BY lower(name) = lower($2) DESC, position
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(&task.status_name)
    .bind(&task.status_category)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn is_member(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.project_id = $1 AND membership.actor_id = $2
              AND membership.enabled AND actor.status = 'active'
              AND actor.kind IN ('human', 'external_agent')
        )
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn mover_name(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<String, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or_else(|| "someone".to_owned()))
}

/// Copy acceptance criteria in order. Whoever moves the task is recorded as
/// creating them, and as completing the ones that were done, since the
/// original authors may not be in the target project.
async fn carry_criteria(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    source_project_id: Uuid,
    target_project_id: Uuid,
    source_id: Uuid,
    target_id: Uuid,
) -> Result<(), AppError> {
    let criteria = sqlx::query_as::<_, SourceCriterion>(
        r#"
        SELECT text, completed FROM work_item_acceptance_criteria
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY position
        "#,
    )
    .bind(source_project_id)
    .bind(source_id)
    .fetch_all(&mut **transaction)
    .await?;
    let criteria = criteria
        .into_iter()
        .map(|criterion| (criterion.text, criterion.completed))
        .collect::<Vec<_>>();
    insert_criteria(
        transaction,
        target_project_id,
        target_id,
        principal.actor_id,
        1,
        &criteria,
    )
    .await
}

/// The lines listing what stays linked on the original. Shared links are
/// named; private conversations and pull requests from private threads are
/// only counted, since readers of the new task may not be able to see them.
fn links_left_behind_text(links: &[LinkLine]) -> String {
    let mut text = String::new();
    for link in links.iter().filter(|link| link.shared) {
        text.push_str(&format!("\n- {}: {}", link.kind, link.label));
    }
    let private_conversations = links
        .iter()
        .filter(|link| !link.shared && link.kind == "Conversation")
        .count();
    let private_pull_requests = links
        .iter()
        .filter(|link| !link.shared && link.kind == "Pull request")
        .count();
    for (count, singular, plural) in [
        (
            private_conversations,
            "private conversation",
            "private conversations",
        ),
        (
            private_pull_requests,
            "private pull request",
            "private pull requests",
        ),
    ] {
        match count {
            0 => {}
            1 => text.push_str(&format!("\n- and 1 {singular}")),
            n => text.push_str(&format!("\n- and {n} {plural}")),
        }
    }
    text
}

/// Threads, repositories and pull requests linked to the original. They stay
/// in its project, so the new task lists them. The list is a comment every
/// target-project member reads, so only links every source-project member
/// can see are marked shared (and named); the rest are counted, and only if
/// the mover can see them.
async fn links_left_behind(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    viewer: Uuid,
    // Repositories and pull requests are named only for a mover who sees
    // code in the source project (role, and an app's agent limit).
    sees_code: bool,
) -> Result<Vec<LinkLine>, AppError> {
    Ok(sqlx::query_as::<_, LinkLine>(
        r#"
        SELECT 'Conversation' AS kind, conversation.name AS label,
               COALESCE(parent.visibility, conversation.visibility) = 'project' AS shared
        FROM work_item_conversation_links link
        JOIN conversations conversation ON conversation.id = link.conversation_id
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE link.project_id = $1 AND link.work_item_id = $2
          AND conversation_visible_to(conversation.id, $3)
        UNION ALL
        SELECT 'Repository', repository.owner || '/' || repository.name, TRUE
        FROM work_item_repository_links link
        JOIN repositories repository ON repository.id = link.repository_id
        WHERE link.project_id = $1 AND link.work_item_id = $2 AND $4
        UNION ALL
        -- Same rule as pull_request_visible_to, for a reader who belongs to
        -- no private conversation: no run_artifact link, or one whose
        -- thread is project-visible.
        SELECT 'Pull request', pull_request.html_url,
               NOT EXISTS (
                   SELECT 1 FROM github_pull_request_conversation_links artifact
                   WHERE artifact.github_pull_request_id = pull_request.id
                     AND artifact.source = 'run_artifact')
               OR EXISTS (
                   SELECT 1 FROM github_pull_request_conversation_links artifact
                   JOIN conversations thread ON thread.id = artifact.conversation_id
                   LEFT JOIN conversations parent ON parent.id = thread.parent_channel_id
                   WHERE artifact.github_pull_request_id = pull_request.id
                     AND artifact.source = 'run_artifact'
                     AND COALESCE(parent.visibility, thread.visibility) = 'project')
        FROM github_pull_request_work_item_links link
        JOIN github_pull_requests pull_request ON pull_request.id = link.github_pull_request_id
        WHERE link.project_id = $1 AND link.work_item_id = $2
          AND pull_request_visible_to(pull_request.id, $3) AND $4
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(viewer)
    .bind(sees_code)
    .fetch_all(&mut **transaction)
    .await?)
}

/// Whether adding a comment tells watchers and the assignee. Copied history
/// doesn't: the move's own comment already did, once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Notify {
    Yes,
    No,
}

async fn add_comment(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    text: &str,
    notify: Notify,
) -> Result<(), AppError> {
    let validated = MessageContent::from_plain_text(text.to_owned())
        .map_err(|_| AppError::Validation("comment"))?;
    let content = serde_json::to_value(validated.content())
        .map_err(|_| AppError::Internal("serialize moved comment"))?;
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM work_items WHERE project_id = $1 AND id = $2",
    )
    .bind(actor.project_id)
    .bind(work_item_id)
    .fetch_one(&mut **transaction)
    .await?;
    let comment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO work_item_comments (
            id, project_id, work_item_id, author_actor_id, content_version, content, plain_text,
            created_at
        ) VALUES ($1, $2, $3, $4, 1, $5, $6, clock_timestamp())
        "#,
    )
    .bind(comment_id)
    .bind(actor.project_id)
    .bind(work_item_id)
    .bind(actor.actor_id)
    .bind(content)
    .bind(text)
    .execute(&mut **transaction)
    .await?;
    if notify == Notify::No {
        return finalize_copied_comment(
            transaction,
            actor,
            work_item_id,
            comment_id,
            "work_item.mcp_move_comment",
        )
        .await;
    }
    finalize_comment(
        transaction,
        actor,
        work_item_id,
        comment_id,
        version,
        "work_item.mcp_move_comment",
    )
    .await?;
    Ok(())
}

async fn archive(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task: &SourceTask,
    new_key: &str,
) -> Result<(), AppError> {
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE work_items
        SET archived_at = now(), archived_by_actor_id = $3,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND archived_at IS NULL
        RETURNING version
        "#,
    )
    .bind(actor.project_id)
    .bind(task.id)
    .bind(actor.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    let summary = format!("Work Item moved to {new_key}");
    finalize_change(
        transaction,
        actor,
        task.id,
        ChangeFinalization {
            activity_kind: "work_item_archived",
            summary: &summary,
            audit_action: "work_item.mcp_moved_out",
            activity_payload: serde_json::json!({ "movedTo": new_key, "previousVersion": task.version }),
            audit_metadata: serde_json::json!({ "moved_to": new_key }),
            version,
        },
    )
    .await?;
    Ok(())
}

/// Where an archived task went, with any subtasks that moved with it.
async fn already_moved(
    transaction: &mut Transaction<'_, Postgres>,
    state: &AppState,
    principal: &McpPrincipal,
    source_id: Uuid,
) -> Result<Option<Vec<MovedTask>>, AppError> {
    let rows = sqlx::query_as::<_, (String, String, Uuid, Uuid, String)>(
        // The moved subtasks have aliases of their own, keyed by their own
        // source IDs, so the whole source tree is walked.
        r#"
        WITH RECURSIVE source_tree(id, depth) AS (
            SELECT $1::uuid, 0
            UNION ALL
            SELECT child.id, source_tree.depth + 1
            FROM work_items child
            JOIN source_tree ON child.parent_work_item_id = source_tree.id
            WHERE source_tree.depth < 50
        )
        SELECT alias.project_key || '-' || alias.number,
               project.project_key || '-' || item.number,
               item.id, item.project_id, item.title
        FROM source_tree
        JOIN work_item_key_aliases alias ON alias.source_work_item_id = source_tree.id
        JOIN work_items item ON item.id = alias.work_item_id
        JOIN projects project ON project.id = item.project_id
        WHERE actor_can_access_project($2, $3, item.project_id)
          AND ($4::uuid IS NULL OR item.project_id = $4)
        ORDER BY source_tree.depth, alias.number
        "#,
    )
    .bind(source_id)
    .bind(principal.actor_id)
    .bind(principal.external_agent_connection_id)
    .bind(super::common::confined_project(principal))
    .fetch_all(&mut **transaction)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    // Each title is the task's current one, where it is now; it is shown
    // only with "see tasks" in that project, as task_open would.
    let mut projects = rows.iter().map(|row| row.3).collect::<Vec<_>>();
    projects.sort_unstable();
    projects.dedup();
    let showing =
        crate::search::projects_showing_tasks(&state.pool, principal.actor_id, &projects).await?;
    Ok(Some(
        rows.into_iter()
            .map(|(from, to, id, project_id, title)| MovedTask {
                from,
                to,
                id: id.to_string(),
                title: if showing.contains(&project_id) {
                    title
                } else {
                    String::new()
                },
                url: public_url(state, &format!("/projects/{project_id}/work-items/{id}")),
            })
            .collect(),
    ))
}

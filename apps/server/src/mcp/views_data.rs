//! Views and data for agents (docs/plans/roadmap.md, W11): saved views,
//! My work, task templates, imports and exports. Each tool checks the
//! caller's current access to the project every time, like the rest.

use std::collections::BTreeMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id, public_url, scope_projects},
    planning::{DATE_MESSAGE, milestone_change},
    tasks::{TaskListArguments, command_actor, resolve_assignee},
    writes::{WriteAccess, check_write_access},
};
use crate::{
    AppState,
    error::AppError,
    policy::Action,
    work_items::{
        imports::{self, CommitRefusal, CreateImportRequest, ImportOptions},
        templates::{self, ApplyOptions, ApplyOutcome},
        views,
    },
};

fn access_refusal<T>(access: WriteAccess) -> Option<ToolResult<T>> {
    match access {
        WriteAccess::Allowed => None,
        WriteAccess::ReadOnly => Some(ToolResult::read_only()),
        WriteAccess::NotFound => Some(ToolResult::not_found("That project")),
        WriteAccess::ProjectArchived => Some(ToolResult::conflict("That project is archived.")),
        WriteAccess::Denied => Some(ToolResult::denied(
            "You can't create tasks in that project.",
        )),
    }
}

async fn can_view(state: &AppState, principal: &McpPrincipal) -> Result<(), AppError> {
    crate::memory::project_access(
        &state.pool,
        principal.actor_id,
        principal.actor_kind,
        principal.project_id,
        Action::WorkItemView,
    )
    .await?;
    Ok(())
}

fn to_value(value: impl Serialize) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| AppError::Internal("serialize tool value"))
}

// ---------------------------------------------------------------------------
// Saved views

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ViewListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ViewListValue {
    /// Each view: id, name, builtin (my_tasks, ready_for_agents, blocked,
    /// recently_changed), shared, pinned, its query as the list page keeps
    /// it, resolvedQuery ("me" replaced by you) and a link.
    pub views: Vec<Value>,
    pub how_to_use: String,
}

pub(super) async fn view_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ViewListArguments,
) -> ToolResult<ViewListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        can_view(state, principal).await?;
        views::ensure_builtin_views(&state.pool, principal.project_id).await?;
        let rows =
            views::visible_views(&state.pool, principal.project_id, principal.actor_id).await?;
        let views = rows
            .into_iter()
            .map(|row| {
                let mut view = views::saved_view(row, principal.actor_id, false);
                view.href = public_url(state, &view.href);
                to_value(view).map(|mut value| {
                    if let Some(object) = value.as_object_mut() {
                        for key in ["canEdit", "canDelete", "position", "version"] {
                            object.remove(key);
                        }
                    }
                    value
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, AppError>(ToolResult::ok(ViewListValue {
            views,
            how_to_use: "Pass a view's name or id as `view` to task_list to list its tasks; filters you pass as well win over the view's.".to_owned(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "view_list"))
}

/// Fills task_list's arguments from a saved view. What the caller passed
/// wins over the view; the view's "me" is the caller.
pub(super) async fn apply_view(
    state: &AppState,
    principal: &McpPrincipal,
    mut arguments: TaskListArguments,
) -> Result<Result<TaskListArguments, (&'static str, String)>, AppError> {
    let Some(wanted) = arguments.view.clone() else {
        return Ok(Ok(arguments));
    };
    if !principal.project_explicit && arguments.project_id.is_none() {
        return Ok(Err((
            "view",
            "Views belong to a project: pass projectId with view.".to_owned(),
        )));
    }
    can_view(state, principal).await?;
    views::ensure_builtin_views(&state.pool, principal.project_id).await?;
    let rows = views::visible_views(&state.pool, principal.project_id, principal.actor_id).await?;
    let wanted_id = parse_id(&wanted);
    let Some(row) = rows.into_iter().find(|row| {
        Some(row.id) == wanted_id
            || row.name.eq_ignore_ascii_case(wanted.trim())
            || row.builtin.as_deref() == Some(wanted.trim())
    }) else {
        return Ok(Err((
            "view",
            format!("No view called “{wanted}” in this project. See view_list."),
        )));
    };
    let query = views::query_params(&row.query);
    let flag = |key: &str| query.get(key).and_then(|value| value.parse::<bool>().ok());
    if arguments.query.is_none() {
        arguments.query = query.get("q").cloned();
    }
    if arguments.assignee.is_none() {
        arguments.assignee = match (query.get("assigneeActorId"), flag("unassigned")) {
            (Some(id), _) => Some(id.clone()),
            (None, Some(true)) => Some("none".to_owned()),
            _ => None,
        };
    }
    if arguments.labels.is_none()
        && let Some(ids) = query.get("labelIds").filter(|ids| *ids != "none")
    {
        arguments.labels = Some(ids.split(',').map(str::to_owned).collect());
    }
    if arguments.priority.is_none() {
        arguments.priority = query.get("priority").cloned();
    }
    if arguments.task_type.is_none() {
        arguments.task_type = query.get("typeId").cloned();
    }
    if arguments.status.is_none() {
        arguments.status = match (query.get("statusId"), query.get("semanticCategory")) {
            (Some(id), _) => sqlx::query_scalar::<_, String>(
                "SELECT name FROM work_item_statuses WHERE project_id = $1 AND id = $2::uuid",
            )
            .bind(principal.project_id)
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten(),
            (None, Some(category)) => Some(category.clone()),
            _ => None,
        };
    }
    if arguments.milestone.is_none() {
        arguments.milestone = query.get("milestoneId").cloned();
    }
    if arguments.parent_id.is_none() {
        arguments.parent_id = query.get("parentId").cloned();
    }
    if arguments.due_before.is_none() {
        arguments.due_before = query.get("dueBefore").cloned();
    }
    arguments.blocked = arguments.blocked.or(flag("blocked"));
    arguments.ready = arguments.ready.or(flag("ready"));
    arguments.overdue = arguments.overdue.or(flag("overdue"));
    if arguments.claimed.is_none() {
        arguments.claimed =
            flag("claimed").map(|claimed| if claimed { "yes" } else { "no" }.to_owned());
    }
    // The list page shows finished tasks unless a view says not to.
    if arguments.include_done.is_none() {
        arguments.include_done = Some(flag("includeDone").unwrap_or(true));
    }
    if arguments.sort.is_none() {
        arguments.sort = query
            .get("sort")
            .filter(|sort| matches!(sort.as_str(), "priority" | "updated" | "number"))
            .cloned();
    }
    Ok(Ok(arguments))
}

// ---------------------------------------------------------------------------
// My work

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MyWorkArguments {
    /// Only this project (ID, link or key). Leave it out for all of yours.
    /// The MCP middleware scopes the call to it.
    #[allow(dead_code)]
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MyWorkValue {
    /// Tasks assigned to you that aren't finished, due first.
    pub assigned: Value,
    /// Tasks you, or agents you set up, are working on now.
    pub claimed: Value,
    /// Tasks in review that you made or watch, assigned to someone else.
    pub review: Value,
}

pub(super) async fn my_work(
    state: &AppState,
    principal: &McpPrincipal,
    _arguments: MyWorkArguments,
) -> ToolResult<MyWorkValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let projects = scope_projects(state, principal).await?;
        let work = views::my_work(&state.pool, principal.actor_id, &projects).await?;
        let value = to_value(work)?;
        Ok::<_, AppError>(ToolResult::ok(MyWorkValue {
            assigned: value["assigned"].clone(),
            claimed: value["claimed"].clone(),
            review: value["review"].clone(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "my_work"))
}

// ---------------------------------------------------------------------------
// Templates

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TemplateListArguments {
    /// The project's ID, link or key.
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TemplateListValue {
    /// Each template: id, name, title, description, type, priority, labels,
    /// acceptanceCriteria, subtasks, estimate.
    pub templates: Vec<Value>,
    /// Schedules that make a task from a template: frequency, next run,
    /// whether it's paused and why.
    pub recurring: Vec<Value>,
}

pub(super) async fn template_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TemplateListArguments,
) -> ToolResult<TemplateListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        can_view(state, principal).await?;
        let mut connection = state.pool.acquire().await?;
        let rows = templates::list_templates(&mut connection, principal.project_id, false).await?;
        let recurring = templates::list_recurring(&mut connection, principal.project_id).await?;
        Ok::<_, AppError>(ToolResult::ok(TemplateListValue {
            templates: rows
                .into_iter()
                .map(|row| to_value(templates::TaskTemplateView::from(row)))
                .collect::<Result<_, _>>()?,
            recurring: recurring
                .into_iter()
                .map(to_value)
                .collect::<Result<_, _>>()?,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "template_list"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskFromTemplateArguments {
    /// The project's ID, link or key.
    pub project_id: String,
    /// The template's name or ID (see template_list).
    pub template: String,
    /// A title instead of the template's.
    pub title: Option<String>,
    /// "me", an @handle or an actor ID.
    pub assignee: Option<String>,
    /// A milestone by name or ID.
    pub milestone: Option<String>,
    /// YYYY-MM-DD.
    pub start_date: Option<String>,
    /// YYYY-MM-DD.
    pub due_date: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskFromTemplateValue {
    pub id: String,
    pub key: String,
    pub subtasks: usize,
    /// Labels the template named that the project had to make.
    pub created_labels: Vec<String>,
    pub url: String,
}

pub(super) async fn task_from_template(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskFromTemplateArguments,
) -> ToolResult<TaskFromTemplateValue> {
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let date = |value: &Option<String>| match value.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(value) => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| ()),
    };
    let Ok(start_date) = date(&arguments.start_date) else {
        return ToolResult::invalid("startDate", DATE_MESSAGE);
    };
    let Ok(due_date) = date(&arguments.due_date) else {
        return ToolResult::invalid("dueDate", DATE_MESSAGE);
    };
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) =
            access_refusal(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(refusal);
        }
        let template_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id FROM task_templates
            WHERE project_id = $1 AND archived_at IS NULL
              AND (id = $2 OR lower(name) = lower($3))
            ORDER BY id = $2 DESC
            LIMIT 1
            "#,
        )
        .bind(project_id)
        .bind(parse_id(&arguments.template))
        .bind(arguments.template.trim())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(template_id) = template_id else {
            return Ok(ToolResult::not_found("That template (see template_list)"));
        };
        let assignee_actor_id = match arguments.assignee.as_deref() {
            None => None,
            Some(value) => {
                match resolve_assignee(&mut transaction, principal, project_id, value).await? {
                    Ok(actor) => actor,
                    Err(problem) => return Ok(ToolResult::invalid("assignee", problem.message())),
                }
            }
        };
        let milestone_id = match arguments.milestone.as_deref() {
            None => None,
            Some(value) => match milestone_change(&mut transaction, project_id, value).await? {
                Some(milestone) => milestone,
                None => {
                    return Ok(ToolResult::invalid(
                        "milestone",
                        "No active milestone by that name or ID. See milestone_list.",
                    ));
                }
            },
        };
        let actor = command_actor(principal, project_id);
        let outcome = templates::apply_template(
            &mut transaction,
            &actor,
            template_id,
            &ApplyOptions {
                title: arguments.title.clone(),
                assignee_actor_id,
                milestone_id,
                start_date,
                due_date,
            },
        )
        .await?;
        let applied = match outcome {
            ApplyOutcome::Created(applied) => applied,
            ApplyOutcome::Refused(field, message) => {
                return Ok(ToolResult::invalid(field, message));
            }
        };
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(TaskFromTemplateValue {
            id: applied.work_item_id.to_string(),
            url: public_url(
                state,
                &format!("/projects/{project_id}/work-items/{}", applied.work_item_id),
            ),
            key: applied.key,
            subtasks: applied.subtask_count,
            created_labels: applied.created_labels,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_from_template"))
}

// ---------------------------------------------------------------------------
// Imports and exports

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskImportArguments {
    /// The project's ID, link or key.
    pub project_id: String,
    /// An import you started earlier, to read again or commit. Leave it out
    /// to start one.
    pub import_id: Option<String>,
    /// markdown (a list of "- [ ] task" items; indented items are
    /// subtasks), csv (Linear and Jira exports map as they are), or github
    /// (a connected repository's issues, read in the background).
    pub source: Option<String>,
    /// The Markdown or CSV text, up to 4 MB.
    pub text: Option<String>,
    /// For github: the repository's ID (see repository_list).
    pub repository_id: Option<String>,
    /// For github: "open" (the default) or "all".
    pub github_state: Option<String>,
    /// Markdown headings become milestones.
    pub headings_as_milestones: Option<bool>,
    /// For csv: which column fills each task field, such as
    /// {"title": "Summary", "externalId": "Issue key"}. Columns are matched
    /// by name automatically when left out.
    pub mapping: Option<BTreeMap<String, String>>,
    /// false (the default): only preview what would be created. true:
    /// create the tasks. Rows already imported from the same source are
    /// skipped either way.
    pub commit: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskImportValue {
    /// The import: id, status (fetching, draft, queued, running, completed,
    /// failed, canceled), counts, and while it's a draft a preview of what
    /// each row becomes.
    pub import: Value,
    pub next_step: String,
}

pub(super) async fn task_import(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskImportArguments,
) -> ToolResult<TaskImportValue> {
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let project_id = principal.project_id;
    let request = match (&arguments.import_id, &arguments.source) {
        (Some(_), _) => None,
        (None, Some(source)) => Some(CreateImportRequest {
            source: source.clone(),
            text: arguments.text.clone(),
            file_name: None,
            repository_id: match arguments.repository_id.as_deref().map(parse_id) {
                None => None,
                Some(Some(id)) => Some(id),
                Some(None) => {
                    return ToolResult::invalid("repositoryId", "Pass the repository's ID.");
                }
            },
            options: Some(ImportOptions {
                headings_as_milestones: arguments.headings_as_milestones.unwrap_or(false),
                create_labels: true,
                create_milestones: true,
                github_state: arguments.github_state.clone(),
                excluded_rows: Vec::new(),
            }),
            mapping: arguments.mapping.clone(),
        }),
        (None, None) => {
            return ToolResult::invalid(
                "source",
                "Pass source (markdown, csv or github), or importId.",
            );
        }
    };
    let prepared = match request.as_ref().map(imports::prepare) {
        None => None,
        Some(Ok(prepared)) => Some(prepared),
        Some(Err((field, message))) => return ToolResult::invalid(field, message),
    };
    let result = async {
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) = access_refusal(check_write_access(&mut transaction, principal).await?) {
            return Ok(refusal);
        }
        let actor = command_actor(principal, project_id);
        let import_id = match (request, prepared, &arguments.import_id) {
            (Some(request), Some(prepared), _) => {
                match imports::insert_import(&mut transaction, &actor, &request, prepared).await? {
                    Ok(id) => id,
                    Err((field, message)) => return Ok(ToolResult::invalid(field, message)),
                }
            }
            (_, _, Some(reference)) => match parse_id(reference) {
                Some(id) => id,
                None => return Ok(ToolResult::invalid("importId", "Pass the import's ID.")),
            },
            _ => return Err(AppError::Internal("import arguments")),
        };
        let mut committed = false;
        if arguments.commit.unwrap_or(false) {
            match imports::queue_commit(&mut transaction, &actor, import_id, None).await? {
                Ok(()) => committed = true,
                Err(CommitRefusal::NotFound) => return Ok(ToolResult::not_found("That import")),
                Err(CommitRefusal::NotDraft(status)) if status == "fetching" => {
                    return Ok(ToolResult::conflict(
                        "The issues are still being read. Call task_import with this importId again in a moment.",
                    ));
                }
                Err(CommitRefusal::NotDraft(status)) => {
                    return Ok(ToolResult::conflict(format!("This import is already {status}.")));
                }
                Err(CommitRefusal::VersionConflict(_)) => {
                    return Ok(ToolResult::conflict("The import changed; read it again."));
                }
                Err(CommitRefusal::Empty) => {
                    return Ok(ToolResult::invalid("text", "There's nothing to import."));
                }
            }
        }
        let Some(view) = imports::import_by_id(&mut transaction, project_id, import_id, !committed).await? else {
            return Ok(ToolResult::not_found("That import"));
        };
        transaction.commit().await?;
        let value = to_value(view)?;
        let next_step = match value["status"].as_str().unwrap_or_default() {
            "fetching" => "The issues are being read. Call task_import with this importId to see the preview.",
            "draft" => "Check the preview, then call task_import with this importId and commit: true.",
            "queued" | "running" => "The tasks are being created. Call task_import with this importId to follow it.",
            _ => "Done. Find the tasks with task_list (sort: updated).",
        };
        Ok::<_, AppError>(ToolResult::ok(TaskImportValue {
            import: value,
            next_step: next_step.to_owned(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_import"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectExportArguments {
    /// The project's ID, link or key.
    pub project_id: String,
    /// Include archived tasks.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectExportValue {
    /// The project's statuses, types, labels, milestones and tasks (at
    /// most 500 here; use the download links for everything).
    pub export: Value,
    pub json_url: String,
    pub csv_url: String,
}

const MCP_EXPORT_TASKS: usize = 500;

pub(super) async fn project_export(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectExportArguments,
) -> ToolResult<ProjectExportValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let project_id = principal.project_id;
    let result = async {
        can_view(state, principal).await?;
        let mut connection = state.pool.acquire().await?;
        let mut export = imports::export_json(
            &mut connection,
            project_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        if let Some(tasks) = export["tasks"].as_array_mut()
            && tasks.len() > MCP_EXPORT_TASKS
        {
            tasks.truncate(MCP_EXPORT_TASKS);
            export["truncated"] = Value::Bool(true);
        }
        Ok::<_, AppError>(ToolResult::ok(ProjectExportValue {
            export,
            json_url: public_url(
                state,
                &format!("/api/v1/projects/{project_id}/export?format=json"),
            ),
            csv_url: public_url(
                state,
                &format!("/api/v1/projects/{project_id}/export?format=csv"),
            ),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_export"))
}

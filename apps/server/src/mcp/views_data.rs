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
    writes::{WriteAccess, check_access_for},
};
use crate::{
    AppState,
    error::AppError,
    policy::Action,
    work_items::{
        imports::{self, CommitRefusal, CreateImportRequest, ImportOptions},
        list::ViewFilters,
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
    /// it, resolvedQuery ("me" replaced by you), version (for view_write)
    /// and a link.
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
            .map(|row| view_value(state, views::saved_view(row, principal.actor_id, false)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, AppError>(ToolResult::ok(ViewListValue {
            views,
            how_to_use: "Pass a view's name or id as `view` to task_list to list the tasks it shows on the task list; a filter you pass as well replaces the view's filter on that field.".to_owned(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "view_list"))
}

/// A view as agents see it: its link in full, and without the web's own
/// fields (whether you can edit or delete it, its place in the sidebar).
fn view_value(state: &AppState, mut view: views::SavedViewView) -> Result<Value, AppError> {
    view.href = public_url(state, &view.href);
    to_value(view).map(|mut value| {
        if let Some(object) = value.as_object_mut() {
            for key in ["canEdit", "canDelete", "position"] {
                object.remove(key);
            }
        }
        value
    })
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ViewWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The view to change, pin or delete: its ID, its name, or a built-in's
    /// key (my_tasks, ready_for_agents, blocked, recently_changed). Leave it
    /// out to save a new view.
    pub view: Option<String>,
    /// The new view's name, or a new name: 1 to 80 characters.
    pub name: Option<String>,
    /// true shares the view with everyone in the project (needs the right
    /// to edit its tasks); false keeps it yours alone. New views are yours
    /// alone unless you say so.
    pub shared: Option<bool>,
    /// The task list's query, as view_list shows it for other views: keys
    /// such as q, statusId, semanticCategory, priority, assigneeActorId
    /// ("me" means whoever opens it, "none" unassigned), labelIds,
    /// milestoneId, blocked, ready, claimed, dueBefore, includeDone, sort
    /// (priority, updated, number, due), groupBy, columnsBy and lanes. Values
    /// are strings; several IDs are comma-separated. On a change it replaces
    /// the whole query.
    pub query: Option<BTreeMap<String, String>>,
    /// "list" (the default) or "board".
    pub layout: Option<String>,
    /// The list columns to show: any of priority, key, status, details,
    /// due, assignee, or "field:<id>" for a custom field. Leave it out to
    /// keep them; [] shows only the title.
    pub fields: Option<Vec<String>>,
    /// true pins the view to your own sidebar, false unpins it. Nobody
    /// else's sidebar changes.
    pub pinned: Option<bool>,
    /// true deletes the view. Built-in views can't be deleted; unpin them
    /// instead.
    pub delete: Option<bool>,
    /// The version from view_list or an earlier view_write. Leave it out to
    /// change the current one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ViewWriteValue {
    /// The view as it is now (as view_list shows it, with its version).
    /// Absent once it's deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<Value>,
    /// Whether this call saved a new view.
    pub created: bool,
    /// Whether this call deleted the view.
    pub deleted: bool,
}

/// view_write's arguments, checked before anything is locked.
struct CheckedView {
    name: Option<String>,
    layout: Option<String>,
    fields: Option<Vec<String>>,
}

fn check_view_arguments(
    arguments: &ViewWriteArguments,
) -> Result<CheckedView, (&'static str, &'static str)> {
    let changes = arguments.name.is_some()
        || arguments.shared.is_some()
        || arguments.query.is_some()
        || arguments.layout.is_some()
        || arguments.fields.is_some()
        || arguments.pinned.is_some();
    if arguments.delete == Some(true) {
        if arguments.view.is_none() {
            return Err(("view", "Name the view to delete."));
        }
        if changes {
            return Err((
                "delete",
                "Deleting a view can't be combined with changing it.",
            ));
        }
    }
    if arguments.view.is_none() && arguments.name.is_none() {
        return Err((
            "name",
            "Give a new view a name, or pass view to change one (see view_list).",
        ));
    }
    let name = match arguments.name.as_deref() {
        None => None,
        Some(value) => Some(
            views::normalize_view_name(value)
                .ok_or(("name", "Give the view a name of 1 to 80 characters."))?,
        ),
    };
    if let Some(layout) = arguments.layout.as_deref() {
        views::check_layout(layout).map_err(|_| ("layout", "layout is \"list\" or \"board\"."))?;
    }
    let fields = views::normalize_fields(arguments.fields.as_deref()).map_err(|_| {
        (
            "fields",
            "fields are priority, key, status, details, due, assignee, or \"field:<id>\" for a custom field.",
        )
    })?;
    Ok(CheckedView {
        name,
        layout: arguments.layout.clone(),
        fields,
    })
}

fn view_outcome<T>(outcome: views::ViewOutcome<T>) -> Result<T, ToolResult<ViewWriteValue>> {
    match outcome {
        views::ViewOutcome::Done(value) => Ok(value),
        views::ViewOutcome::Invalid(field, message) => Err(ToolResult::invalid(field, message)),
        views::ViewOutcome::TooMany(message) => Err(ToolResult::conflict(message)),
        views::ViewOutcome::Builtin => Err(ToolResult::conflict(
            "Built-in views can't be deleted. Unpin it (pinned: false) to take it out of your sidebar.",
        )),
        views::ViewOutcome::VersionConflict(current) => Err(ToolResult::conflict(format!(
            "Someone changed this view first (now version {}). Read it again with view_list, then retry.",
            current.version
        ))),
    }
}

/// The view `wanted` names among those `principal` can see: by ID, name or
/// built-in key. Two different views with that name are no match.
async fn find_view(
    state: &AppState,
    principal: &McpPrincipal,
    wanted: &str,
) -> Result<Option<Uuid>, AppError> {
    let rows = views::visible_views(&state.pool, principal.project_id, principal.actor_id).await?;
    let wanted_id = parse_id(wanted);
    let wanted = wanted.trim();
    if let Some(row) = rows.iter().find(|row| Some(row.id) == wanted_id) {
        return Ok(Some(row.id));
    }
    let mut matches = rows.iter().filter(|row| {
        row.name.eq_ignore_ascii_case(wanted) || row.builtin.as_deref() == Some(wanted)
    });
    Ok(match (matches.next(), matches.next()) {
        (Some(row), None) => Some(row.id),
        _ => None,
    })
}

pub(super) async fn view_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ViewWriteArguments,
) -> ToolResult<ViewWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let checked = match check_view_arguments(&arguments) {
        Ok(checked) => checked,
        Err((field, message)) => return ToolResult::invalid(field, message),
    };
    let project_id = principal.project_id;
    let reader = principal.actor_id;
    let result = async {
        if let Some(query) = &arguments.query {
            match views::check_view_query(&state.pool, project_id, reader, query).await {
                Ok(()) => {}
                Err(AppError::Validation(field) | AppError::Invalid(field)) => {
                    return Ok(ToolResult::invalid(
                        "query",
                        format!(
                            "The task list can't read this query ({field}). Copy the keys a view in view_list uses; IDs must belong to this project."
                        ),
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        views::ensure_builtin_views(&state.pool, project_id).await?;
        let view_id = match arguments.view.as_deref() {
            None => None,
            Some(wanted) => match find_view(state, principal, wanted).await? {
                Some(id) => Some(id),
                None => {
                    return Ok(ToolResult::not_found(
                        "That view (pass its ID if two share a name; see view_list)",
                    ));
                }
            },
        };
        let mut transaction = state.pool.begin().await?;
        match check_access_for(&mut transaction, principal, Action::WorkItemView).await? {
            WriteAccess::Allowed => {}
            WriteAccess::ReadOnly => return Ok(ToolResult::read_only()),
            WriteAccess::NotFound => return Ok(ToolResult::not_found("That project")),
            WriteAccess::ProjectArchived => {
                return Ok(ToolResult::conflict("That project is archived."));
            }
            WriteAccess::Denied => {
                return Ok(ToolResult::denied("You can't see tasks in that project."));
            }
        }
        // Shared views change what everyone in the project sees, so they
        // take the right to edit its tasks, as on the web.
        let sharing = matches!(
            check_access_for(&mut transaction, principal, Action::WorkItemEdit).await?,
            WriteAccess::Allowed
        );
        let shared_denied = || {
            ToolResult::denied(
                "Only people who can edit this project's tasks can change its shared views. Save a view of your own instead (shared: false).",
            )
        };
        if arguments.shared == Some(true) && !sharing {
            return Ok(shared_denied());
        }
        // Checked under the project lock check_access_for took, so two calls
        // saving one new name can't both pass.
        if view_id.is_none()
            && let Some(name) = &checked.name
            && views::visible_views(&mut *transaction, project_id, reader)
                .await?
                .iter()
                .any(|row| row.name.eq_ignore_ascii_case(name))
        {
            return Ok(ToolResult::conflict(format!(
                "There's already a view called “{name}” here. Pass view (its name or ID, see view_list) to change it instead."
            )));
        }
        let actor = command_actor(principal, project_id);
        let mut created = false;
        let mut view = None;
        match view_id {
            None => {
                let new = views::NewView {
                    name: checked.name.clone().unwrap_or_default(),
                    shared: arguments.shared.unwrap_or(false),
                    query: arguments.query.clone().unwrap_or_default(),
                    layout: checked.layout.clone().unwrap_or_else(|| "list".to_owned()),
                    fields: checked.fields.clone(),
                };
                let outcome =
                    views::insert_view(&mut transaction, &actor, project_id, reader, sharing, &new)
                        .await?;
                match view_outcome(outcome) {
                    Ok(saved) => {
                        created = true;
                        view = Some(saved);
                    }
                    Err(refusal) => return Ok(refusal),
                }
            }
            Some(view_id) if arguments.delete == Some(true) => {
                let outcome = match views::remove_view(
                    &mut transaction,
                    &actor,
                    project_id,
                    view_id,
                    reader,
                    sharing,
                    arguments.expected_version,
                )
                .await
                {
                    Err(AppError::Forbidden) => return Ok(shared_denied()),
                    outcome => outcome?,
                };
                if let Err(refusal) = view_outcome(outcome) {
                    return Ok(refusal);
                }
                transaction.commit().await?;
                return Ok(ToolResult::ok(ViewWriteValue {
                    view: None,
                    created: false,
                    deleted: true,
                }));
            }
            Some(view_id) => {
                let change = views::ViewChange {
                    expected_version: arguments.expected_version,
                    name: checked.name.clone(),
                    shared: arguments.shared,
                    query: arguments.query.clone(),
                    layout: checked.layout.clone(),
                    fields: checked.fields.clone().map(Some),
                };
                let changing = change.name.is_some()
                    || change.shared.is_some()
                    || change.query.is_some()
                    || change.layout.is_some()
                    || change.fields.is_some();
                if changing {
                    let outcome = match views::change_view(
                        &mut transaction,
                        &actor,
                        project_id,
                        view_id,
                        reader,
                        sharing,
                        change,
                    )
                    .await
                    {
                        Err(AppError::Forbidden) => return Ok(shared_denied()),
                        outcome => outcome?,
                    };
                    match view_outcome(outcome) {
                        Ok(saved) => view = Some(saved),
                        Err(refusal) => return Ok(refusal),
                    }
                }
            }
        }
        let view_id = view.as_ref().map(|view| view.id).or(view_id);
        if let (Some(view_id), Some(pinned)) = (view_id, arguments.pinned) {
            view = Some(
                views::pin_view_for(
                    &mut transaction,
                    project_id,
                    view_id,
                    reader,
                    sharing,
                    pinned,
                    None,
                )
                .await?,
            );
        }
        let view = match (view, view_id) {
            (Some(view), _) => view,
            // Nothing to change: the view as it is.
            (None, Some(view_id)) => views::visible_views(&mut *transaction, project_id, reader)
                .await?
                .into_iter()
                .find(|row| row.id == view_id)
                .map(|row| views::saved_view(row, reader, sharing))
                .ok_or(AppError::NotFound("saved_view"))?,
            (None, None) => return Err(AppError::Internal("view_write made no view")),
        };
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(ViewWriteValue {
            view: Some(view_value(state, view)?),
            created,
            deleted: false,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "view_write"))
}

/// Reads a saved view for task_list: its filters, applied exactly as the
/// task list applies them, less the fields the caller passed (theirs
/// replace the view's), and its sort unless the caller gave one. The view's
/// "me" is the caller.
pub(super) async fn apply_view(
    state: &AppState,
    principal: &McpPrincipal,
    mut arguments: TaskListArguments,
) -> Result<Result<(TaskListArguments, Option<ViewFilters>), (&'static str, String)>, AppError> {
    let Some(wanted) = arguments.view.clone() else {
        return Ok(Ok((arguments, None)));
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
    let mut query = views::query_params(&row.query);
    let passed = [
        (arguments.query.is_some(), &["q"][..]),
        (
            arguments.status.is_some() || arguments.not_status.is_some(),
            &[
                "statusId",
                "notStatusId",
                "semanticCategory",
                "notSemanticCategory",
            ],
        ),
        (
            arguments.assignee.is_some() || arguments.not_assignee.is_some(),
            &["assigneeActorId", "notAssigneeActorId", "unassigned"],
        ),
        (
            arguments.labels.is_some()
                || arguments.any_labels.is_some()
                || arguments.not_labels.is_some(),
            &["labelIds", "anyLabelIds", "notLabelIds"],
        ),
        (
            arguments.task_type.is_some() || arguments.not_type.is_some(),
            &["typeId", "notTypeId"],
        ),
        (
            arguments.priority.is_some() || arguments.not_priority.is_some(),
            &["priority", "notPriority"],
        ),
        (
            arguments.milestone.is_some() || arguments.not_milestone.is_some(),
            &["milestoneId", "notMilestoneId"],
        ),
        (arguments.parent_id.is_some(), &["parentId"]),
        (arguments.due_before.is_some(), &["dueBefore"]),
        (arguments.due_after.is_some(), &["dueAfter"]),
        (arguments.blocked.is_some(), &["blocked"]),
        (arguments.ready.is_some(), &["ready"]),
        (arguments.overdue.is_some(), &["overdue"]),
        (arguments.claimed.is_some(), &["claimed"]),
        (arguments.include_done.is_some(), &["includeDone"]),
        (arguments.include_archived.is_some(), &["archived"]),
    ];
    for (given, keys) in passed {
        if given {
            for key in keys {
                query.remove(*key);
            }
        }
    }
    if arguments.sort.is_none() {
        arguments.sort = query
            .get("sort")
            .filter(|sort| matches!(sort.as_str(), "priority" | "updated" | "number"))
            .cloned();
    }
    let Ok(mut filters) = ViewFilters::parse(&query, principal.actor_id) else {
        return Ok(Err((
            "view",
            format!(
                "“{}” can't be read any more. Open it on the task list and save it again.",
                row.name
            ),
        )));
    };
    filters
        .resolve_field_me(&state.pool, principal.project_id, principal.actor_id)
        .await?;
    // The task list shows finished tasks unless the view hides them.
    arguments.include_done.get_or_insert(true);
    match arguments.include_archived {
        Some(_) => filters.leave_archived_to_caller(),
        None => arguments.include_archived = Some(filters.archived_only()),
    }
    Ok(Ok((arguments, Some(filters))))
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
        // AWD-75: only projects where Tasks is on for the caller, as task_list.
        let projects = {
            let mut connection = state.pool.acquire().await?;
            crate::modules::state::ProjectModules::load(
                &mut connection,
                principal.actor_id,
                &projects,
            )
            .await?
            .with(&projects, "tasks")
        };
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
        if let Some(refusal) = access_refusal(
            check_access_for(&mut transaction, principal, Action::WorkItemCreate).await?,
        ) {
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
        if let Some(refusal) = access_refusal(
            check_access_for(&mut transaction, principal, Action::WorkItemCreate).await?,
        ) {
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
        let Some(view) = imports::import_by_id(&mut transaction, project_id, import_id, !committed, principal.actor_id).await? else {
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

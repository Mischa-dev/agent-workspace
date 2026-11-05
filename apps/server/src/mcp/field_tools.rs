//! Custom field tools (AWD-89): list a project's task fields, and add,
//! rename, reorder, archive or restore one (owners and admins). Values go on
//! tasks through task_create and task_update (`fields`), filter in
//! task_list (`fieldFilters`) and read back in task_open and task_list. The
//! rules are `crate::fields` and `crate::work_item_fields`, the same the web
//! app uses.
//!
//! `field` is never resolved by the MCP middleware: it is looked up only
//! among the fields of `principal.project_id`, so an ID from another project
//! never matches.

use std::collections::HashMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, StoredRole, ToolResult, insert_audit, mark_via, parse_id,
        role_allows,
    },
    talk::can_write_in,
    tasks::TaskSummary,
};
use crate::{
    AppState,
    error::AppError,
    fields::{
        self, ChangeFieldOutcome, CreateFieldOutcome, FieldChange, FieldClause, FieldOp, FieldType,
        FieldView, LinkTarget, NewField, OptionInput,
    },
    memory,
    policy::Action,
    work_item_commands::lock_membership,
    work_item_fields::{self, FieldValueChanges, KIND, ValueProblem, Writer},
    work_item_labels::echo,
};

const MESSAGES_SPACE_MESSAGE: &str =
    "The Messages space has no tasks, so it has no fields. Pick a project with projects_list.";
const MANAGERS_MESSAGE: &str = "Only people whose role can change the workflow (owners and admins, unless a custom role says otherwise) can change the project's fields. Anyone who can edit tasks sets their values with task_update.";

// ---------------------------------------------------------------------------
// field_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FieldListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Also list archived fields, which field_write can restore.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct FieldListValue {
    /// The project's task fields in order, then archived ones when asked
    /// for. Set values with task_update's `fields`, by name.
    pub fields: Vec<FieldView>,
    /// Whether you can add and change fields here (owners and admins).
    pub can_manage: bool,
}

pub(super) async fn field_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: FieldListArguments,
) -> ToolResult<FieldListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        let messages_space = memory::project_access(
            &state.pool,
            principal.actor_id,
            principal.actor_kind,
            principal.project_id,
            Action::WorkItemView,
        )
        .await?;
        if messages_space {
            return Ok(ToolResult::invalid("projectId", MESSAGES_SPACE_MESSAGE));
        }
        let definitions = work_item_fields::definitions(
            &state.pool,
            principal.project_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        // Defining fields is the workflow's permission (AWD-74 roles):
        // owners and admins, or a role that kept "Change the workflow".
        let can_manage =
            can_write_in(state, principal, principal.project_id, Action::WorkflowManage).await?
                && sqlx::query_scalar::<_, bool>(
                    "SELECT archived_at IS NULL FROM projects WHERE id = $1",
                )
                .bind(principal.project_id)
                .fetch_one(&state.pool)
                .await?;
        Ok::<_, AppError>(ToolResult::ok(FieldListValue {
            fields: definitions.iter().map(FieldView::from).collect(),
            can_manage,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "field_list"))
}

// ---------------------------------------------------------------------------
// field_write

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FieldWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The field to change, by ID or current name. Leave it out to add one.
    pub field: Option<String>,
    /// The new field's name, or a new name. 1 to 60 characters, unique in
    /// the project ignoring case.
    pub name: Option<String>,
    /// For a new field: text, number, date, single_choice, multiple_choice,
    /// person, url, checkbox, or link. It never changes afterwards.
    #[serde(rename = "type")]
    pub field_type: Option<String>,
    /// What the field is for, on one line. "" clears it.
    pub description: Option<String>,
    /// Choice fields: every option, in order. Keep an existing option (and
    /// the tasks that have it) by passing its id; one left out is archived,
    /// never removed.
    pub options: Option<Vec<OptionInput>>,
    /// For a new link field: "task" or "page".
    pub target: Option<String>,
    /// Its place among the project's fields, 1 being first.
    pub position: Option<i32>,
    /// true archives it (hidden everywhere, values kept), false restores it.
    pub archived: Option<bool>,
    /// The version from field_list. Leave it out to change the current one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct FieldWriteValue {
    pub field: FieldView,
    /// False when it already existed, or nothing needed to change.
    pub created: bool,
    /// "field" on create; otherwise any of name, description, options,
    /// position, archived. Empty when nothing changed.
    pub changed_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub(super) async fn field_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: FieldWriteArguments,
) -> ToolResult<FieldWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let name = match arguments.name.as_deref() {
        Some(value) => match fields::normalize_name(value) {
            Some(name) => Some(name),
            None => {
                return ToolResult::invalid(
                    "name",
                    "Give the field a name of 1 to 60 characters on one line, that isn't an ID.",
                );
            }
        },
        None => None,
    };
    let description = match arguments.description.as_deref() {
        Some(value) => match fields::normalize_description(value) {
            Some(text) => Some(text),
            None => {
                return ToolResult::invalid(
                    "description",
                    "Keep the description to one line of at most 300 characters.",
                );
            }
        },
        None => None,
    };
    if arguments.field.is_some() && arguments.field_type.is_some() {
        return ToolResult::invalid(
            "type",
            "A field's type can't change. Add a new field with the type you want.",
        );
    }
    let project_id = principal.project_id;
    let result = async {
        if !can_write_in(state, principal, project_id, Action::WorkflowManage).await? {
            return Ok(ToolResult::denied(MANAGERS_MESSAGE));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let Some(membership) =
            lock_membership(&mut transaction, principal.actor_id, project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !membership.enabled {
            return Ok(ToolResult::not_found("That project"));
        }
        if membership.project_archived_at.is_some() {
            return Ok(ToolResult::conflict(ARCHIVED_PROJECT_MESSAGE));
        }
        // Checked again under the project lock, with the role as it is now
        // and any agent limit.
        if !role_allows(
            principal,
            project_id,
            &StoredRole::of(&membership.role, membership.role_permissions.as_deref()),
            Action::WorkflowManage,
        ) {
            return Ok(ToolResult::denied(MANAGERS_MESSAGE));
        }
        let (kind, key) = sqlx::query_as::<_, (String, String)>(
            "SELECT kind, project_key FROM projects WHERE id = $1",
        )
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        if kind == "direct" {
            return Ok(ToolResult::invalid("projectId", MESSAGES_SPACE_MESSAGE));
        }
        let definitions = work_item_fields::definitions(&mut *transaction, project_id, true).await?;

        let Some(reference) = arguments.field.as_deref() else {
            // Add a field.
            let Some(name) = name else {
                return Ok(ToolResult::invalid(
                    "name",
                    "Give the new field a name, or pass field to change one.",
                ));
            };
            let Some(field_type) = arguments.field_type.as_deref().and_then(FieldType::parse)
            else {
                return Ok(ToolResult::invalid(
                    "type",
                    "Pick a type: text, number, date, single_choice, multiple_choice, person, url, checkbox, or link.",
                ));
            };
            if arguments.archived == Some(true) || arguments.expected_version.is_some() {
                return Ok(ToolResult::invalid(
                    "field",
                    "archived and expectedVersion are for changing a field; pass field to change one.",
                ));
            }
            let config = match fields::normalize_config(
                field_type,
                None,
                arguments.options.as_deref(),
                arguments.target.as_deref(),
            ) {
                Ok(config) => config,
                Err(problem) => return Ok(ToolResult::invalid(problem.field, problem.message)),
            };
            let outcome = fields::create(
                &mut transaction,
                project_id,
                KIND,
                principal.actor_id,
                NewField {
                    name,
                    description: description.unwrap_or_default(),
                    field_type,
                    config,
                    position: arguments.position,
                },
            )
            .await?;
            let value = match outcome {
                CreateFieldOutcome::Created(field) => {
                    insert_audit(
                        &mut transaction,
                        principal,
                        project_id,
                        "field_definition.mcp_created",
                        "field_definition",
                        field.id,
                        serde_json::json!({
                            "name": field.name,
                            "type": field.field_type.as_str(),
                            "recordKind": KIND.as_str(),
                        }),
                    )
                    .await?;
                    FieldWriteValue {
                        field: FieldView::from(&field),
                        created: true,
                        changed_fields: vec!["field".to_owned()],
                        note: None,
                    }
                }
                CreateFieldOutcome::NameTaken(field) => FieldWriteValue {
                    note: Some(format!(
                        "A field called \"{0}\" already exists in {key}, so nothing changed. To change it, pass field: \"{0}\".",
                        field.name
                    )),
                    field: FieldView::from(&field),
                    created: false,
                    changed_fields: Vec::new(),
                },
                CreateFieldOutcome::ProjectFull => {
                    return Ok(ToolResult::conflict(format!(
                        "{key} already has {} task fields, or keeps {} counting archived ones. Archive or restore one instead.",
                        fields::MAX_ACTIVE_FIELDS,
                        fields::MAX_FIELD_ROWS
                    )));
                }
            };
            transaction.commit().await?;
            return Ok(ToolResult::ok(value));
        };

        // Change a field. A restore by name finds the archived field.
        let restoring = arguments.archived == Some(false);
        let found = if restoring {
            let wanted = reference.trim().to_lowercase();
            definitions
                .iter()
                .find(|field| {
                    field.archived
                        && (field.id.to_string() == wanted || field.name.to_lowercase() == wanted)
                })
                .or_else(|| fields::find(&definitions, reference))
        } else {
            fields::find(&definitions, reference)
        };
        let Some(current) = found.cloned() else {
            return Ok(ToolResult::NotFound {
                message: format!(
                    "No field called \"{}\" in {key}. Its fields: {}. Leave out field to add one.",
                    echo(reference),
                    fields::active_names(&definitions)
                ),
            });
        };
        if arguments.target.is_some() {
            return Ok(ToolResult::invalid(
                "target",
                "A link field's target can't change. Add a new field instead.",
            ));
        }
        let config = match arguments.options.as_deref() {
            Some(options) => match fields::normalize_config(
                current.field_type,
                Some(&current.config),
                Some(options),
                None,
            ) {
                Ok(config) => Some(config),
                Err(problem) => return Ok(ToolResult::invalid(problem.field, problem.message)),
            },
            None => None,
        };
        if name.is_none()
            && description.is_none()
            && config.is_none()
            && arguments.position.is_none()
            && arguments.archived.is_none()
        {
            return Ok(ToolResult::invalid(
                "field",
                "Say what to change: name, description, options, position, or archived.",
            ));
        }
        let outcome = fields::change(
            &mut transaction,
            project_id,
            KIND,
            current.id,
            principal.actor_id,
            FieldChange {
                name,
                description,
                config,
                position: arguments.position,
                archived: arguments.archived,
                expected_version: arguments.expected_version,
            },
        )
        .await?;
        let value = match outcome {
            ChangeFieldOutcome::Changed {
                field,
                previous,
                changed,
            } => {
                let action = match (previous.archived, field.archived) {
                    (false, true) => "field_definition.mcp_archived",
                    (true, false) => "field_definition.mcp_restored",
                    _ => "field_definition.mcp_updated",
                };
                insert_audit(
                    &mut transaction,
                    principal,
                    project_id,
                    action,
                    "field_definition",
                    field.id,
                    serde_json::json!({
                        "changedFields": changed,
                        "previous": { "name": previous.name },
                        "previousVersion": previous.version,
                    }),
                )
                .await?;
                FieldWriteValue {
                    note: field.archived.then(|| {
                        "Archived: it's hidden everywhere, and tasks keep their values for when it's restored.".to_owned()
                    }),
                    field: FieldView::from(&field),
                    created: false,
                    changed_fields: changed.into_iter().map(str::to_owned).collect(),
                }
            }
            ChangeFieldOutcome::Unchanged(field) => FieldWriteValue {
                field: FieldView::from(&field),
                created: false,
                changed_fields: Vec::new(),
                note: Some("It already looks like that; nothing changed.".to_owned()),
            },
            ChangeFieldOutcome::NotFound => {
                return Ok(ToolResult::not_found("That field"));
            }
            ChangeFieldOutcome::VersionConflict(field) => {
                return Ok(ToolResult::conflict(format!(
                    "\"{}\" changed since you read it (now version {}). Read it again with field_list and retry.",
                    field.name, field.version
                )));
            }
            ChangeFieldOutcome::NameTaken(other) => {
                return Ok(ToolResult::invalid(
                    "name",
                    format!("Another field in {key} is already called \"{other}\"."),
                ));
            }
            ChangeFieldOutcome::ProjectFull => {
                return Ok(ToolResult::conflict(format!(
                    "{key} already has {} active task fields. Archive one first.",
                    fields::MAX_ACTIVE_FIELDS
                )));
            }
        };
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "field_write"))
}

// ---------------------------------------------------------------------------
// Values on tasks, for the task tools

/// One of a task's fields as task_open shows it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskFieldValue {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    /// The value: text, a number, YYYY-MM-DD, an option's name (a list of
    /// them for multiple choice), a person's @handle, a URL, true, a task's
    /// key or a page's title. null when it has none.
    pub value: Value,
}

/// A task's values for every active field of its project, in order, empty
/// ones as null, so an agent sees what it can fill in.
pub(super) async fn task_fields(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<TaskFieldValue>, AppError> {
    let definitions = work_item_fields::definitions(&mut *connection, project_id, false).await?;
    if definitions.is_empty() {
        return Ok(Vec::new());
    }
    let values = sqlx::query_scalar::<_, Value>(
        "SELECT field_values FROM work_items WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or(Value::Object(Map::new()));
    let references =
        work_item_fields::references(connection, project_id, &definitions, [&values]).await?;
    let named = work_item_fields::named_values(&definitions, &values, &references)
        .into_iter()
        .map(|(field, value)| (field.id, value))
        .collect::<HashMap<_, _>>();
    Ok(definitions
        .into_iter()
        .map(|field| TaskFieldValue {
            value: named.get(&field.id).cloned().unwrap_or(Value::Null),
            name: field.name,
            field_type: field.field_type,
        })
        .collect())
}

/// Fills in each task line's `fields` (by name, only the ones with a value).
pub(super) async fn fill_summary_fields(
    connection: &mut PgConnection,
    tasks: &mut [TaskSummary],
) -> Result<(), AppError> {
    let ids = tasks
        .iter()
        .filter_map(|task| task.id.parse::<Uuid>().ok())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, Value)>(
        "SELECT id, project_id, field_values FROM work_items WHERE id = ANY($1) AND field_values <> '{}'::jsonb",
    )
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut by_project: HashMap<Uuid, Vec<(Uuid, Value)>> = HashMap::new();
    for (id, project_id, values) in rows {
        by_project.entry(project_id).or_default().push((id, values));
    }
    let mut filled: HashMap<Uuid, Map<String, Value>> = HashMap::new();
    for (project_id, rows) in by_project {
        let definitions =
            work_item_fields::definitions(&mut *connection, project_id, false).await?;
        if definitions.is_empty() {
            continue;
        }
        let references = work_item_fields::references(
            connection,
            project_id,
            &definitions,
            rows.iter().map(|(_, values)| values),
        )
        .await?;
        for (id, values) in rows {
            let named = work_item_fields::named_values(&definitions, &values, &references);
            if !named.is_empty() {
                filled.insert(
                    id,
                    named
                        .into_iter()
                        .map(|(field, value)| (field.name, value))
                        .collect(),
                );
            }
        }
    }
    for task in tasks {
        if let Some(values) = task
            .id
            .parse::<Uuid>()
            .ok()
            .and_then(|id| filled.remove(&id))
        {
            task.fields = Some(values);
        }
    }
    Ok(())
}

/// What task_create and task_update's `fields` wrote, checked and resolved
/// in this transaction, as the web app's `fieldValues` are.
pub(super) async fn written_values(
    connection: &mut PgConnection,
    principal: &McpPrincipal,
    project_id: Uuid,
    written: &Map<String, Value>,
) -> Result<Result<FieldValueChanges, ValueProblem>, AppError> {
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?;
    let definitions = work_item_fields::definitions(&mut *connection, project_id, true).await?;
    let written = written
        .iter()
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect::<Vec<_>>();
    work_item_fields::resolve(
        connection,
        project_id,
        Writer {
            actor_id: principal.actor_id,
            project_key: &project_key,
        },
        &definitions,
        "fields",
        &written,
    )
    .await
}

/// One task_list field filter as an agent writes it.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FieldFilterArgument {
    /// The field, by name or ID (see field_list).
    pub field: String,
    /// is (any of the values), isNot, empty, notEmpty, contains (text),
    /// gt, gte, lt, lte (numbers and dates).
    pub op: FieldOp,
    /// What to compare with: option names, "me" or @handles, task keys,
    /// numbers, YYYY-MM-DD dates, true or false. Not for empty and
    /// notEmpty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// task_list's field filters for one project, as stored values. The message
/// says what's wrong and what would work.
pub(super) async fn filter_clauses(
    connection: &mut PgConnection,
    principal: &McpPrincipal,
    project_id: Uuid,
    project_key: &str,
    filters: &[FieldFilterArgument],
) -> Result<Result<Vec<FieldClause>, String>, AppError> {
    if filters.len() > fields::MAX_CLAUSES {
        return Ok(Err(format!(
            "Pass at most {} field filters.",
            fields::MAX_CLAUSES
        )));
    }
    let definitions = work_item_fields::definitions(&mut *connection, project_id, true).await?;
    let mut clauses = Vec::with_capacity(filters.len());
    for filter in filters {
        let Some(field) = fields::find(&definitions, &filter.field) else {
            return Ok(Err(format!(
                "{project_key} has no task field called \"{}\". Its fields: {}.",
                echo(&filter.field),
                fields::active_names(&definitions)
            )));
        };
        let mut values = Vec::with_capacity(filter.values.len());
        for value in &filter.values {
            match field.field_type {
                FieldType::Person => {
                    match work_item_fields::member_id(
                        connection,
                        project_id,
                        principal.actor_id,
                        value,
                    )
                    .await?
                    {
                        Some(id) => values.push(id.to_string()),
                        None => {
                            return Ok(Err(format!(
                                "No one called {} is in {project_key}.",
                                echo(value)
                            )));
                        }
                    }
                }
                FieldType::Link => {
                    let target = field.config.target.unwrap_or(LinkTarget::Task);
                    match work_item_fields::find_link(
                        connection,
                        project_id,
                        Writer {
                            actor_id: principal.actor_id,
                            project_key,
                        },
                        target,
                        value,
                    )
                    .await?
                    {
                        Some((id, _)) => values.push(id.to_string()),
                        None => {
                            return Ok(Err(format!(
                                "\"{}\" links to a {} in {project_key}; {} isn't one.",
                                field.name,
                                if target == LinkTarget::Page {
                                    "page"
                                } else {
                                    "task"
                                },
                                echo(value)
                            )));
                        }
                    }
                }
                _ => values.push(value.clone()),
            }
        }
        match fields::clause_from_written(field, filter.op, &values) {
            Ok(clause) => clauses.push(clause),
            Err(message) => return Ok(Err(message)),
        }
    }
    Ok(Ok(clauses))
}

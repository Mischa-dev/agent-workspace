//! AWD-111: project templates through MCP. Anyone in a workspace can list
//! its project templates; a person, through an app connected as themselves,
//! saves a project they manage as one and changes the ones they made (or
//! any, if they manage the workspace's settings), as Settings does on the
//! web. Agents acting as themselves read them only. Everything goes through
//! `crate::work_items::project_templates`, the code the web app uses.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, parse_id, project_role},
    writes::{WriteAccess, check_access_for},
};
use crate::{
    AppState,
    error::AppError,
    policy::{Action, ActorKind},
    work_items::project_templates::{
        self as templates, ProjectTemplateOutcome, ProjectTemplateView, TemplateChange,
        TemplateReader,
    },
};

const PEOPLE_WRITE_TEMPLATES: &str = "Agents can read project templates, but only people save or change them, on the web or through an app connected as themselves.";

/// A project template, as a picker shows it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectTemplateLine {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The workflow's statuses and types, by name.
    pub statuses: Vec<String>,
    pub types: Vec<String>,
    /// How many labels, settings and task templates it sets up.
    pub labels: usize,
    pub channels: Vec<String>,
    pub settings: usize,
    pub task_templates: usize,
    /// The project it was saved from, for members of that project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    pub archived: bool,
    /// Whether you may rename or archive it.
    pub can_edit: bool,
    /// Pass as expectedVersion to change it.
    pub version: i64,
}

impl From<ProjectTemplateView> for ProjectTemplateLine {
    fn from(view: ProjectTemplateView) -> Self {
        Self {
            id: view.id.to_string(),
            name: view.name,
            description: view.description,
            statuses: view.contains.statuses,
            types: view.contains.types,
            labels: view.contains.labels,
            channels: view.contains.channels,
            settings: view.contains.settings,
            task_templates: view.contains.task_templates,
            source_project: view.source_project_name,
            created_by: view.created_by_name,
            archived: view.archived,
            can_edit: view.can_edit,
            version: view.version,
        }
    }
}

fn reader(principal: &McpPrincipal) -> TemplateReader {
    TemplateReader {
        actor_id: principal.actor_id,
        human: principal.actor_kind == ActorKind::Human,
    }
}

/// The workspace of the project the call names (the middleware scoped the
/// caller to it), once the caller is checked to be in that project.
async fn workspace_of(
    state: &AppState,
    principal: &McpPrincipal,
    project: &str,
) -> Result<Option<Uuid>, AppError> {
    if parse_id(project) != Some(principal.project_id)
        || project_role(state, principal, principal.project_id)
            .await?
            .is_none()
    {
        return Ok(None);
    }
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(principal.project_id)
            .fetch_optional(&state.pool)
            .await?,
    )
}

// ---------------------------------------------------------------------------
// project_template_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectTemplateListArguments {
    /// A project in the workspace: its ID, link or key (such as "AWD").
    pub project_id: String,
    /// Include archived templates too.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectTemplateListValue {
    pub templates: Vec<ProjectTemplateLine>,
}

pub(super) async fn project_template_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectTemplateListArguments,
) -> ToolResult<ProjectTemplateListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut connection = state.pool.acquire().await?;
        let templates = templates::templates_for(
            &mut connection,
            reader(principal),
            organization_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(ProjectTemplateListValue {
            templates: templates.into_iter().map(Into::into).collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_template_list"))
}

// ---------------------------------------------------------------------------
// project_template_write

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectTemplateWriteArguments {
    /// Without template: the project to save as a template (you must manage
    /// it). With template: any project in the workspace.
    pub project_id: String,
    /// A template to change, by ID or name. Leave it out to save projectId
    /// as a new template.
    pub template: Option<String>,
    /// The template's name: required when saving, a rename when changing.
    pub name: Option<String>,
    pub description: Option<String>,
    /// Archive the template (true) or bring it back (false).
    pub archived: Option<bool>,
    /// The version you last read (project_template_list); a change is
    /// refused if the template moved on since. Defaults to the current one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectTemplateWriteValue {
    /// "saved" or "changed".
    pub done: String,
    pub template: ProjectTemplateLine,
}

pub(super) async fn project_template_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectTemplateWriteArguments,
) -> ToolResult<ProjectTemplateWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_WRITE_TEMPLATES);
    }
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut transaction = state.pool.begin().await?;
        let (done, outcome, template_id) = match arguments.template.as_deref() {
            None => {
                if arguments.archived.is_some() || arguments.expected_version.is_some() {
                    return Ok(ToolResult::invalid(
                        "template",
                        "Name the template to archive or change; leave template out only to save a project as a new one.",
                    ));
                }
                let Some(name) = arguments.name.as_deref() else {
                    return Ok(ToolResult::invalid("name", "Give the new template a name."));
                };
                match check_access_for(&mut transaction, principal, Action::SettingsManage)
                    .await?
                {
                    WriteAccess::Allowed => {}
                    WriteAccess::ReadOnly => return Ok(ToolResult::read_only()),
                    WriteAccess::NotFound => return Ok(ToolResult::not_found("That project")),
                    WriteAccess::ProjectArchived => {
                        return Ok(ToolResult::conflict("That project is archived."));
                    }
                    WriteAccess::Denied => {
                        return Ok(ToolResult::denied(
                            "Only people who manage a project save it as a template.",
                        ));
                    }
                }
                let outcome = templates::save_project_template(
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    principal.project_id,
                    name,
                    arguments.description.as_deref().unwrap_or(""),
                )
                .await?;
                ("saved", outcome, None)
            }
            Some(wanted) => {
                let Some((template_id, version)) =
                    find_template(&mut transaction, organization_id, wanted).await?
                else {
                    return Ok(ToolResult::not_found(
                        "That project template (pass its ID if two share a name; see project_template_list)",
                    ));
                };
                let outcome = templates::change_project_template(
                    &mut transaction,
                    reader(principal),
                    organization_id,
                    template_id,
                    TemplateChange {
                        expected_version: arguments.expected_version.unwrap_or(version),
                        name: arguments.name.as_deref(),
                        description: arguments.description.as_deref(),
                        archived: arguments.archived,
                    },
                )
                .await?;
                ("changed", outcome, Some(template_id))
            }
        };
        let (view, archived_now) = match outcome {
            ProjectTemplateOutcome::Done(view, archived_now) => (view, archived_now),
            ProjectTemplateOutcome::Invalid(field, message) => {
                return Ok(ToolResult::invalid(field, message));
            }
            ProjectTemplateOutcome::NameTaken => {
                return Ok(ToolResult::conflict(
                    "The workspace already has a project template with that name.",
                ));
            }
            ProjectTemplateOutcome::LimitReached => {
                return Ok(ToolResult::conflict(
                    "The workspace keeps 100 project templates at most. Archive one first.",
                ));
            }
            ProjectTemplateOutcome::TooLarge => {
                return Ok(ToolResult::invalid(
                    "projectId",
                    "This project's setup is too large to save as a template.",
                ));
            }
        };
        let action = match (template_id, archived_now) {
            (None, _) => "project_template.created",
            (Some(_), true) => "project_template.archived",
            (Some(_), false) => "project_template.updated",
        };
        insert_audit(
            &mut transaction,
            principal,
            principal.project_id,
            action,
            "project_template",
            view.id,
            serde_json::json!({ "organizationId": organization_id }),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(ProjectTemplateWriteValue {
            done: done.to_owned(),
            template: (*view).into(),
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(
            "Only whoever made a template, or someone who manages the workspace's settings, changes it.",
        ),
        Err(AppError::VersionConflict {
            current_version, ..
        }) => ToolResult::conflict(format!(
            "The template changed since you read it; it's at version {current_version} now. Read it again and retry."
        )),
        Err(AppError::NotFound(_)) => ToolResult::not_found("That project template"),
        Err(error) => ToolResult::from_error(error, "project_template_write"),
    }
}

/// A template of the workspace by ID, else by name: an active one first,
/// then a single archived one. Two with the same name are ambiguous.
pub(super) async fn find_template(
    executor: &mut sqlx::PgConnection,
    organization_id: Uuid,
    wanted: &str,
) -> Result<Option<(Uuid, i64)>, AppError> {
    let wanted = wanted.trim();
    if let Some(id) = parse_id(wanted) {
        return Ok(sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT id, version FROM project_templates WHERE organization_id = $1 AND id = $2",
        )
        .bind(organization_id)
        .bind(id)
        .fetch_optional(&mut *executor)
        .await?);
    }
    for archived in [false, true] {
        let found = sqlx::query_as::<_, (Uuid, i64)>(
            r#"
            SELECT id, version FROM project_templates
            WHERE organization_id = $1 AND lower(name) = lower($2)
              AND (archived_at IS NOT NULL) = $3
            ORDER BY id
            LIMIT 2
            "#,
        )
        .bind(organization_id)
        .bind(wanted)
        .bind(archived)
        .fetch_all(&mut *executor)
        .await?;
        match found.as_slice() {
            [] => continue,
            [one] => return Ok(Some(*one)),
            _ => return Ok(None),
        }
    }
    Ok(None)
}

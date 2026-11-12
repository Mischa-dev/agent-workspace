//! AWD-76: workspace templates for agents and apps. Anyone in a workspace
//! can read its templates, built in and saved, with what each does in plain
//! words, and suggest one. Only a person who manages the workspace's
//! settings and roles uses or saves one, through an app acting for them;
//! an agent acting as itself never does. Everything goes through
//! `crate::workspace_templates`, the same code the web app uses, and is
//! audited the same way.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id, project_role},
};
use crate::{
    AppState,
    error::AppError,
    policy::ActorKind,
    roles::{self, Auditor},
    workspace_templates::{
        self as templates, Applied, ApplyError, SaveOutcome, TemplateUse, TemplateView,
    },
};

const PEOPLE_USE_TEMPLATES: &str = "Agents can read templates and suggest one, but only a workspace owner or admin uses or saves them, in Settings > Workspace templates or through an app connected as themselves.";
const MANAGERS_USE_TEMPLATES: &str = "Only people who manage the workspace's settings and roles use or save templates. Suggest one to a workspace owner or admin instead.";

/// The workspace of the project a call names, once the caller is checked
/// to be in that project.
async fn workspace_of(
    state: &AppState,
    principal: &McpPrincipal,
    project: &str,
) -> Result<Option<Uuid>, AppError> {
    if parse_id(project) != Some(principal.project_id) {
        return Ok(None);
    }
    if project_role(state, principal, principal.project_id)
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
// workspace_template_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkspaceTemplateListArguments {
    /// A project in the workspace: its ID, link or key (such as "AWD").
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkspaceTemplateListValue {
    /// Built-in templates first (Everything first), then the workspace's
    /// saved ones.
    pub templates: Vec<TemplateView>,
    /// The last template used here, and what it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<TemplateUse>,
    /// Whether this connection may use and save templates. When it can't,
    /// suggest one to a workspace owner or admin.
    pub can_use: bool,
}

pub(super) async fn workspace_template_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WorkspaceTemplateListArguments,
) -> ToolResult<WorkspaceTemplateListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut connection = state.pool.acquire().await?;
        let caller = roles::standing(&mut connection, organization_id, principal.actor_id).await?;
        Ok::<_, AppError>(ToolResult::ok(WorkspaceTemplateListValue {
            templates: templates::list(&mut connection, organization_id).await?,
            last_used: templates::last_use(&mut connection, organization_id).await?,
            can_use: principal.can_write() && templates::may_use(caller),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "workspace_template_list"))
}

// ---------------------------------------------------------------------------
// workspace_template_apply

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkspaceTemplateApplyArguments {
    /// A project in the workspace: its ID, link or key.
    pub project_id: String,
    /// The template: a built-in one's id or name (such as "software_team"
    /// or "Software team"), or a saved one's ID or name, from
    /// workspace_template_list.
    pub template: String,
}

pub(super) async fn workspace_template_apply(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WorkspaceTemplateApplyArguments,
) -> ToolResult<Applied> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_USE_TEMPLATES);
    }
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut transaction = state.pool.begin().await?;
        let caller = roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
        if !templates::may_use(caller) {
            return Ok(ToolResult::denied(MANAGERS_USE_TEMPLATES));
        }
        let Some(template) =
            templates::find(&mut transaction, organization_id, &arguments.template).await?
        else {
            return Ok(ToolResult::not_found("That template"));
        };
        let auditor = Auditor::App(principal);
        match templates::apply(
            &mut transaction,
            &auditor,
            caller,
            organization_id,
            &template,
        )
        .await
        {
            Ok(applied) => {
                transaction.commit().await?;
                Ok(ToolResult::ok(applied))
            }
            Err(ApplyError::Problem(problem)) => {
                Ok(ToolResult::invalid(&problem.field, problem.message))
            }
            Err(ApplyError::App(error)) => Err(error),
        }
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "workspace_template_apply"))
}

// ---------------------------------------------------------------------------
// workspace_template_save

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkspaceTemplateSaveArguments {
    /// A project in the workspace: its ID, link or key.
    pub project_id: String,
    /// The new template's name, 1 to 60 characters. Leave it out with
    /// remove.
    #[serde(default)]
    pub name: Option<String>,
    /// What it's for, on one line.
    #[serde(default)]
    pub description: Option<String>,
    /// True to carry the project named by projectId as the template's
    /// starter project, with its channels and collections. Default false.
    #[serde(default)]
    pub include_project: Option<bool>,
    /// A saved template to remove, by ID or name. Built-in templates stay.
    #[serde(default)]
    pub remove: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkspaceTemplateSaveValue {
    /// saved or removed.
    pub done: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<TemplateView>,
}

pub(super) async fn workspace_template_save(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WorkspaceTemplateSaveArguments,
) -> ToolResult<WorkspaceTemplateSaveValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_USE_TEMPLATES);
    }
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut transaction = state.pool.begin().await?;
        let caller = roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
        if !templates::may_use(caller) {
            return Ok(ToolResult::denied(MANAGERS_USE_TEMPLATES));
        }
        let auditor = Auditor::App(principal);
        if let Some(reference) = arguments.remove.as_deref() {
            if arguments.name.is_some() {
                return Ok(ToolResult::invalid(
                    "remove",
                    "Save one template or remove one, not both at once.",
                ));
            }
            let found = templates::find(&mut transaction, organization_id, reference).await?;
            let Some(template_id) = found
                .filter(|template| !template.built_in)
                .and_then(|template| Uuid::parse_str(&template.id).ok())
            else {
                return Ok(ToolResult::not_found("That saved template"));
            };
            templates::remove(&mut transaction, &auditor, organization_id, template_id).await?;
            transaction.commit().await?;
            return Ok(ToolResult::ok(WorkspaceTemplateSaveValue {
                done: "removed".to_owned(),
                template: None,
            }));
        }
        let Some(name) = arguments
            .name
            .as_deref()
            .and_then(templates::normalize_name)
        else {
            return Ok(ToolResult::invalid(
                "name",
                "Give the template a name of 1 to 60 characters.",
            ));
        };
        let Some(description) =
            templates::normalize_description(arguments.description.as_deref().unwrap_or(""))
        else {
            return Ok(ToolResult::invalid(
                "description",
                "Keep the description to 300 characters.",
            ));
        };
        let project_id = arguments
            .include_project
            .unwrap_or(false)
            .then_some(principal.project_id);
        match templates::save(
            &mut transaction,
            &auditor,
            caller,
            organization_id,
            &name,
            &description,
            project_id,
        )
        .await?
        {
            SaveOutcome::Saved(template) => {
                transaction.commit().await?;
                Ok(ToolResult::ok(WorkspaceTemplateSaveValue {
                    done: "saved".to_owned(),
                    template: Some(template),
                }))
            }
            SaveOutcome::NameTaken => Ok(ToolResult::invalid(
                "name",
                "A template already has that name. Pick another.",
            )),
            SaveOutcome::Full => Ok(ToolResult::conflict(format!(
                "This workspace has {} saved templates. Remove one first.",
                templates::MAX_SAVED
            ))),
        }
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "workspace_template_save"))
}

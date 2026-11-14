//! AWD-111: bug_report_link_list and bug_report_link_write, a project's
//! bug report links (a URL a game or site posts reports to, each one a new
//! task), through MCP as the project's settings do on the web. Everything
//! goes through `crate::bug_reports`, which audits each change.

use rmcp::schemars;
use serde::Deserialize;
use serde_json::Value;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
};
use uuid::Uuid;

use crate::{
    AppState,
    bug_reports::{self, CreateLink, UpdateLink},
    collection_forms::{FormConfig, FormField},
    error::AppError,
    events::Caller,
};

const PEOPLE_ONLY: &str = "Only people make and change bug report links: anyone who can add tasks here makes one, and its maker or the project's owners and admins change it, on the web or through an app connected as themselves.";

fn to_value(value: impl serde::Serialize) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| AppError::Internal("serialize tool value"))
}

/// Whether Tasks is on in the project for the caller. These tools also
/// serve forms where only Collections is on; task links stay out of reach
/// there.
async fn tasks_on(state: &AppState, principal: &McpPrincipal) -> Result<bool, AppError> {
    let mut connection = state.pool.acquire().await?;
    Ok(crate::modules::state::for_project(
        &mut connection,
        principal.project_id,
        principal.actor_id,
    )
    .await?
    .is_none_or(|modules| modules.contains("tasks")))
}

const TASKS_OFF: &str = "Tasks is turned off here, so its bug report links are too: only forms (links with a collection) can be made or changed.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BugReportLinkListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
}

pub(super) async fn bug_report_link_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: BugReportLinkListArguments,
) -> ToolResult<Value> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let caller = Caller::mcp(principal);
    let listed = async {
        let mut list = bug_reports::list(state, &caller, principal.project_id).await?;
        if !tasks_on(state, principal).await? {
            list.links.retain(|link| link.collection_id.is_some());
        }
        to_value(list)
    };
    match listed.await {
        Ok(value) => ToolResult::ok(value),
        Err(error) => ToolResult::from_error(error, "bug_report_link_list"),
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BugReportLinkWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// A link to change, by ID (bug_report_link_list). Leave it out to make
    /// one; its URL is shown once.
    #[serde(default)]
    pub link_id: Option<String>,
    /// Its name, up to 80 characters: required when making one.
    #[serde(default)]
    pub name: Option<String>,
    /// "active" or "paused". Turning a link back on makes you its maker.
    #[serde(default)]
    pub state: Option<String>,
    /// Give the link a new URL; the old one stops working. The new URL is
    /// shown once.
    #[serde(default)]
    pub replace: Option<bool>,
    /// Delete the link; its URL stops working and its tasks stay.
    #[serde(default)]
    pub delete: Option<bool>,
    /// Make a form instead (AWD-110): a link whose answers become records
    /// in this collection (by name or ID), filled in by anyone with its
    /// formUrl. Needs "Edit records" here.
    #[serde(default)]
    pub collection: Option<String>,
    /// A form's settings: making one, or changing one's. Each field is the
    /// collection's, by name or ID; people and links can't be asked.
    #[serde(default)]
    pub form: Option<FormWrite>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FormWrite {
    /// Shown above the fields: what the form is for.
    #[serde(default)]
    pub intro: Option<String>,
    /// The fields it asks for, in order, after the title.
    #[serde(default)]
    pub fields: Vec<FormFieldWrite>,
    /// Shown once an answer is in.
    #[serde(default)]
    pub thanks: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FormFieldWrite {
    /// The field, by name or ID.
    pub field: String,
    #[serde(default)]
    pub required: bool,
}

/// A form as written, by the collection's field names, as it's stored.
async fn form_from_written(
    state: &AppState,
    project_id: Uuid,
    collection_id: Uuid,
    written: &FormWrite,
) -> Result<Result<FormConfig, String>, AppError> {
    let definitions = crate::collections::field_definitions(
        &mut *state.pool.acquire().await?,
        project_id,
        collection_id,
        false,
    )
    .await?;
    let mut fields = Vec::with_capacity(written.fields.len());
    for asked in &written.fields {
        let Some(field) = crate::fields::find(&definitions, &asked.field) else {
            return Ok(Err(format!(
                "The collection has no field called \"{}\". Its fields: {}.",
                asked.field.chars().take(80).collect::<String>(),
                crate::fields::active_names(&definitions)
            )));
        };
        fields.push(FormField {
            field: field.id,
            required: asked.required,
        });
    }
    Ok(Ok(FormConfig {
        intro: written.intro.clone().unwrap_or_default(),
        fields,
        thanks: written.thanks.clone().unwrap_or_default(),
    }))
}

pub(super) async fn bug_report_link_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: BugReportLinkWriteArguments,
) -> ToolResult<Value> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != crate::policy::ActorKind::Human {
        return ToolResult::denied(PEOPLE_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let replace = arguments.replace == Some(true);
    let delete = arguments.delete == Some(true);
    let edits = arguments.name.is_some() || arguments.state.is_some() || arguments.form.is_some();
    if usize::from(replace) + usize::from(delete) + usize::from(edits) > 1 {
        return ToolResult::invalid(
            "linkId",
            "Change a link's name or state, replace its URL, or delete it: one at a time.",
        );
    }
    let caller = Caller::mcp(principal);
    let project_id = principal.project_id;
    let result = async {
        let Some(link) = arguments.link_id.as_deref() else {
            if replace || delete || arguments.state.is_some() {
                return Ok(ToolResult::invalid(
                    "linkId",
                    "Name the link to change with linkId.",
                ));
            }
            let Some(name) = arguments.name.clone() else {
                return Ok(ToolResult::invalid("name", "A new link needs a name."));
            };
            let (collection_id, form) = match arguments.collection.as_deref() {
                None if arguments.form.is_some() => {
                    return Ok(ToolResult::invalid(
                        "collection",
                        "A form adds records to a collection: name it with collection.",
                    ));
                }
                None if !tasks_on(state, principal).await? => {
                    return Ok(ToolResult::conflict(TASKS_OFF));
                }
                None => (None, None),
                Some(reference) => {
                    let Some(collection) =
                        crate::collections::find(&state.pool, project_id, reference).await?
                    else {
                        return Ok(ToolResult::not_found("That collection"));
                    };
                    let written = arguments.form.clone().unwrap_or(FormWrite {
                        intro: None,
                        fields: Vec::new(),
                        thanks: None,
                    });
                    let form = match form_from_written(state, project_id, collection.id, &written)
                        .await?
                    {
                        Ok(form) => form,
                        Err(message) => return Ok(ToolResult::invalid("form", message)),
                    };
                    if let Some(message) = bug_reports::form_problem(
                        &state.pool,
                        project_id,
                        Some(collection.id),
                        Some(&form),
                    )
                    .await?
                    {
                        return Ok(ToolResult::invalid("form", message));
                    }
                    (Some(collection.id), Some(form))
                }
            };
            let made = bug_reports::create(
                state,
                &caller,
                project_id,
                CreateLink {
                    name,
                    collection_id,
                    form,
                },
            )
            .await?;
            return Ok(ToolResult::ok(to_value(made)?));
        };
        let Some(link_id) = parse_id(link) else {
            return Ok(ToolResult::invalid("linkId", "Use the link's ID."));
        };
        // Only links in the project this call names.
        if bug_reports::link_project(&state.pool, link_id).await? != Some(project_id) {
            return Ok(ToolResult::not_found("That bug report link"));
        }
        if bug_reports::link_collection(&state.pool, link_id)
            .await?
            .is_none()
            && !tasks_on(state, principal).await?
        {
            return Ok(ToolResult::conflict(TASKS_OFF));
        }
        let value = if delete {
            bug_reports::delete(state, &caller, link_id).await?;
            serde_json::json!({ "deleted": true })
        } else if replace {
            to_value(bug_reports::replace(state, &caller, link_id).await?)?
        } else if edits {
            let form = match &arguments.form {
                None => None,
                Some(written) => {
                    let Some(collection_id) =
                        bug_reports::link_collection(&state.pool, link_id).await?
                    else {
                        return Ok(ToolResult::invalid(
                            "form",
                            "That link makes tasks, not records: only a form has form settings.",
                        ));
                    };
                    let form =
                        match form_from_written(state, project_id, collection_id, written).await? {
                            Ok(form) => form,
                            Err(message) => return Ok(ToolResult::invalid("form", message)),
                        };
                    if let Some(message) = bug_reports::form_problem(
                        &state.pool,
                        project_id,
                        Some(collection_id),
                        Some(&form),
                    )
                    .await?
                    {
                        return Ok(ToolResult::invalid("form", message));
                    }
                    Some(form)
                }
            };
            to_value(
                bug_reports::update(
                    state,
                    &caller,
                    link_id,
                    UpdateLink {
                        name: arguments.name.clone(),
                        state: arguments.state.clone(),
                        form,
                    },
                )
                .await?,
            )?
        } else {
            return Ok(ToolResult::invalid(
                "linkId",
                "Give a name or state, replace: true, or delete: true.",
            ));
        };
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(PEOPLE_ONLY),
        Err(AppError::NotFound("bug_report_link")) => ToolResult::not_found("That bug report link"),
        Err(AppError::Validation("bug_report_links.limit")) => ToolResult::conflict(
            "This project has as many bug report links as it can; delete one first.",
        ),
        Err(error) => ToolResult::from_error(error, "bug_report_link_write"),
    }
}

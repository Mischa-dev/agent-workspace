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
use crate::{
    AppState,
    bug_reports::{self, CreateLink, UpdateLink},
    error::AppError,
    events::Caller,
};

const PEOPLE_ONLY: &str = "Only people make and change bug report links: anyone who can add tasks here makes one, and its maker or the project's owners and admins change it, on the web or through an app connected as themselves.";

fn to_value(value: impl serde::Serialize) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| AppError::Internal("serialize tool value"))
}

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
    match bug_reports::list(state, &caller, principal.project_id)
        .await
        .and_then(to_value)
    {
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
    let edits = arguments.name.is_some() || arguments.state.is_some();
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
            let made = bug_reports::create(state, &caller, project_id, CreateLink { name }).await?;
            return Ok(ToolResult::ok(to_value(made)?));
        };
        let Some(link_id) = parse_id(link) else {
            return Ok(ToolResult::invalid("linkId", "Use the link's ID."));
        };
        // Only links in the project this call names.
        if bug_reports::link_project(&state.pool, link_id).await? != Some(project_id) {
            return Ok(ToolResult::not_found("That bug report link"));
        }
        let value = if delete {
            bug_reports::delete(state, &caller, link_id).await?;
            serde_json::json!({ "deleted": true })
        } else if replace {
            to_value(bug_reports::replace(state, &caller, link_id).await?)?
        } else if edits {
            to_value(
                bug_reports::update(
                    state,
                    &caller,
                    link_id,
                    UpdateLink {
                        name: arguments.name.clone(),
                        state: arguments.state.clone(),
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

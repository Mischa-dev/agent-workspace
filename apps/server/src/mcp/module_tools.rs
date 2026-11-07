//! AWD-75: module_list, which modules are on in a workspace and what each
//! does; and thing_open, any thing with its links read both ways (the
//! shared contract's "describe").

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, accessible_project_ids, parse_id},
};
use crate::{
    AppState,
    error::AppError,
    modules::{
        self,
        contract::{EmbedDecl, Thing, ThingDecl, ThingLink},
        manifest, state,
    },
};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ModuleListArguments {
    /// A project's ID, link or key (such as "AWD"): its workspace's modules.
    /// Left out, the workspace of the project this connection is in.
    #[serde(default)]
    pub project_id: Option<String>,
    /// One module's id (such as "tasks") for everything it declares: its
    /// kinds of thing with their fields, and the live blocks it offers.
    #[serde(default)]
    pub module: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModuleDetail {
    pub things: Vec<ThingDecl>,
    pub embeds: Vec<EmbedDecl>,
    /// Permission keys it owns.
    pub permissions: Vec<String>,
    /// Events it follows.
    pub listens: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModuleListValue {
    #[schemars(with = "String")]
    pub workspace_id: Uuid,
    pub modules: Vec<modules::http::ModuleView>,
    /// The preset the workspace's switches match, such as
    /// "chat_with_agents", if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<ModuleDetail>,
}

/// The workspace of the project a call names, or of the connection's own
/// project, if the caller can open it.
async fn workspace_of(
    state: &AppState,
    principal: &McpPrincipal,
    project: Option<&str>,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    let project_id = match project {
        Some(project) => match parse_id(project) {
            Some(id) => id,
            None => return Ok(None),
        },
        None => principal.project_id,
    };
    if !accessible_project_ids(state, principal)
        .await?
        .contains(&project_id)
    {
        return Ok(None);
    }
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&state.pool)
            .await?
            .map(|organization_id| (organization_id, project_id)),
    )
}

pub(super) async fn module_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ModuleListArguments,
) -> ToolResult<ModuleListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let detail_of = match arguments.module.as_deref() {
        Some(id) => match manifest::get(id) {
            Some(module) => Some(module),
            None => {
                let known = manifest::core()
                    .iter()
                    .map(|module| module.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return ToolResult::invalid(
                    "module",
                    format!("There's no module {id}. Use one of {known}."),
                );
            }
        },
        None => None,
    };
    let result = async {
        let Some((organization_id, project_id)) =
            workspace_of(state, principal, arguments.project_id.as_deref()).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut connection = state.pool.acquire().await?;
        let switches = state::switches(&mut connection, organization_id).await?;
        // In a project the caller names, their role there narrows the
        // workspace's switches, as tool calls there are checked; otherwise
        // their workspace role does.
        let set = if arguments.project_id.is_some() {
            state::for_project(&mut connection, project_id, principal.actor_id)
                .await?
                .unwrap_or_default()
        } else {
            state::for_member(&mut connection, organization_id, principal.actor_id).await?
        };
        Ok::<_, AppError>(ToolResult::ok(ModuleListValue {
            workspace_id: organization_id,
            modules: manifest::core()
                .iter()
                .map(|module| modules::http::ModuleView::of(module, set, &switches))
                .collect(),
            preset: modules::http::matching_preset(&switches),
            detail: detail_of.map(|module| ModuleDetail {
                things: module.things.clone(),
                embeds: module.embeds.clone(),
                permissions: module.permissions.clone(),
                listens: module.events.listens.clone(),
            }),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "module_list"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThingOpenArguments {
    /// What kind of thing: task, page, decision, whiteboard, thread,
    /// channel, conversation or project.
    pub kind: String,
    /// Its ID or link.
    pub id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThingOpenValue {
    pub thing: Thing,
    /// Its links both ways, such as "Blocked by" or "Referenced in", to
    /// things you can see.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ThingLink>,
}

pub(super) async fn thing_open(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ThingOpenArguments,
) -> ToolResult<ThingOpenValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let kind = arguments.kind.trim().to_ascii_lowercase();
    if !modules::things::KINDS.contains(&kind.as_str()) {
        return ToolResult::invalid(
            "kind",
            format!("Use one of {}.", modules::things::KINDS.join(", ")),
        );
    }
    let Some(id) = parse_id(&arguments.id) else {
        return ToolResult::invalid("id", "Pass the thing's ID or link.");
    };
    let result = async {
        let projects = accessible_project_ids(state, principal).await?;
        let task_projects =
            crate::search::projects_showing_tasks(&state.pool, principal.actor_id, &projects)
                .await?;
        let mut connection = state.pool.acquire().await?;
        let anywhere = state::anywhere(&mut connection, principal.actor_id).await?;
        let workspaces = state::by_workspace(&mut connection, principal.actor_id).await?;
        let viewer = modules::things::Viewer {
            actor_id: principal.actor_id,
            projects: &projects,
            task_projects: &task_projects,
            modules: anywhere,
            workspaces: &workspaces,
            // An older credential confined to one project reaches no board
            // across a workspace, as whiteboards::access has it.
            workspace_boards: super::common::confined_project(principal).is_none(),
        };
        let Some(thing) = modules::things::get(&mut connection, &viewer, &kind, id).await? else {
            return Ok(ToolResult::not_found("That thing"));
        };
        let modules = match thing.project_id {
            Some(project_id) => state::for_project(&mut connection, project_id, principal.actor_id)
                .await?
                .unwrap_or_default(),
            None => anywhere,
        };
        // The thing and its links follow its own project's modules (the
        // caller's role there narrowing them), as `shown` reads them: a
        // conversation by its project's kind, so a DM thread follows Messages.
        let viewer = modules::things::Viewer { modules, ..viewer };
        let Some(thing) = modules::things::get(&mut connection, &viewer, &kind, id).await? else {
            return Ok(ToolResult::not_found("That thing"));
        };
        let links = modules::things::links(&mut connection, &viewer, &thing).await?;
        Ok::<_, AppError>(ToolResult::ok(ThingOpenValue { thing, links }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "thing_open"))
}

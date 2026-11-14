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
    /// What kind of thing: task, page, decision, whiteboard, sheet, thread,
    /// channel, conversation, project, collection or record (a
    /// collection's). Leave it out to open any thing by its ID.
    pub kind: Option<String>,
    /// Its ID, link, or thing:<id>.
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

/// The record a collection link opens (`…/collections/<id>?record=<id>`).
fn record_in_link(value: &str) -> Option<Uuid> {
    let (_, query) = value.trim().split_once('?')?;
    query
        .split(['&', '#'])
        .find_map(|pair| pair.strip_prefix("record="))
        .and_then(|id| id.parse::<Uuid>().ok())
}

pub(super) async fn thing_open(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ThingOpenArguments,
) -> ToolResult<ThingOpenValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let kind = arguments
        .kind
        .as_deref()
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty());
    if let Some(kind) = &kind
        && !modules::things::KINDS.contains(&kind.as_str())
    {
        return ToolResult::invalid(
            "kind",
            format!("Use one of {}.", modules::things::KINDS.join(", ")),
        );
    }
    let raw = arguments.id.trim();
    let raw = raw.strip_prefix("thing:").unwrap_or(raw);
    // A record's link is its collection's, with ?record=<id> (AWD-110).
    let id = record_in_link(raw).or_else(|| parse_id(raw));
    let Some(id) = id else {
        return ToolResult::invalid("id", "Pass the thing's ID or link.");
    };
    let result = async {
        // Without a kind, the registry says what it is.
        let kind = match kind {
            Some(kind) => kind,
            None => {
                let mut connection = state.pool.acquire().await?;
                match crate::things::locate(&mut connection, id).await? {
                    Some(located) if !located.deleted => located.kind.to_string(),
                    _ => return Ok(ToolResult::not_found("That thing")),
                }
            }
        };
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

// ---------------------------------------------------------------------------
// workspace_update (AWD-111)

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkspaceUpdateArguments {
    /// A project in the workspace: its ID, link or key (such as "AWD").
    pub project_id: String,
    /// A new name for the workspace.
    #[serde(default)]
    pub name: Option<String>,
    /// The workspace's version you last read, for a rename. Defaults to the
    /// current one.
    #[serde(default)]
    pub expected_version: Option<i64>,
    /// A module preset to start from (as in the workspace's settings), then
    /// `modules` on top.
    #[serde(default)]
    pub preset: Option<String>,
    /// Switch modules on (true) or off (false), by id (module_list). Off
    /// hides a module everywhere in the workspace without deleting anything.
    #[serde(default)]
    pub modules: Option<std::collections::BTreeMap<String, bool>>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkspaceUpdateValue {
    pub name: String,
    pub version: i64,
    /// Whether the name changed.
    pub renamed: bool,
    /// The modules this call switched, by id: true for on.
    pub switched: std::collections::BTreeMap<String, bool>,
    /// The modules now off in the workspace, by id.
    pub modules_off: Vec<String>,
}

const MANAGERS_ONLY: &str = "Only the workspace's owners and admins who manage its settings change it, as people, on the web or through an app connected as themselves.";

pub(super) async fn workspace_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WorkspaceUpdateArguments,
) -> ToolResult<WorkspaceUpdateValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != crate::policy::ActorKind::Human {
        return ToolResult::denied(MANAGERS_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let switches = arguments.modules.clone().unwrap_or_default();
    if arguments.name.is_none() && arguments.preset.is_none() && switches.is_empty() {
        return ToolResult::invalid("name", "Give a new name, a preset, or modules to switch.");
    }
    if arguments.name.is_none() && arguments.expected_version.is_some() {
        return ToolResult::invalid("expectedVersion", "expectedVersion goes with a rename.");
    }
    let project_id = principal.project_id;
    let result = async {
        if super::common::project_role(state, principal, project_id)
            .await?
            .is_none()
        {
            return Ok(ToolResult::not_found("That project"));
        }
        let mut transaction = state.pool.begin().await?;
        super::common::mark_via(&mut transaction, principal).await?;
        let organization_id =
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_one(&mut *transaction)
                .await?;
        let mut renamed = false;
        if let Some(name) = arguments.name.as_deref() {
            let current =
                sqlx::query_scalar::<_, i64>("SELECT version FROM organizations WHERE id = $1")
                    .bind(organization_id)
                    .fetch_one(&mut *transaction)
                    .await?;
            let (_, metadata) = crate::workspace::rename_workspace_in(
                &mut transaction,
                principal.actor_id,
                organization_id,
                name,
                arguments.expected_version.unwrap_or(current),
            )
            .await?;
            if let Some(mut metadata) = metadata {
                metadata["organizationId"] = serde_json::json!(organization_id);
                super::common::insert_audit(
                    &mut transaction,
                    principal,
                    project_id,
                    "workspace.renamed",
                    "workspace",
                    organization_id,
                    metadata,
                )
                .await?;
                renamed = true;
            }
        }
        let mut switched = std::collections::BTreeMap::new();
        if arguments.preset.is_some() || !switches.is_empty() {
            let metadata = modules::http::change_modules_in(
                &mut transaction,
                principal.actor_id,
                organization_id,
                arguments.preset.as_deref(),
                &switches,
            )
            .await?;
            if let Some(metadata) = metadata {
                if let Some(changed) = metadata["changed"].as_object() {
                    for (module, on) in changed {
                        switched.insert(module.clone(), on.as_bool().unwrap_or(false));
                    }
                }
                super::common::insert_audit(
                    &mut transaction,
                    principal,
                    project_id,
                    "modules.changed",
                    "organization",
                    organization_id,
                    metadata,
                )
                .await?;
            }
        }
        let (name, version) = sqlx::query_as::<_, (String, i64)>(
            "SELECT name, version FROM organizations WHERE id = $1",
        )
        .bind(organization_id)
        .fetch_one(&mut *transaction)
        .await?;
        let off = state::switches(&mut transaction, organization_id).await?;
        let modules_off = modules::http::switched_off(&off);
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(WorkspaceUpdateValue {
            name,
            version,
            renamed,
            switched,
            modules_off,
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(MANAGERS_ONLY),
        Err(AppError::NotFound("workspace")) => ToolResult::not_found("That project"),
        Err(AppError::Validation("preset")) => {
            ToolResult::invalid("preset", "There's no module preset by that name.")
        }
        Err(AppError::Validation("module")) => ToolResult::invalid(
            "modules",
            "One of those isn't a module. module_list shows their ids.",
        ),
        Err(AppError::Validation("module can't be switched")) => {
            ToolResult::invalid("modules", "One of those modules is always on.")
        }
        Err(AppError::Validation(field)) if field.contains("name") => {
            ToolResult::invalid("name", "That name isn't valid.")
        }
        Err(AppError::VersionConflict {
            current_version, ..
        }) => ToolResult::conflict(format!(
            "The workspace changed since you read it; it's at version {current_version} now. Read it again and retry."
        )),
        Err(error) => ToolResult::from_error(error, "workspace_update"),
    }
}

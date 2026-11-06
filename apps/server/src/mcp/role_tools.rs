//! AWD-74: roles for agents and apps. Anyone can read the roles of a
//! workspace they're in and what they themselves may do; only a person
//! (through an app) who manages roles or members changes them, never an
//! agent acting as itself. Everything goes through `crate::roles`, the same
//! code the Roles page uses, and is audited the same way.

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
    roles::{self, Auditor, PermissionsView, RoleChange, RoleDefaults, RoleInput, RoleView},
};

const AGENTS_DONT_CHANGE_ROLES: &str = "Agents can't change roles. Ask a workspace owner or admin, who can do it on the Roles page or through an app connected as themselves.";

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

/// One role, as agents read it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RoleLine {
    pub id: String,
    /// owner, admin, member, viewer, or a custom role's key.
    pub key: String,
    pub name: String,
    pub description: String,
    pub built_in: bool,
    /// The built-in role a custom one starts from.
    pub base_role: String,
    pub permissions: Vec<String>,
    /// What apps and agents acting for holders may do; absent when it's the
    /// same as the holder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_limit: Option<Vec<String>>,
    /// Where holders start: layoutPreset, channels, pinnedViews, homeHidden.
    pub defaults: serde_json::Value,
    /// Modules holders don't get, on top of what the workspace has off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules_off: Vec<String>,
    pub is_default: bool,
    pub version: i64,
    pub holders: i64,
}

fn line(view: RoleView) -> RoleLine {
    RoleLine {
        id: view.id.to_string(),
        key: view.key,
        name: view.name,
        description: view.description,
        built_in: view.built_in,
        base_role: view.base_role,
        permissions: view
            .permissions
            .iter()
            .map(|key| (*key).to_owned())
            .collect(),
        agent_limit: view
            .agent_limit
            .map(|keys| keys.iter().map(|key| (*key).to_owned()).collect()),
        defaults: serde_json::to_value(&view.defaults).unwrap_or_default(),
        modules_off: view.modules_off,
        is_default: view.is_default,
        version: view.version,
        holders: view.workspace_holders + view.project_holders,
    }
}

// ---------------------------------------------------------------------------
// role_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RoleListArguments {
    /// A project in the workspace: its ID, link or key (such as "AWD").
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PermissionLine {
    pub key: String,
    pub module: String,
    pub label: String,
    pub meaning: String,
    /// Agents never get it, whatever their role says.
    pub people_only: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RoleListValue {
    pub roles: Vec<RoleLine>,
    /// Every permission a role can hold, by module.
    pub permissions: Vec<PermissionLine>,
}

pub(super) async fn role_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RoleListArguments,
) -> ToolResult<RoleListValue> {
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut connection = state.pool.acquire().await?;
        let caller = roles::standing(&mut connection, organization_id, principal.actor_id).await?;
        let roles = roles::list(&mut connection, caller, organization_id).await?;
        let (_, permissions) = roles::catalogue();
        Ok::<_, AppError>(ToolResult::ok(RoleListValue {
            roles: roles.into_iter().map(line).collect(),
            permissions: permissions
                .into_iter()
                .map(|entry| PermissionLine {
                    key: entry.key.to_owned(),
                    module: entry.module.to_owned(),
                    label: entry.label.to_owned(),
                    meaning: entry.meaning.to_owned(),
                    people_only: entry.people_only,
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "role_list"))
}

// ---------------------------------------------------------------------------
// role_get

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RoleGetArguments {
    /// A project in the workspace: its ID, link or key.
    pub project_id: String,
    /// The role's name, key (such as "member") or ID.
    pub role: String,
}

pub(super) async fn role_get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RoleGetArguments,
) -> ToolResult<RoleLine> {
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut connection = state.pool.acquire().await?;
        let caller = roles::standing(&mut connection, organization_id, principal.actor_id).await?;
        let Some(role_id) =
            roles::find_role(&mut connection, organization_id, &arguments.role).await?
        else {
            return Ok(ToolResult::not_found("That role"));
        };
        let view = roles::read(&mut connection, caller, organization_id, role_id).await?;
        Ok::<_, AppError>(ToolResult::ok(line(view)))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "role_get"))
}

// ---------------------------------------------------------------------------
// my_permissions

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MyPermissionsArguments {
    /// The project to check: its ID, link or key.
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScopeLine {
    pub role: String,
    pub role_name: String,
    /// What the role allows.
    pub permissions: Vec<String>,
    /// What this call may do: for a person's app, the role cut down by
    /// their agent limit.
    pub this_connection_may: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MyPermissionsValue {
    /// "person" for an app acting for a person, "agent" for an agent acting
    /// as itself.
    pub acting_as: String,
    pub workspace: ScopeLine,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ScopeLine>,
}

fn scope_line(scope: roles::ScopePermissions) -> ScopeLine {
    ScopeLine {
        role: scope.role,
        role_name: scope.role_name,
        permissions: scope
            .permissions
            .iter()
            .map(|key| (*key).to_owned())
            .collect(),
        this_connection_may: scope.for_apps.iter().map(|key| (*key).to_owned()).collect(),
    }
}

pub(super) async fn my_permissions(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MyPermissionsArguments,
) -> ToolResult<MyPermissionsValue> {
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut connection = state.pool.acquire().await?;
        let PermissionsView {
            workspace, project, ..
        } = roles::permissions_of(
            &mut connection,
            organization_id,
            principal.actor_id,
            Some(principal.project_id),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(MyPermissionsValue {
            acting_as: if principal.actor_kind == ActorKind::Human {
                "person"
            } else {
                "agent"
            }
            .to_owned(),
            workspace: scope_line(workspace),
            project: project.map(scope_line),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "my_permissions"))
}

// ---------------------------------------------------------------------------
// role_write

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RoleWriteArguments {
    /// A project in the workspace: its ID, link or key.
    pub project_id: String,
    /// The role to change or remove, by name, key or ID. Leave it out to
    /// make a new one.
    pub role: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// A new role's base: admin, member or viewer. It keeps only
    /// permissions its base has. Only owners make roles based on admin.
    pub base_role: Option<String>,
    /// Permission keys from role_list, such as "tasks.edit". Built-in roles
    /// keep theirs.
    pub permissions: Option<Vec<String>>,
    /// What apps and agents acting for holders may do, as permission keys.
    /// An empty list leaves them reading only; leave it out to keep it.
    pub agent_limit: Option<Vec<String>>,
    /// True takes the agent limit away (apps may do what holders may).
    pub clear_agent_limit: Option<bool>,
    /// Where holders start: {"layoutPreset", "channels", "pinnedViews",
    /// "homeHidden"}.
    pub defaults: Option<serde_json::Value>,
    /// Modules (ids from module_list, such as "runs") that holders don't
    /// get, on top of what the workspace has off. An empty list gives them
    /// every module the workspace has on; leave it out to keep it. Never for
    /// the owner role.
    pub modules_off: Option<Vec<String>>,
    /// True makes it the role new members and invitations start with.
    pub is_default: Option<bool>,
    /// True removes the role. Only custom roles nobody holds can go.
    pub remove: Option<bool>,
    /// The role's version from role_get, when changing one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RoleWriteValue {
    /// created, changed or removed.
    pub done: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<RoleLine>,
}

pub(super) async fn role_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RoleWriteArguments,
) -> ToolResult<RoleWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(AGENTS_DONT_CHANGE_ROLES);
    }
    let defaults = match arguments
        .defaults
        .clone()
        .map(serde_json::from_value::<RoleDefaults>)
    {
        None => None,
        Some(Ok(defaults)) => Some(defaults),
        Some(Err(_)) => {
            return ToolResult::invalid(
                "defaults",
                "Use layoutPreset (a preset id), channels (names), pinnedViews (my_tasks, ready_for_agents, blocked, recently_changed) and homeHidden (part ids).",
            );
        }
    };
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut transaction = state.pool.begin().await?;
        let caller = roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
        let auditor = Auditor::App(principal);
        let existing = match arguments.role.as_deref() {
            None => None,
            Some(text) => match roles::find_role(&mut transaction, organization_id, text).await? {
                Some(id) => Some(id),
                None => return Ok(ToolResult::not_found("That role")),
            },
        };
        let value = match existing {
            None => {
                if arguments.remove == Some(true) {
                    return Ok(ToolResult::invalid("role", "Name the role to remove."));
                }
                let (Some(name), Some(base_role), Some(permissions)) = (
                    arguments.name.clone(),
                    arguments.base_role.clone(),
                    arguments.permissions.clone(),
                ) else {
                    return Ok(ToolResult::invalid(
                        "name",
                        "A new role needs name, baseRole and permissions.",
                    ));
                };
                let view = roles::create(
                    &mut transaction,
                    &auditor,
                    caller,
                    organization_id,
                    RoleInput {
                        name,
                        description: arguments.description.clone().unwrap_or_default(),
                        base_role,
                        permissions,
                        agent_limit: arguments.agent_limit.clone(),
                        defaults: defaults.unwrap_or_default(),
                        modules_off: arguments.modules_off.clone().unwrap_or_default(),
                        is_default: arguments.is_default.unwrap_or(false),
                    },
                )
                .await?;
                RoleWriteValue {
                    done: "created".to_owned(),
                    role: Some(line(view)),
                }
            }
            Some(role_id) if arguments.remove == Some(true) => {
                roles::remove(&mut transaction, &auditor, caller, organization_id, role_id).await?;
                RoleWriteValue {
                    done: "removed".to_owned(),
                    role: None,
                }
            }
            Some(role_id) => {
                let agent_limit = if arguments.clear_agent_limit == Some(true) {
                    Some(None)
                } else {
                    arguments.agent_limit.clone().map(Some)
                };
                let view = roles::change(
                    &mut transaction,
                    &auditor,
                    caller,
                    organization_id,
                    role_id,
                    RoleChange {
                        expected_version: arguments.expected_version,
                        name: arguments.name.clone(),
                        description: arguments.description.clone(),
                        permissions: arguments.permissions.clone(),
                        agent_limit,
                        defaults,
                        modules_off: arguments.modules_off.clone(),
                        is_default: arguments.is_default,
                    },
                )
                .await?;
                RoleWriteValue {
                    done: "changed".to_owned(),
                    role: Some(line(view)),
                }
            }
        };
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| match error {
        AppError::Forbidden => ToolResult::denied(
            "You can't make that change. Owners and admins manage roles; only owners manage roles based on admin, and nobody gives a role more than they hold.",
        ),
        error => ToolResult::from_error(error, "role_write"),
    })
}

// ---------------------------------------------------------------------------
// role_assign

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RoleAssignArguments {
    /// A project in the workspace: its ID, link or key.
    pub project_id: String,
    /// Who: an @handle or actor ID of someone in the workspace.
    pub person: String,
    /// The workspace role to give them, by name, key or ID. For a role in
    /// one project only, use project_member_set.
    pub role: String,
    /// True gives them the role in each of their projects too (where they
    /// aren't the owner).
    pub apply_to_projects: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct AssignmentLine {
    pub actor_id: String,
    /// The role they hold in the workspace now.
    pub workspace_role: String,
    /// Projects where their role changed with it.
    pub projects_changed: usize,
    /// Projects left as they were: they own it, it takes an owner, or you
    /// may not manage members there.
    pub projects_skipped: usize,
    /// Each project left as it was, and why.
    pub skipped_projects: Vec<SkippedProjectLine>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkippedProjectLine {
    pub project_id: String,
    pub reason: String,
}

pub(super) async fn role_assign(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RoleAssignArguments,
) -> ToolResult<AssignmentLine> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(AGENTS_DONT_CHANGE_ROLES);
    }
    let result = async {
        let Some(organization_id) = workspace_of(state, principal, &arguments.project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut transaction = state.pool.begin().await?;
        let caller = roles::standing(&mut transaction, organization_id, principal.actor_id).await?;
        let person = arguments.person.trim();
        let Some(actor_id) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT profile.actor_id FROM organization_actor_profiles profile
            WHERE profile.organization_id = $1
              AND (profile.actor_id::text = $2 OR lower(profile.handle) = lower($3))
            LIMIT 1
            "#,
        )
        .bind(organization_id)
        .bind(person)
        .bind(person.trim_start_matches('@'))
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(ToolResult::not_found(&format!(
                "{person} in your workspace"
            )));
        };
        let Some(role_id) =
            roles::find_role(&mut transaction, organization_id, &arguments.role).await?
        else {
            return Ok(ToolResult::not_found("That role"));
        };
        let assignment = roles::assign(
            &mut transaction,
            &Auditor::App(principal),
            caller,
            organization_id,
            actor_id,
            role_id,
            arguments.apply_to_projects.unwrap_or(false),
            None,
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(AssignmentLine {
            actor_id: assignment.actor_id.to_string(),
            workspace_role: assignment.role_name,
            projects_changed: assignment.projects_changed,
            projects_skipped: assignment.projects_skipped,
            skipped_projects: assignment
                .skipped_projects
                .iter()
                .map(|skipped| SkippedProjectLine {
                    project_id: skipped.project_id.to_string(),
                    reason: skipped.reason.to_owned(),
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| match error {
        AppError::Forbidden => ToolResult::denied(
            "You can't give that role. Owners and admins assign roles; only owners change an admin's role or give one based on admin, and nobody gives a role that holds more than they do.",
        ),
        error => ToolResult::from_error(error, "role_assign"),
    })
}

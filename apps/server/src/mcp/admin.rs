//! Project admin: a project's workflow and members, renaming, and archiving
//! or restoring it, and creating one. Changing a project is for people with an
//! owner or admin role; agents never create or manage projects (docs/plans/mcp-tools.md, decision 5).
//! workflow_update sends the whole workflow through `replace_workflow`, as
//! the web settings page does.

use std::collections::{BTreeMap, HashMap};

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    McpPrincipal,
    briefing::LayoutLine,
    common::{StoredRole, ToolResult, insert_audit, parse_id, project_role, role_allows},
    tasks::command_actor,
    workflow::Workflow,
};
use crate::{
    AppState,
    error::AppError,
    layouts::{self, Layout},
    memory,
    policy::Action,
    settings::Target,
    work_item_commands::lock_membership,
    work_item_workflow::{
        self, AllTransitions, StatusDraft, TransitionDraft, TransitionsDraft, TypeDraft,
        WorkflowDocument, WorkflowRefusal,
    },
};

/// What changing a project takes: a role holding the permission (K1.13:
/// the role decides, never the kind of account).
const ADMIN_ONLY: &str = "Your role here doesn't allow that. Ask someone who manages this project.";

// ---------------------------------------------------------------------------
// workflow_get

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkflowArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowType {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowStatus {
    pub id: String,
    pub name: String,
    /// backlog, unstarted, started, review, verification, completed, or
    /// canceled. task_list filters by these too.
    pub category: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowTransition {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowValue {
    pub types: Vec<WorkflowType>,
    /// In board order, left to right.
    pub statuses: Vec<WorkflowStatus>,
    /// The status changes allowed, by status name.
    pub transitions: Vec<WorkflowTransition>,
    /// Pass as expectedVersion to workflow_update.
    #[serde(default)]
    pub version: i64,
    /// What task estimates count: points or hours.
    #[serde(default)]
    pub estimate_unit: String,
}

/// The workflow as workflow_get shows it.
async fn workflow_value(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
) -> Result<WorkflowValue, AppError> {
    let workflow = Workflow::load(connection, project_id).await?;
    let (version, estimate_unit) =
        work_item_workflow::task_settings(connection, project_id).await?;
    let types = workflow
        .types
        .iter()
        .map(|kind| WorkflowType {
            id: kind.id.to_string(),
            name: kind.name.clone(),
            is_default: kind.is_default,
        })
        .collect();
    let statuses = workflow
        .statuses
        .iter()
        .filter(|status| status.enabled)
        .map(|status| WorkflowStatus {
            id: status.id.to_string(),
            name: status.name.clone(),
            category: status.category.clone(),
            is_default: status.is_default,
        })
        .collect();
    let enabled = |id: Uuid| {
        workflow
            .statuses
            .iter()
            .any(|status| status.id == id && status.enabled)
    };
    let transitions = workflow
        .transitions
        .iter()
        .filter(|(from, to)| enabled(*from) && enabled(*to))
        .map(|(from, to)| WorkflowTransition {
            from: workflow.status_name(*from).to_owned(),
            to: workflow.status_name(*to).to_owned(),
        })
        .collect();
    Ok(WorkflowValue {
        types,
        statuses,
        transitions,
        version,
        estimate_unit,
    })
}

pub(super) async fn workflow_get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WorkflowArguments,
) -> ToolResult<WorkflowValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        memory::project_access(
            &state.pool,
            principal.actor_id,
            principal.project_id,
            Action::WorkItemView,
        )
        .await?;
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        Ok::<_, AppError>(ToolResult::ok(
            workflow_value(&mut connection, principal.project_id).await?,
        ))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "workflow_get"))
}

// ---------------------------------------------------------------------------
// workflow_update

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkflowUpdateArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The version from workflow_get. If someone changed the workflow since,
    /// nothing changes.
    pub expected_version: i64,
    /// points or hours. Left out, it stays as it is.
    pub estimate_unit: Option<String>,
    /// Every status the workflow should have, in board order. Keep a status
    /// by passing its id (you can rename it or change its category); leave
    /// it out to retire it. A new one has no id.
    pub statuses: Vec<WorkflowStatusDraft>,
    /// Every task type the workflow should have, in order, the same way.
    pub types: Vec<WorkflowTypeDraft>,
    /// true allows any status change.
    pub all_transitions: Option<bool>,
    /// The status changes allowed, each from and to a status by name, id or
    /// key. Left out (without allTransitions), the current ones between
    /// statuses that stay are kept.
    pub transitions: Option<Vec<WorkflowTransitionDraft>>,
    /// Where the open tasks of a retired status or type go: its name or id
    /// to a status or type that stays (name, id or key). Each moved task gets
    /// an entry in its history.
    pub replacements: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkflowStatusDraft {
    /// An existing status's id. Leave it out for a new status.
    pub id: Option<String>,
    /// A short name for a new status, for transitions and replacements to
    /// use before it has an id.
    pub key: Option<String>,
    pub name: String,
    /// backlog, unstarted, started, review, verification, completed, or
    /// canceled.
    #[serde(alias = "semanticCategory")]
    pub category: String,
    /// Where new tasks start. One status is the default.
    pub is_default: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkflowTypeDraft {
    /// An existing type's id. Leave it out for a new type.
    pub id: Option<String>,
    pub key: Option<String>,
    pub name: String,
    pub is_default: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkflowTransitionDraft {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowUpdateValue {
    pub statuses_added: u32,
    pub statuses_retired: u32,
    pub types_added: u32,
    pub types_retired: u32,
    /// Open tasks moved off retired statuses and types.
    pub tasks_moved: u32,
    /// The workflow now, with its new version.
    pub workflow: WorkflowValue,
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(super) async fn workflow_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WorkflowUpdateArguments,
) -> ToolResult<WorkflowUpdateValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    if arguments.all_transitions == Some(true) && arguments.transitions.is_some() {
        return ToolResult::invalid(
            "transitions",
            "Pass allTransitions or transitions, not both.",
        );
    }
    let project_id = principal.project_id;
    let mut statuses = Vec::with_capacity(arguments.statuses.len());
    for status in &arguments.statuses {
        let id = match status.id.as_deref() {
            Some(id) => match parse_id(id) {
                Some(id) => Some(id),
                None => {
                    return ToolResult::invalid(
                        "statuses",
                        format!(
                            "{} has an id that isn't one (see workflow_get).",
                            status.name
                        ),
                    );
                }
            },
            None => None,
        };
        statuses.push(StatusDraft {
            id,
            key: status.key.clone(),
            name: status.name.clone(),
            semantic_category: status.category.trim().to_lowercase(),
            is_default: status.is_default == Some(true),
        });
    }
    let mut types = Vec::with_capacity(arguments.types.len());
    for kind in &arguments.types {
        let id = match kind.id.as_deref() {
            Some(id) => match parse_id(id) {
                Some(id) => Some(id),
                None => {
                    return ToolResult::invalid(
                        "types",
                        format!("{} has an id that isn't one (see workflow_get).", kind.name),
                    );
                }
            },
            None => None,
        };
        types.push(TypeDraft {
            id,
            key: kind.key.clone(),
            name: kind.name.clone(),
            is_default: kind.is_default == Some(true),
        });
    }
    let result = async {
        // The caller must reach the project through this connection, and be
        // an owner or admin there, checked again under the project lock.
        if project_role(state, principal, project_id).await?.is_none() {
            return Ok(ToolResult::not_found("That project"));
        }
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        super::common::mark_via(&mut transaction, principal).await?;
        let Some(membership) =
            lock_membership(&mut transaction, principal.actor_id, project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !membership.enabled {
            return Ok(ToolResult::not_found("That project"));
        }
        if !role_allows(
            principal,
            project_id,
            &StoredRole::of(&membership.role, membership.role_permissions.as_deref())
                .with_groups(membership.group_permissions.clone()),
            Action::WorkflowManage,
        ) {
            return Ok(ToolResult::denied(ADMIN_ONLY));
        }
        if membership.project_archived_at.is_some() {
            return Ok(ToolResult::conflict(
                "That project is archived. Restore it with project_archive (archived: false) first.",
            ));
        }
        let transitions = if arguments.all_transitions == Some(true) {
            TransitionsDraft::All(AllTransitions::All)
        } else if let Some(list) = &arguments.transitions {
            TransitionsDraft::List(
                list.iter()
                    .map(|transition| TransitionDraft {
                        from: transition.from.clone(),
                        to: transition.to.clone(),
                    })
                    .collect(),
            )
        } else {
            // The current ones, by ID: replace_workflow keeps those between
            // statuses that stay and refuses the rest, so drop those here.
            let kept = statuses.iter().filter_map(|status| status.id).collect::<Vec<_>>();
            TransitionsDraft::List(
                sqlx::query_as::<_, (Uuid, Uuid)>(
                    r#"
                    SELECT from_status_id, to_status_id FROM work_item_status_transitions
                    WHERE project_id = $1 AND from_status_id = ANY($2) AND to_status_id = ANY($2)
                    ORDER BY from_status_id, to_status_id
                    "#,
                )
                .bind(project_id)
                .bind(&kept)
                .fetch_all(&mut *transaction)
                .await?
                .into_iter()
                .map(|(from, to)| TransitionDraft {
                    from: from.to_string(),
                    to: to.to_string(),
                })
                .collect(),
            )
        };
        // Replacements are keyed by the retired status's or type's ID; an
        // agent may name it instead, so names are turned into IDs here.
        let named = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT id, name FROM work_item_statuses WHERE project_id = $1 AND enabled
            UNION ALL
            SELECT id, name FROM work_item_types WHERE project_id = $1 AND enabled
            "#,
        )
        .bind(project_id)
        .fetch_all(&mut *transaction)
        .await?;
        let mut replacements = HashMap::new();
        for (from, to) in arguments.replacements.iter().flatten() {
            let from = from.trim();
            match parse_id(from) {
                Some(id) => {
                    replacements.insert(id.to_string(), to.clone());
                }
                None => {
                    for (id, _) in named
                        .iter()
                        .filter(|(_, name)| name.to_lowercase() == from.to_lowercase())
                    {
                        replacements.insert(id.to_string(), to.clone());
                    }
                }
            }
        }
        let document = WorkflowDocument {
            expected_version: arguments.expected_version,
            estimate_unit: arguments
                .estimate_unit
                .as_deref()
                .map(|unit| unit.trim().to_lowercase()),
            statuses: statuses.clone(),
            types: types.clone(),
            transitions,
            replacements,
        };
        let actor = command_actor(principal, project_id);
        let change = match work_item_workflow::replace_workflow(&mut transaction, &actor, &document)
            .await?
        {
            Ok(change) => change,
            Err(WorkflowRefusal::VersionConflict { current_version }) => {
                return Ok(ToolResult::conflict(format!(
                    "Someone changed this workflow first (now version {current_version}). Read it again with workflow_get, then retry with expectedVersion {current_version}."
                )));
            }
            Err(WorkflowRefusal::Invalid { field, message }) => {
                return Ok(ToolResult::invalid(field, message));
            }
        };
        let workflow = workflow_value(&mut transaction, project_id).await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(WorkflowUpdateValue {
            statuses_added: count(change.statuses_added),
            statuses_retired: count(change.statuses_retired),
            types_added: count(change.types_added),
            types_retired: count(change.types_retired),
            tasks_moved: count(change.tasks_moved),
            workflow,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "workflow_update"))
}

// ---------------------------------------------------------------------------
// project_members

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MembersArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Include people and agents who were removed.
    pub include_removed: Option<bool>,
    /// Skip this many members: pass the last call's nextOffset to read the
    /// next page.
    pub offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemberLine {
    pub actor_id: String,
    pub name: String,
    pub handle: String,
    /// person or agent.
    pub kind: String,
    /// owner, admin, member, or viewer.
    pub role: String,
    /// False once someone is removed from the project.
    pub active: bool,
    /// Pass as expectedVersion to project_member_set.
    pub version: i64,
    pub joined_at: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MembersValue {
    pub members: Vec<MemberLine>,
    /// Set when there are more members: pass it as offset to read them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

/// Members per page of project_members.
const MEMBERS_PAGE: u32 = 500;

pub(super) async fn project_members(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MembersArguments,
) -> ToolResult<MembersValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let offset = arguments.offset.unwrap_or(0);
    let result = async {
        let Some(role) = project_role(state, principal, principal.project_id).await? else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !role_allows(principal, principal.project_id, &role, Action::MembershipView) {
            return Ok(ToolResult::denied(
                "Only a project's owners and admins can list its members and roles. people_search finds anyone you can mention.",
            ));
        }
        let members = sqlx::query_as::<_, MemberLine>(
            r#"
            SELECT membership.actor_id::text, membership.display_name AS name, membership.handle,
                   CASE actor.kind WHEN 'external_agent' THEN 'agent' ELSE 'person' END AS kind,
                   membership.role, membership.enabled AS active, membership.version,
                   membership.created_at::text AS joined_at
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.project_id = $1
              AND actor.kind IN ('human', 'external_agent', 'integration') -- // kind-check: mechanical (accounts that sign in: people, agents and apps; the system and workers aren't members to list)
              AND ($2 OR membership.enabled)
            ORDER BY membership.enabled DESC,
                     CASE membership.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1
                                          WHEN 'member' THEN 2 ELSE 3 END,
                     lower(membership.display_name), membership.actor_id
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(principal.project_id)
        .bind(arguments.include_removed.unwrap_or(false))
        .bind(i64::from(MEMBERS_PAGE) + 1)
        .bind(i64::from(offset))
        .fetch_all(crate::db::scoped(&state.pool))
        .await?;
        let mut members = members;
        let next_offset = (members.len() > MEMBERS_PAGE as usize).then(|| {
            members.truncate(MEMBERS_PAGE as usize);
            offset.saturating_add(MEMBERS_PAGE)
        });
        Ok::<_, AppError>(ToolResult::ok(MembersValue {
            members,
            next_offset,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_members"))
}

#[derive(Debug, FromRow)]
struct AdminRow {
    role: String,
    role_permissions: Option<Vec<String>>,
    group_permissions: Option<Vec<String>>,
    kind: String,
}

impl AdminRow {
    fn role(&self) -> StoredRole {
        StoredRole::of(&self.role, self.role_permissions.as_deref())
            .with_groups(self.group_permissions.clone())
    }
}

/// The caller's standing in a Project for changing it, archived or not.
/// Archived Projects are invisible everywhere else, so restoring one needs
/// this separate check.
async fn admin_row(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &McpPrincipal,
    project_id: Uuid,
) -> Result<Option<AdminRow>, AppError> {
    // Membership changes also lock the project first. Read authority after
    // that lock, and retain it until the project mutation commits.
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .execute(&mut **transaction)
        .await?;
    Ok(sqlx::query_as::<_, AdminRow>(
        r#"
        SELECT membership.role,
               workspace_role_permissions(membership.role_id) AS role_permissions,
               space_group_permissions($2, project.id) AS group_permissions,
               project.kind
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $2
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        JOIN actors actor ON actor.id = $2 AND actor.status = 'active'
        WHERE project.id = $1
          -- A person's sign-in must be active too; an agent has none.
          AND NOT EXISTS (
              SELECT 1 FROM users user_account
              WHERE user_account.actor_id = $2 AND user_account.status <> 'active'
          )
        FOR SHARE OF membership, organization_membership, actor
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

// ---------------------------------------------------------------------------
// project_layout_set

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectLayoutSetArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The preset, by name: a built-in one such as "Personal life", "Bug
    /// reporting" or "Software team", or one the workspace saved. A name
    /// that isn't found lists them all. Leave it out to follow the
    /// workspace's default again.
    pub preset: Option<String>,
    /// The layout version you last read. Leave it out to replace whatever
    /// is set.
    pub expected_version: Option<i64>,
    /// Set the workspace's default layout (with preset, or without it for
    /// none) instead of this project's: the workspace's owners and admins.
    #[serde(default)]
    pub workspace: Option<bool>,
    /// Instead of the project's layout, set which preset each role or
    /// person here gets: every rule the project should have (others are
    /// removed; an empty list removes them all).
    #[serde(default)]
    pub rules: Option<Vec<LayoutRuleArgument>>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LayoutRuleArgument {
    /// A role: admin (owners too), member, viewer, or one of the
    /// workspace's custom roles by name (role_list). Give role or person.
    #[serde(default)]
    pub role: Option<String>,
    /// One person in the project, by @handle or actor ID.
    #[serde(default)]
    pub person: Option<String>,
    /// The preset they get, by name, as for preset.
    pub preset: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectLayoutValue {
    pub project_id: String,
    /// The layout the project now uses, its own or the workspace's
    /// default. Missing when nothing is chosen: everything shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutLine>,
    /// The project's layout version, for expectedVersion; 0 while it
    /// follows the workspace. With workspace: true, the workspace's.
    pub version: i64,
    /// With rules: how many rules the project has now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<usize>,
}

pub(super) async fn project_layout_set(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectLayoutSetArguments,
) -> ToolResult<ProjectLayoutValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    if arguments.rules.is_some()
        && (arguments.preset.is_some()
            || arguments.expected_version.is_some()
            || arguments.workspace.is_some())
    {
        return ToolResult::invalid(
            "rules",
            "Set the rules on their own, without preset, expectedVersion or workspace.",
        );
    }
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        if arguments.rules.is_some() {
            // Lock the project first, as the web's rule changes do.
            sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
                .bind(project_id)
                .execute(&mut *transaction)
                .await?;
        }
        let Some(row) = admin_row(&mut transaction, principal, project_id).await? else {
            return Ok(ToolResult::not_found("That project"));
        };
        // The same people who change it in the project's settings: the
        // project's owners and admins, whatever their role in the workspace.
        if !role_allows(principal, project_id, &row.role(), Action::SettingsManage) {
            return Ok(ToolResult::denied(ADMIN_ONLY));
        }
        if row.kind == "direct" {
            return Ok(ToolResult::invalid(
                "projectId",
                "The Messages space has no layout of its own.",
            ));
        }
        let (organization_id, archived) = sqlx::query_as::<_, (Uuid, bool)>(
            "SELECT organization_id, archived_at IS NOT NULL FROM projects WHERE id = $1",
        )
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        if archived {
            return Ok(ToolResult::conflict(
                "That project is archived. Restore it with project_archive first.",
            ));
        }
        if let Some(rules) = arguments.rules.as_deref() {
            let (result, saved) =
                layout_rules_set(&mut transaction, principal, organization_id, rules).await?;
            // A refused set may have written some rules: keep none of them.
            if saved {
                transaction.commit().await?;
            }
            return Ok(result);
        }
        let wanted = arguments
            .preset
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let layout = match wanted {
            Some(text) => {
                let Some(preset) =
                    layouts::find_preset(&mut transaction, organization_id, text).await?
                else {
                    let names = layouts::preset_names(&mut transaction, organization_id).await?;
                    return Ok(ToolResult::invalid(
                        "preset",
                        format!(
                            "There's no preset called \"{text}\". Pick one of: {}.",
                            names.join(", ")
                        ),
                    ));
                };
                Some(Layout::preset(preset))
            }
            None => None,
        };
        let preset = layout.as_ref().map(|layout| layout.preset.clone());
        let workspace = arguments.workspace == Some(true);
        if workspace {
            crate::settings::authorize_workspace_manage_as(
                &mut *transaction,
                principal.actor_id,
                organization_id,
            )
            .await?;
        }
        let version = layouts::write_layout(
            &mut transaction,
            if workspace {
                Target::Workspace { organization_id }
            } else {
                Target::Project {
                    organization_id,
                    project_id,
                }
            },
            layout,
            arguments.expected_version,
            principal.actor_id,
        )
        .await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            if workspace {
                "layout.workspace_changed"
            } else {
                "layout.project_changed"
            },
            if workspace { "organization" } else { "project" },
            if workspace {
                organization_id
            } else {
                project_id
            },
            serde_json::json!({
                "preset": preset,
                "version": version,
                "organizationId": organization_id,
            }),
        )
        .await?;
        let layout = layouts::project_tone(&mut transaction, project_id)
            .await?
            .map(LayoutLine::from);
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(ProjectLayoutValue {
            project_id: project_id.to_string(),
            layout,
            version,
            rules: None,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_layout_set"))
}

/// Replaces the project's layout rules, by role or person and preset name,
/// for a caller already checked to manage the project.
async fn layout_rules_set(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &McpPrincipal,
    organization_id: Uuid,
    wanted: &[LayoutRuleArgument],
) -> Result<(ToolResult<ProjectLayoutValue>, bool), AppError> {
    let project_id = principal.project_id;
    let mut rules = Vec::with_capacity(wanted.len());
    for rule in wanted {
        let Some(preset) =
            layouts::find_preset(&mut *transaction, organization_id, &rule.preset).await?
        else {
            let names = layouts::preset_names(&mut *transaction, organization_id).await?;
            return Ok((
                ToolResult::invalid(
                    "rules",
                    format!(
                        "There's no preset called \"{}\". Pick one of: {}.",
                        rule.preset.trim(),
                        names.join(", ")
                    ),
                ),
                false,
            ));
        };
        let mut input = layouts::RuleInput {
            role: None,
            role_id: None,
            actor_id: None,
            preset,
        };
        match (
            rule.role.as_deref().map(str::trim),
            rule.person.as_deref().map(str::trim),
        ) {
            (Some(role @ ("admin" | "member" | "viewer")), None) => {
                input.role = Some(role.to_owned());
            }
            (Some(role), None) => {
                let Some(role_id) =
                    crate::roles::find_role(&mut *transaction, organization_id, role).await?
                else {
                    return Ok((
                        ToolResult::invalid(
                            "rules",
                            format!("There's no role called \"{role}\" in this workspace."),
                        ),
                        false,
                    ));
                };
                input.role_id = Some(role_id);
            }
            (None, Some(person)) => {
                let Some(actor_id) = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    SELECT membership.actor_id FROM project_memberships membership
                    WHERE membership.project_id = $1
                      AND (membership.actor_id::text = $2 OR lower(membership.handle) = lower($3))
                    LIMIT 1
                    "#,
                )
                .bind(project_id)
                .bind(person)
                .bind(person.trim_start_matches('@'))
                .fetch_optional(&mut **transaction)
                .await?
                else {
                    return Ok((
                        ToolResult::invalid("rules", format!("{person} isn't in this project.")),
                        false,
                    ));
                };
                input.actor_id = Some(actor_id);
            }
            _ => {
                return Ok((
                    ToolResult::invalid("rules", "Each rule names a role or a person, not both."),
                    false,
                ));
            }
        }
        rules.push(input);
    }
    let metadata = match layouts::set_rules_in(
        transaction,
        principal.actor_id,
        organization_id,
        project_id,
        &rules,
    )
    .await
    {
        Ok(metadata) => metadata,
        Err(AppError::Validation(_)) => {
            return Ok((
                ToolResult::invalid(
                    "rules",
                    "Each role and person can have one rule, people must be people in this project, and a project has at most a few dozen rules.",
                ),
                false,
            ));
        }
        Err(error) => return Err(error),
    };
    insert_audit(
        transaction,
        principal,
        project_id,
        "layout.rules_changed",
        "project",
        project_id,
        metadata,
    )
    .await?;
    let layout = layouts::project_tone(&mut *transaction, project_id)
        .await?
        .map(LayoutLine::from);
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM settings WHERE project_id = $1 AND key = 'layout.default' AND scope = 'project'",
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or(0);
    Ok((
        ToolResult::ok(ProjectLayoutValue {
            project_id: project_id.to_string(),
            layout,
            version,
            rules: Some(rules.len()),
        }),
        true,
    ))
}

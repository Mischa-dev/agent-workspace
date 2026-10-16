//! Project admin: a project's workflow and members, renaming, and archiving
//! or restoring it. Changing a project is for people with an owner or admin
//! role; agents never manage projects (docs/plans/mcp-tools.md, decision 5).
//! workflow_update sends the whole workflow through `replace_workflow`, as
//! the web settings page does.

use std::collections::{BTreeMap, HashMap};

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, parse_id, project_role, public_url, role_allows},
    tasks::command_actor,
    workflow::Workflow,
};
use crate::{
    AppState,
    error::AppError,
    memory,
    policy::{Action, ActorKind},
    work_item_commands::lock_membership,
    work_item_workflow::{
        self, AllTransitions, StatusDraft, TransitionDraft, TransitionsDraft, TypeDraft,
        WorkflowDocument, WorkflowRefusal,
    },
};

/// Owner or admin, and a person: what changing a project takes.
const ADMIN_ONLY: &str =
    "Only a project's owners and admins can do that, and only as themselves, not as an agent.";

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
            principal.actor_kind,
            principal.project_id,
            Action::WorkItemView,
        )
        .await?;
        let mut connection = state.pool.acquire().await?;
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
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(ADMIN_ONLY);
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
        let mut transaction = state.pool.begin().await?;
        super::common::mark_via(&mut transaction, principal).await?;
        let Some(membership) =
            lock_membership(&mut transaction, principal.actor_id, project_id).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !membership.enabled {
            return Ok(ToolResult::not_found("That project"));
        }
        if !role_allows(principal, project_id, &membership.role, Action::ProjectManage) {
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
}

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
              AND actor.kind IN ('human', 'external_agent')
              AND ($2 OR membership.enabled)
            ORDER BY membership.enabled DESC,
                     CASE membership.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1
                                          WHEN 'member' THEN 2 ELSE 3 END,
                     lower(membership.display_name)
            LIMIT 500
            "#,
        )
        .bind(principal.project_id)
        .bind(arguments.include_removed.unwrap_or(false))
        .fetch_all(&state.pool)
        .await?;
        Ok::<_, AppError>(ToolResult::ok(MembersValue { members }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_members"))
}

// ---------------------------------------------------------------------------
// project_update and project_archive

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectValue {
    pub id: String,
    pub key: String,
    pub name: String,
    pub archived: bool,
    /// Pass as expectedVersion next time.
    pub version: i64,
    #[sqlx(skip)]
    pub url: String,
}

#[derive(Debug, FromRow)]
struct AdminRow {
    role: String,
    organization_role: String,
    kind: String,
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
        SELECT membership.role, organization_membership.role AS organization_role,
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
        JOIN users user_account ON user_account.actor_id = $2 AND user_account.status = 'active'
        WHERE project.id = $1
        FOR SHARE OF membership, organization_membership, actor, user_account
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Whether the caller may change this Project: a person with write access,
/// connected for their whole account or to this Project, who is an owner or
/// admin of it.
fn may_administer(principal: &McpPrincipal, project_id: Uuid, row: &AdminRow) -> bool {
    principal.actor_kind == ActorKind::Human
        && (principal.account_wide || principal.project_id == project_id)
        && matches!(row.organization_role.as_str(), "owner" | "admin")
        && role_allows(principal, project_id, &row.role, Action::ProjectManage)
}

async fn project_value(state: &AppState, project_id: Uuid) -> Result<ProjectValue, AppError> {
    let mut value = sqlx::query_as::<_, ProjectValue>(
        r#"
        SELECT id::text, project_key AS key, name, archived_at IS NOT NULL AS archived, version
        FROM projects WHERE id = $1
        "#,
    )
    .bind(project_id)
    .fetch_one(&state.pool)
    .await?;
    value.url = public_url(state, &format!("/projects/{project_id}"));
    Ok(value)
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectUpdateArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The new name.
    pub name: String,
    /// The version you last read (projects_list or a previous result). Leave
    /// it out to rename whatever the current version is.
    pub expected_version: Option<i64>,
}

pub(super) async fn project_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectUpdateArguments,
) -> ToolResult<ProjectValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(ADMIN_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let Ok(name) = crate::projects::normalize_project_name(&arguments.name) else {
        return ToolResult::invalid(
            "name",
            "Give the project a name of 1 to 120 bytes, on one line.",
        );
    };
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let Some(row) = admin_row(&mut transaction, principal, project_id).await? else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !may_administer(principal, project_id, &row) {
            return Ok(ToolResult::denied(ADMIN_ONLY));
        }
        if row.kind == "direct" {
            return Ok(ToolResult::invalid(
                "projectId",
                "The Messages space can't be renamed.",
            ));
        }
        let current = sqlx::query_as::<_, (String, i64, bool)>(
            "SELECT name, version, archived_at IS NOT NULL FROM projects WHERE id = $1 FOR UPDATE",
        )
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        if current.2 {
            return Ok(ToolResult::conflict(
                "That project is archived. Restore it with project_archive first.",
            ));
        }
        if let Some(expected) = arguments.expected_version
            && expected != current.1
        {
            return Ok(ToolResult::conflict(format!(
                "Someone changed this project first (now version {}). Read it again, then retry.",
                current.1
            )));
        }
        if current.0 != name {
            sqlx::query(
                "UPDATE projects SET name = $2, version = version + 1, updated_at = now() WHERE id = $1",
            )
            .bind(project_id)
            .bind(&name)
            .execute(&mut *transaction)
            .await?;
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                "project.mcp_renamed",
                "project",
                project_id,
                serde_json::json!({ "previous_name": current.0, "previous_version": current.1 }),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(project_value(state, project_id).await?))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_update"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectArchiveArguments {
    /// The project's ID or link. An archived project can only be named this
    /// way, since its key no longer resolves.
    pub project: String,
    /// True to archive, false to restore. Defaults to true.
    pub archived: Option<bool>,
    /// The version you last read. Leave it out to act on the current one.
    pub expected_version: Option<i64>,
}

pub(super) async fn project_archive(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ProjectArchiveArguments,
) -> ToolResult<ProjectValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(ADMIN_ONLY);
    }
    let archive = arguments.archived.unwrap_or(true);
    let result = async {
        let Some(project_id) = project_reference(state, principal, &arguments.project).await?
        else {
            return Ok(ToolResult::not_found("That project"));
        };
        let mut transaction = state.pool.begin().await?;
        let Some(row) = admin_row(&mut transaction, principal, project_id).await? else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !may_administer(principal, project_id, &row) {
            return Ok(ToolResult::denied(ADMIN_ONLY));
        }
        if row.kind == "direct" {
            return Ok(ToolResult::invalid(
                "project",
                "The Messages space can't be archived.",
            ));
        }
        let (archived, version) = sqlx::query_as::<_, (bool, i64)>(
            "SELECT archived_at IS NOT NULL, version FROM projects WHERE id = $1 FOR UPDATE",
        )
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        if let Some(expected) = arguments.expected_version
            && expected != version
        {
            return Ok(ToolResult::conflict(format!(
                "Someone changed this project first (now version {version}). Read it again, then retry."
            )));
        }
        if archived == archive {
            transaction.commit().await?;
            return Ok(ToolResult::ok(project_value(state, project_id).await?));
        }
        if archive {
            let has_active_run = sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM agent_runs run
                    JOIN conversations conversation ON conversation.id = run.conversation_id
                    WHERE conversation.project_id = $1
                      AND run.state IN (
                          'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
                      )
                )
                "#,
            )
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
            if has_active_run {
                return Ok(ToolResult::conflict(
                    "Codex is still running in that project. Wait for its runs to finish, or interrupt them, then archive it.",
                ));
            }
            sqlx::query(
                r#"
                UPDATE projects
                SET archived_at = now(), archived_by_actor_id = $2,
                    version = version + 1, updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(project_id)
            .bind(principal.actor_id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE projects
                SET archived_at = NULL, archived_by_actor_id = NULL,
                    version = version + 1, updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(project_id)
            .execute(&mut *transaction)
            .await?;
        }
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            if archive {
                "project.mcp_archived"
            } else {
                "project.mcp_restored"
            },
            "project",
            project_id,
            serde_json::json!({ "previous_version": version }),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(project_value(state, project_id).await?))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "project_archive"))
}

/// A Project named by ID, link, or key, including archived ones the caller
/// still belongs to. Keys match active Projects first.
async fn project_reference(
    state: &AppState,
    principal: &McpPrincipal,
    reference: &str,
) -> Result<Option<Uuid>, AppError> {
    if let Some(project_id) = parse_id(reference) {
        return Ok(Some(project_id));
    }
    let matches = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $1
         AND membership.enabled
        WHERE lower(project.project_key) = lower($2)
        ORDER BY project.archived_at IS NOT NULL
        LIMIT 2
        "#,
    )
    .bind(principal.actor_id)
    .bind(reference.trim())
    .fetch_all(&state.pool)
    .await?;
    Ok(matches.first().copied())
}

// ---------------------------------------------------------------------------
// project_member_set

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MemberSetArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Who: an @handle or actor ID of someone in your organization, person
    /// or agent.
    pub person: String,
    /// admin, member, or viewer. Agents can be members or viewers. Leave it
    /// out when removing; new members default to member.
    pub role: Option<String>,
    /// True to remove them from the project. Their access ends at once;
    /// what they wrote stays.
    pub remove: Option<bool>,
    /// Their membership version from project_members. Leave it out to change
    /// the current one.
    pub expected_version: Option<i64>,
}

pub(super) async fn project_member_set(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MemberSetArguments,
) -> ToolResult<MemberLine> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(ADMIN_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let remove = arguments.remove.unwrap_or(false);
    if remove && arguments.role.is_some() {
        return ToolResult::invalid("role", "Leave out role when removing someone.");
    }
    if let Some(role) = arguments.role.as_deref()
        && !matches!(role, "admin" | "member" | "viewer")
    {
        return ToolResult::invalid(
            "role",
            "Use admin, member, or viewer. Ownership can only be given in the web app.",
        );
    }
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let Some(row) = admin_row(&mut transaction, principal, project_id).await? else {
            return Ok(ToolResult::not_found("That project"));
        };
        if !may_administer(principal, project_id, &row) {
            return Ok(ToolResult::denied(ADMIN_ONLY));
        }
        let person = arguments.person.trim();
        let Some(actor_id) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT profile.actor_id
            FROM projects project
            JOIN organization_actor_profiles profile
              ON profile.organization_id = project.organization_id
            WHERE project.id = $1
              AND (profile.actor_id::text = $2 OR lower(profile.handle) = lower($3))
            LIMIT 1
            "#,
        )
        .bind(project_id)
        .bind(person)
        .bind(person.trim_start_matches('@'))
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(ToolResult::not_found(&format!(
                "{person} in your organization"
            )));
        };
        if actor_id == principal.actor_id {
            return Ok(ToolResult::invalid(
                "person",
                "You can't change your own membership. Ask another owner.",
            ));
        }
        let existing = sqlx::query_scalar::<_, bool>(
            "SELECT enabled FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(project_id)
        .bind(actor_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let action = match (existing, remove) {
            (None, true) => {
                return Ok(ToolResult::not_found(&format!("{person} in this project")));
            }
            (None, false) => {
                let added = crate::memberships::add_member(
                    &mut transaction,
                    principal.actor_id,
                    project_id,
                    actor_id,
                    arguments.role.as_deref().unwrap_or("member"),
                )
                .await?;
                added.inserted.then_some("project_membership.mcp_added")
            }
            (Some(_), _) => {
                let change = crate::memberships::change_member(
                    &mut transaction,
                    principal.actor_id,
                    project_id,
                    actor_id,
                    arguments.role.as_deref(),
                    Some(!remove),
                    arguments.expected_version,
                )
                .await?;
                change.updated.is_some().then_some(if remove {
                    "project_membership.mcp_removed"
                } else {
                    "project_membership.mcp_changed"
                })
            }
        };
        if let Some(action) = action {
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                action,
                "actor",
                actor_id,
                serde_json::json!({ "role": arguments.role }),
            )
            .await?;
        }
        transaction.commit().await?;
        let member = sqlx::query_as::<_, MemberLine>(
            r#"
            SELECT membership.actor_id::text, membership.display_name AS name, membership.handle,
                   CASE actor.kind WHEN 'external_agent' THEN 'agent' ELSE 'person' END AS kind,
                   membership.role, membership.enabled AS active, membership.version,
                   membership.created_at::text AS joined_at
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.project_id = $1 AND membership.actor_id = $2
            "#,
        )
        .bind(project_id)
        .bind(actor_id)
        .fetch_one(&state.pool)
        .await?;
        Ok::<_, AppError>(ToolResult::ok(member))
    }
    .await;
    result.unwrap_or_else(|error| match error {
        AppError::Forbidden => ToolResult::denied(
            "You can't change that person's access. Admins manage members and viewers; only owners manage admins, and agents can only be members or viewers.",
        ),
        error => ToolResult::from_error(error, "project_member_set"),
    })
}

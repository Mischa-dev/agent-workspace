//! AWD-74: roles and access.
//!
//! Four layers decide what someone may do, narrowest last: modules (AWD-75
//! joins them in [`effective`]), roles, agent limits, and each person's own
//! view (layouts). This module is the roles and agent limits.
//!
//! - A workspace's roles are rows in `workspace_roles`. The four built-in
//!   ones mean exactly what they always have: their permissions come from
//!   `policy::PERMISSIONS`. Owners and admins make custom roles from Admin,
//!   Member or Viewer and keep a subset of its permissions; only owners make
//!   or change one based on Admin, and nobody gives a role a permission they
//!   don't hold themselves.
//! - Someone holds a role in the workspace and one in each project they're
//!   in; in a project, the project's role is the one that counts. Assigning
//!   a workspace role can apply it to their projects too.
//! - A role has defaults for people who hold it: the layout they start with,
//!   private channels they join on arriving in a project, the views they
//!   start with pinned, and Home sections they don't see.
//! - Agent limits: an app or agent acting for a person may do only what the
//!   person may, cut down by their role's agent limit and their own. An agent
//!   with its own identity holds a role like anyone else.
//!
//! Every change is audited with what it was before and after.

use std::collections::BTreeSet;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, patch, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    layouts,
    mcp::McpPrincipal,
    outbox::{OutboxEventPayload, ProjectMembershipUpdatedPayload, enqueue_typed},
    policy::{
        ActorKind, AgentLimits, MembershipRole, MembershipScope, PERMISSION_MODULES, PERMISSIONS,
        Permission, PermissionSet, RoleGrant, current_agent_limit,
    },
    work_items::views::BUILTIN_VIEWS,
};

const MAX_CUSTOM_ROLES: i64 = 50;
const MAX_DEFAULT_CHANNELS: usize = 20;
const MAX_HIDDEN_PARTS: usize = 40;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/roles",
            get(list_roles).post(create_role_route),
        )
        .route(
            "/api/v1/organizations/{organization_id}/roles/{role_id}",
            patch(change_role_route).delete(remove_role_route),
        )
        .route(
            "/api/v1/organizations/{organization_id}/members/{actor_id}/role",
            put(assign_role_route),
        )
        .route(
            "/api/v1/organizations/{organization_id}/members/{actor_id}/agent-limit",
            put(set_agent_limit_route),
        )
        .route(
            "/api/v1/organizations/{organization_id}/permissions",
            get(read_permissions),
        )
}

// ---------------------------------------------------------------------------
// What someone may do

/// What a membership lets someone do: their role's permissions, cut down by
/// an agent limit when an app or agent is acting for them. Agents never get
/// the people-only permissions. Later layers (modules, AWD-75) join here.
pub(crate) fn effective(
    grant: RoleGrant,
    limit: Option<PermissionSet>,
    kind: ActorKind,
) -> PermissionSet {
    let mut permissions = grant.permissions();
    if let Some(limit) = limit {
        permissions = permissions.intersect(limit.union(PermissionSet::required()));
    }
    if kind != ActorKind::Human {
        permissions = without_people_only(permissions);
    }
    permissions
}

pub(crate) fn without_people_only(permissions: PermissionSet) -> PermissionSet {
    PERMISSIONS
        .iter()
        .filter(|info| info.people_only)
        .fold(permissions, |set, info| set.without(info.permission))
}

/// A limit as stored: NULL is no limit.
fn stored_limit(keys: Option<&[String]>) -> PermissionSet {
    keys.map_or_else(PermissionSet::all, PermissionSet::from_keys)
}

#[derive(Debug, FromRow)]
struct LimitRow {
    scope_id: Uuid,
    organization_id: Uuid,
    role: String,
    role_permissions: Option<Vec<String>>,
    role_limit: Option<Vec<String>>,
    personal_limit: Option<Vec<String>>,
}

/// The role (custom, or the built-in row) a membership holds, for its
/// agent limit and defaults.
const HELD_ROLE_JOIN: &str = "LEFT JOIN workspace_roles held ON held.organization_id = {org} \
     AND (held.id = {membership}.role_id OR ({membership}.role_id IS NULL AND held.key = {membership}.role))";

fn held_role_join(organization: &str, membership: &str) -> String {
    HELD_ROLE_JOIN
        .replace("{org}", organization)
        .replace("{membership}", membership)
}

/// What a person's apps and agents may do in each workspace and project of
/// theirs, and the union of it across all of them.
pub(crate) struct AppAccess {
    pub limits: AgentLimits,
    pub union: PermissionSet,
}

/// Loads `actor_id`'s memberships and works out, for an app or agent acting
/// for them, what it may do in each workspace and project: the person's
/// role there cut down by the role's agent limit, the workspace role's, and
/// their own.
pub(crate) async fn app_access(
    connection: &mut PgConnection,
    actor_id: Uuid,
) -> Result<AppAccess, AppError> {
    let organizations = sqlx::query_as::<_, LimitRow>(&format!(
        r#"
        SELECT membership.organization_id AS scope_id, membership.organization_id,
               membership.role, workspace_role_permissions(membership.role_id) AS role_permissions,
               held.agent_limit AS role_limit, membership.agent_limit AS personal_limit
        FROM organization_memberships membership
        {held}
        WHERE membership.actor_id = $1 AND membership.enabled
        "#,
        held = held_role_join("membership.organization_id", "membership"),
    ))
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let projects = sqlx::query_as::<_, LimitRow>(&format!(
        r#"
        SELECT membership.project_id AS scope_id, project.organization_id,
               membership.role, workspace_role_permissions(membership.role_id) AS role_permissions,
               held.agent_limit AS role_limit, NULL::text[] AS personal_limit
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        {held}
        WHERE membership.actor_id = $1 AND membership.enabled
        "#,
        held = held_role_join("project.organization_id", "membership"),
    ))
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut limits = AgentLimits {
        actor_id,
        ..AgentLimits::default()
    };
    let mut union = PermissionSet::EMPTY;
    for row in &organizations {
        let limit = stored_limit(row.role_limit.as_deref())
            .intersect(stored_limit(row.personal_limit.as_deref()))
            .union(PermissionSet::required());
        limits.organizations.insert(row.scope_id, limit);
        if let Ok(grant) = RoleGrant::stored(&row.role, row.role_permissions.as_deref()) {
            union = union.union(effective(grant, Some(limit), ActorKind::Human));
        }
    }
    for row in &projects {
        // Someone no longer in the workspace has no workspace limit, so
        // their apps may do nothing beyond reading there.
        let workspace = limits
            .organizations
            .get(&row.organization_id)
            .copied()
            .unwrap_or(PermissionSet::required());
        let limit = stored_limit(row.role_limit.as_deref())
            .intersect(workspace)
            .union(PermissionSet::required());
        limits.projects.insert(row.scope_id, limit);
        if let Ok(grant) = RoleGrant::stored(&row.role, row.role_permissions.as_deref())
            && grant.role() != MembershipRole::Worker
        {
            union = union.union(effective(grant, Some(limit), ActorKind::Human));
        }
    }
    Ok(AppAccess { limits, union })
}

/// What an agent with its own identity may do anywhere: the union of its
/// roles, without the people-only permissions.
pub(crate) async fn agent_union(
    connection: &mut PgConnection,
    actor_id: Uuid,
) -> Result<PermissionSet, AppError> {
    let rows = sqlx::query_as::<_, (String, Option<Vec<String>>)>(
        r#"
        SELECT role, workspace_role_permissions(role_id) FROM project_memberships
        WHERE actor_id = $1 AND enabled
        UNION ALL
        SELECT role, workspace_role_permissions(role_id) FROM organization_memberships
        WHERE actor_id = $1 AND enabled
        "#,
    )
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|(role, permissions)| RoleGrant::stored(role, permissions.as_deref()).ok())
        .filter(|grant| grant.role() != MembershipRole::Worker)
        .fold(PermissionSet::EMPTY, |set, grant| {
            set.union(effective(grant, None, ActorKind::ExternalAgent))
        }))
}

/// The caller in a workspace: their workspace role and what it lets them do
/// here and now (an app acting for them is cut down by their agent limit).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Standing {
    pub actor_id: Uuid,
    pub kind: ActorKind,
    pub role: MembershipRole,
    pub permissions: PermissionSet,
}

impl Standing {
    fn may(self, permission: Permission) -> bool {
        self.permissions.contains(permission)
    }

    fn is_owner(self) -> bool {
        self.role == MembershipRole::Owner
    }
}

pub(crate) async fn standing(
    connection: &mut PgConnection,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<Standing, AppError> {
    let (kind, role, role_permissions) =
        sqlx::query_as::<_, (String, String, Option<Vec<String>>)>(
            r#"
            SELECT actor.kind, membership.role, workspace_role_permissions(membership.role_id)
            FROM organization_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND membership.enabled AND actor.status = 'active'
              AND actor.kind IN ('human', 'external_agent')
            "#,
        )
        .bind(organization_id)
        .bind(actor_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    let kind = if kind == "human" {
        ActorKind::Human
    } else {
        ActorKind::ExternalAgent
    };
    let grant = RoleGrant::stored(&role, role_permissions.as_deref())?;
    let limit = current_agent_limit(actor_id, MembershipScope::Organization(organization_id));
    Ok(Standing {
        actor_id,
        kind,
        role: grant.role(),
        permissions: effective(grant, limit, kind),
    })
}

// ---------------------------------------------------------------------------
// Roles: reading

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoleDefaults {
    /// A built-in preset's id or a saved preset's ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_preset: Option<String>,
    /// Private channels holders join when they arrive in a project, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
    /// The built-in views holders start with pinned. Left out: My tasks and
    /// Ready for agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_views: Option<Vec<String>>,
    /// Home and workspace sections holders don't see.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub home_hidden: Vec<String>,
}

#[derive(Debug, FromRow)]
struct RoleRow {
    id: Uuid,
    key: String,
    built_in: bool,
    base_role: String,
    name: String,
    description: String,
    permissions: Vec<String>,
    agent_limit: Option<Vec<String>>,
    defaults: Value,
    is_default: bool,
    version: i64,
    updated_at: DateTime<Utc>,
    workspace_holders: i64,
    project_holders: i64,
}

impl RoleRow {
    fn base(&self) -> Result<MembershipRole, AppError> {
        MembershipRole::parse(&self.base_role).ok_or(AppError::Internal("stored base role"))
    }

    /// What the role lets holders do.
    fn grant(&self) -> Result<RoleGrant, AppError> {
        Ok(if self.built_in {
            RoleGrant::built_in(self.base()?)
        } else {
            RoleGrant::stored(&self.base_role, Some(&self.permissions))?
        })
    }

    fn defaults(&self) -> RoleDefaults {
        serde_json::from_value(self.defaults.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoleView {
    pub id: Uuid,
    pub key: String,
    pub built_in: bool,
    pub base_role: String,
    pub name: String,
    pub description: String,
    pub permissions: Vec<&'static str>,
    /// What apps and agents acting for holders may do; absent when it's the
    /// same as the holder.
    pub agent_limit: Option<Vec<&'static str>>,
    pub defaults: RoleDefaults,
    pub is_default: bool,
    pub version: i64,
    pub updated_at: DateTime<Utc>,
    /// People and agents holding it in the workspace.
    pub workspace_holders: i64,
    /// Memberships holding it in the workspace's projects.
    pub project_holders: i64,
    pub capabilities: RoleCapabilities,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoleCapabilities {
    /// Change its name, description, defaults and agent limit.
    pub edit: bool,
    pub edit_permissions: bool,
    pub remove: bool,
    /// Give it to someone.
    pub assign: bool,
}

/// What `caller` may do to a role.
fn role_capabilities(caller: Standing, row: &RoleRow) -> RoleCapabilities {
    let base_is_high = matches!(row.base_role.as_str(), "owner" | "admin");
    let manage = caller.kind == ActorKind::Human
        && caller.may(Permission::RolesManage)
        && (!base_is_high || caller.is_owner());
    let within_caller = row
        .grant()
        .is_ok_and(|grant| grant.permissions().is_subset_of(caller.permissions));
    RoleCapabilities {
        edit: manage,
        edit_permissions: manage && !row.built_in,
        remove: manage
            && !row.built_in
            && !row.is_default
            && row.workspace_holders + row.project_holders == 0,
        assign: caller.kind == ActorKind::Human
            && caller.may(Permission::MembersManage)
            && row.key != "owner"
            && (!base_is_high || caller.is_owner())
            && within_caller,
    }
}

fn role_view(caller: Standing, row: RoleRow) -> Result<RoleView, AppError> {
    let capabilities = role_capabilities(caller, &row);
    let grant = row.grant()?;
    let defaults = row.defaults();
    Ok(RoleView {
        id: row.id,
        key: row.key,
        built_in: row.built_in,
        base_role: row.base_role,
        name: row.name,
        description: row.description,
        permissions: grant.permissions().keys(),
        agent_limit: row
            .agent_limit
            .as_deref()
            .map(|keys| PermissionSet::from_keys(keys).keys()),
        defaults,
        is_default: row.is_default,
        version: row.version,
        updated_at: row.updated_at,
        workspace_holders: row.workspace_holders,
        project_holders: row.project_holders,
        capabilities,
    })
}

const ROLE_SELECT: &str = r#"
    SELECT role.id, role.key, role.built_in, role.base_role, role.name, role.description,
           role.permissions, role.agent_limit, role.defaults, role.is_default, role.version,
           role.updated_at,
           (SELECT count(*) FROM organization_memberships holder
             JOIN actors actor ON actor.id = holder.actor_id AND actor.status = 'active'
              AND actor.kind IN ('human', 'external_agent')
             WHERE holder.organization_id = role.organization_id AND holder.enabled
               AND (holder.role_id = role.id
                    OR (role.built_in AND holder.role_id IS NULL AND holder.role = role.key)))
             AS workspace_holders,
           (SELECT count(*) FROM project_memberships holder
             JOIN projects project ON project.id = holder.project_id
              AND project.organization_id = role.organization_id
              AND project.kind <> 'direct' AND project.archived_at IS NULL
             JOIN actors actor ON actor.id = holder.actor_id AND actor.status = 'active'
             WHERE holder.enabled
               AND (holder.role_id = role.id
                    OR (role.built_in AND holder.role_id IS NULL AND holder.role = role.key)))
             AS project_holders
    FROM workspace_roles role
"#;

const ROLE_ORDER: &str = "ORDER BY role.built_in DESC, \
     CASE role.key WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 WHEN 'member' THEN 2 WHEN 'viewer' THEN 3 ELSE 4 END, \
     lower(role.name), role.id";

async fn role_rows(
    connection: &mut PgConnection,
    organization_id: Uuid,
) -> Result<Vec<RoleRow>, AppError> {
    Ok(sqlx::query_as::<_, RoleRow>(&format!(
        "{ROLE_SELECT} WHERE role.organization_id = $1 {ROLE_ORDER}"
    ))
    .bind(organization_id)
    .fetch_all(&mut *connection)
    .await?)
}

async fn role_row(
    connection: &mut PgConnection,
    organization_id: Uuid,
    role_id: Uuid,
) -> Result<RoleRow, AppError> {
    sqlx::query_as::<_, RoleRow>(&format!(
        "{ROLE_SELECT} WHERE role.organization_id = $1 AND role.id = $2"
    ))
    .bind(organization_id)
    .bind(role_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("role"))
}

/// A workspace's role by ID, key or name (any case), for agents.
pub(crate) async fn find_role(
    connection: &mut PgConnection,
    organization_id: Uuid,
    text: &str,
) -> Result<Option<Uuid>, AppError> {
    let text = text.trim();
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM workspace_roles
        WHERE organization_id = $1
          AND (id::text = $2 OR key = lower($2) OR lower(name) = lower($2))
        ORDER BY built_in DESC
        LIMIT 1
        "#,
    )
    .bind(organization_id)
    .bind(text)
    .fetch_optional(&mut *connection)
    .await?)
}

pub(crate) async fn list(
    connection: &mut PgConnection,
    caller: Standing,
    organization_id: Uuid,
) -> Result<Vec<RoleView>, AppError> {
    role_rows(connection, organization_id)
        .await?
        .into_iter()
        .map(|row| role_view(caller, row))
        .collect()
}

pub(crate) async fn read(
    connection: &mut PgConnection,
    caller: Standing,
    organization_id: Uuid,
    role_id: Uuid,
) -> Result<RoleView, AppError> {
    role_view(
        caller,
        role_row(connection, organization_id, role_id).await?,
    )
}

// ---------------------------------------------------------------------------
// The permission catalogue and starting points

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionEntry {
    pub key: &'static str,
    pub module: &'static str,
    pub label: &'static str,
    pub meaning: &'static str,
    /// project, workspace or both.
    pub scope: &'static str,
    /// The built-in roles that hold it.
    pub built_in: Vec<&'static str>,
    pub people_only: bool,
    pub required: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModuleEntry {
    pub key: &'static str,
    pub name: &'static str,
}

pub(crate) fn catalogue() -> (Vec<ModuleEntry>, Vec<PermissionEntry>) {
    let modules = PERMISSION_MODULES
        .iter()
        .map(|(key, name)| ModuleEntry { key, name })
        .collect();
    let permissions = PERMISSIONS
        .iter()
        .map(|info| PermissionEntry {
            key: info.key,
            module: info.module,
            label: info.label,
            meaning: info.meaning,
            scope: info.scope.key(),
            built_in: ["owner", "admin", "member", "viewer"]
                .into_iter()
                .zip(info.built_in)
                .filter_map(|(role, held)| held.then_some(role))
                .collect(),
            people_only: info.people_only,
            required: info.required,
        })
        .collect();
    (modules, permissions)
}

/// A ready-made custom role to start from.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartingPoint {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub base_role: &'static str,
    pub permissions: Vec<&'static str>,
    pub agent_limit: Option<Vec<&'static str>>,
    pub defaults: RoleDefaults,
}

pub(crate) fn starting_points() -> Vec<StartingPoint> {
    vec![
        StartingPoint {
            key: "contractor",
            name: "Contractor",
            description: "Works on tasks and code they're given: no new channels, no approving runs, no deleting.",
            base_role: "member",
            permissions: vec![
                "projects.view",
                "tasks.view",
                "tasks.create",
                "tasks.edit",
                "channels.post",
                "pages.edit",
                "whiteboards.edit",
                "runs.launch",
                "runs.interrupt",
                "code.view",
                "code.publish",
            ],
            agent_limit: None,
            defaults: RoleDefaults::default(),
        },
        StartingPoint {
            key: "read_only",
            name: "Read only",
            description: "Reads everything they're given and changes nothing, across the whole workspace.",
            base_role: "viewer",
            permissions: vec!["projects.view", "tasks.view", "code.view"],
            agent_limit: None,
            defaults: RoleDefaults::default(),
        },
        StartingPoint {
            key: "family_member",
            name: "Family member",
            description: "Lists, chores and plans: tasks, channels, pages and whiteboards, without runs or code.",
            base_role: "member",
            permissions: vec![
                "projects.view",
                "tasks.view",
                "tasks.create",
                "tasks.edit",
                "tasks.delete",
                "channels.post",
                "channels.create",
                "pages.edit",
                "whiteboards.edit",
            ],
            agent_limit: Some(vec![
                "projects.view",
                "tasks.view",
                "tasks.create",
                "tasks.edit",
                "channels.post",
            ]),
            defaults: RoleDefaults {
                layout_preset: Some("household".to_owned()),
                pinned_views: Some(vec!["my_tasks".to_owned()]),
                home_hidden: vec!["home.capacity".to_owned(), "workspace.runs".to_owned()],
                ..RoleDefaults::default()
            },
        },
    ]
}

// ---------------------------------------------------------------------------
// Checking what's sent

fn permission_keys(keys: &[String]) -> Result<PermissionSet, AppError> {
    if keys.len() > 100 {
        return Err(AppError::Validation("role permissions"));
    }
    keys.iter().try_fold(PermissionSet::EMPTY, |set, key| {
        Permission::from_key(key)
            .map(|permission| set.with(permission))
            .ok_or(AppError::Validation("role permission"))
    })
}

fn role_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("role name"));
    }
    Ok(name.to_owned())
}

fn role_description(value: &str) -> Result<String, AppError> {
    let description = value.trim();
    if description.chars().count() > 300 || description.chars().any(char::is_control) {
        return Err(AppError::Validation("role description"));
    }
    Ok(description.to_owned())
}

/// `Family member` → `family-member`: the role's key.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug = slug.trim_end_matches('-').to_owned();
    if !slug.starts_with(|c: char| c.is_ascii_lowercase()) {
        slug.insert_str(0, "role-");
    }
    slug.truncate(32);
    slug.trim_end_matches('-').to_owned()
}

async fn checked_defaults(
    connection: &mut PgConnection,
    organization_id: Uuid,
    mut defaults: RoleDefaults,
) -> Result<RoleDefaults, AppError> {
    if let Some(preset) = defaults.layout_preset.as_deref()
        && !layouts::preset_is_known(connection, organization_id, preset).await?
    {
        return Err(AppError::Validation("role layout preset"));
    }
    let mut channels = Vec::new();
    for channel in &defaults.channels {
        let name = channel.trim().trim_start_matches('#').trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(AppError::Validation("role channel"));
        }
        if !channels
            .iter()
            .any(|known: &String| known.eq_ignore_ascii_case(name))
        {
            channels.push(name.to_owned());
        }
    }
    if channels.len() > MAX_DEFAULT_CHANNELS {
        return Err(AppError::Validation("role channels"));
    }
    defaults.channels = channels;
    if let Some(views) = defaults.pinned_views.as_mut() {
        if views
            .iter()
            .any(|view| !BUILTIN_VIEWS.iter().any(|builtin| builtin.key == view))
        {
            return Err(AppError::Validation("role pinned view"));
        }
        let unique = views.iter().cloned().collect::<BTreeSet<_>>();
        views.retain(|view| unique.contains(view));
        views.dedup();
    }
    let hidden = defaults
        .home_hidden
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if hidden.len() > MAX_HIDDEN_PARTS
        || hidden.iter().any(|part| {
            !(part.starts_with("home.") || part.starts_with("workspace."))
                || !layouts::part_is_known(part)
        })
    {
        return Err(AppError::Validation("role hidden section"));
    }
    defaults.home_hidden = hidden.into_iter().collect();
    Ok(defaults)
}

// ---------------------------------------------------------------------------
// Audit

/// Who made a change and through what: the browser or an app.
pub(crate) enum Auditor<'a> {
    Browser {
        headers: &'a HeaderMap,
        principal: &'a AuthenticatedActor,
    },
    App(&'a McpPrincipal),
}

impl Auditor<'_> {
    async fn record(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        organization_id: Uuid,
        action: &str,
        target_kind: &str,
        target_id: Uuid,
        metadata: Value,
    ) -> Result<(), AppError> {
        match self {
            Self::Browser { headers, principal } => {
                crate::workspace::audit(
                    transaction,
                    headers,
                    principal,
                    organization_id,
                    action,
                    target_kind,
                    Some(target_id),
                    metadata,
                )
                .await
            }
            Self::App(principal) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_events (
                        id, actor_id, organization_id, authentication_class, action,
                        target_kind, target_id, outcome, metadata, channel, client_name,
                        mcp_credential_id, mcp_oauth_grant_id, external_agent_connection_id
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'succeeded', $8, 'mcp', $9, $10, $11, $12)
                    "#,
                )
                .bind(Uuid::now_v7())
                .bind(principal.actor_id)
                .bind(organization_id)
                .bind(principal.authentication_class)
                .bind(action)
                .bind(target_kind)
                .bind(target_id)
                .bind(metadata)
                .bind(&principal.client_name)
                .bind(principal.mcp_credential_id)
                .bind(principal.oauth_grant_id)
                .bind(principal.external_agent_connection_id)
                .execute(&mut **transaction)
                .await?;
                Ok(())
            }
        }
    }
}

/// A role as the audit log keeps it.
fn audit_shape(view: &RoleView) -> Value {
    json!({
        "name": view.name,
        "description": view.description,
        "baseRole": view.base_role,
        "permissions": view.permissions,
        "agentLimit": view.agent_limit,
        "defaults": view.defaults,
        "isDefault": view.is_default,
    })
}

// ---------------------------------------------------------------------------
// Roles: writing

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoleInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub base_role: String,
    pub permissions: Vec<String>,
    #[serde(default)]
    pub agent_limit: Option<Vec<String>>,
    #[serde(default)]
    pub defaults: RoleDefaults,
    #[serde(default)]
    pub is_default: bool,
}

/// Whether `caller` may make or change roles based on `base`.
fn may_define(caller: Standing, base: MembershipRole) -> Result<(), AppError> {
    if caller.kind != ActorKind::Human || !caller.may(Permission::RolesManage) {
        return Err(AppError::Forbidden);
    }
    if matches!(base, MembershipRole::Owner | MembershipRole::Admin) && !caller.is_owner() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// A custom role's permissions: known keys, within its base, and within
/// what the caller holds.
fn custom_permissions(
    caller: Standing,
    base: MembershipRole,
    keys: &[String],
) -> Result<PermissionSet, AppError> {
    let permissions = permission_keys(keys)?.union(PermissionSet::required());
    if !permissions.is_subset_of(PermissionSet::built_in(base)) {
        return Err(AppError::Refused {
            field: "permissions",
            message: "A role can only have permissions its base role has.",
        });
    }
    if !permissions.is_subset_of(caller.permissions) {
        return Err(AppError::Forbidden);
    }
    Ok(permissions)
}

fn limit_keys(keys: Option<&[String]>) -> Result<Option<Vec<&'static str>>, AppError> {
    keys.map(|keys| permission_keys(keys).map(|set| set.union(PermissionSet::required()).keys()))
        .transpose()
}

async fn lock_roles(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("awd74-roles:{organization_id}"))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn name_taken(error: sqlx::Error) -> AppError {
    match &error {
        sqlx::Error::Database(database)
            if database.code().as_deref() == Some("23505")
                && database.constraint() == Some("workspace_roles_name") =>
        {
            AppError::Conflict("role", "name_taken")
        }
        _ => AppError::Database(error),
    }
}

pub(crate) async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    caller: Standing,
    organization_id: Uuid,
    input: RoleInput,
) -> Result<RoleView, AppError> {
    let base = match input.base_role.as_str() {
        "admin" => MembershipRole::Admin,
        "member" => MembershipRole::Member,
        "viewer" => MembershipRole::Viewer,
        _ => return Err(AppError::Validation("role base")),
    };
    may_define(caller, base)?;
    let name = role_name(&input.name)?;
    let description = role_description(&input.description)?;
    let permissions = custom_permissions(caller, base, &input.permissions)?;
    let agent_limit = limit_keys(input.agent_limit.as_deref())?;
    if input.is_default && base == MembershipRole::Admin {
        return Err(AppError::Refused {
            field: "isDefault",
            message: "New members can't start with a role based on Admin.",
        });
    }
    let defaults = checked_defaults(transaction, organization_id, input.defaults).await?;
    lock_roles(transaction, organization_id).await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM workspace_roles WHERE organization_id = $1 AND NOT built_in",
    )
    .bind(organization_id)
    .fetch_one(&mut **transaction)
    .await?;
    if count >= MAX_CUSTOM_ROLES {
        return Err(AppError::Conflict("role", "too_many_roles"));
    }
    let base_key = slug(&name);
    let taken = sqlx::query_scalar::<_, String>(
        "SELECT key FROM workspace_roles WHERE organization_id = $1 AND key LIKE $2 || '%'",
    )
    .bind(organization_id)
    .bind(&base_key)
    .fetch_all(&mut **transaction)
    .await?;
    let key = std::iter::once(base_key.clone())
        .chain((2..).map(|n| format!("{base_key}-{n}")))
        .find(|candidate| {
            !taken.contains(candidate)
                && !matches!(
                    candidate.as_str(),
                    "owner" | "admin" | "member" | "viewer" | "worker"
                )
        })
        .unwrap_or_else(|| format!("role-{}", Uuid::now_v7().simple()));
    if input.is_default {
        clear_default(transaction, organization_id).await?;
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO workspace_roles (
            id, organization_id, key, built_in, base_role, name, description, permissions,
            agent_limit, defaults, is_default, created_by_actor_id, updated_by_actor_id
        ) VALUES ($1, $2, $3, false, $4, $5, $6, $7, $8, $9, $10, $11, $11)
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(&key)
    .bind(base.key())
    .bind(&name)
    .bind(&description)
    .bind(permissions.keys())
    .bind(agent_limit)
    .bind(serde_json::to_value(&defaults).map_err(|_| AppError::Internal("role defaults"))?)
    .bind(input.is_default)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(name_taken)?;
    let view = read(transaction, caller, organization_id, id).await?;
    auditor
        .record(
            transaction,
            organization_id,
            "role.created",
            "role",
            id,
            json!({ "after": audit_shape(&view) }),
        )
        .await?;
    Ok(view)
}

async fn clear_default(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE workspace_roles SET is_default = false, version = version + 1, updated_at = now()
        WHERE organization_id = $1 AND is_default
        "#,
    )
    .bind(organization_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// A change to a role; what's left out stays.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RoleChange {
    pub expected_version: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    /// `null` takes the limit away (apps may do what holders may).
    #[serde(default, deserialize_with = "double_option")]
    pub agent_limit: Option<Option<Vec<String>>>,
    #[serde(default)]
    pub defaults: Option<RoleDefaults>,
    #[serde(default)]
    pub is_default: Option<bool>,
}

fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

pub(crate) async fn change(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    caller: Standing,
    organization_id: Uuid,
    role_id: Uuid,
    change: RoleChange,
) -> Result<RoleView, AppError> {
    lock_roles(transaction, organization_id).await?;
    let row = role_row(transaction, organization_id, role_id).await?;
    let base = row.base()?;
    may_define(caller, base)?;
    if let Some(expected) = change.expected_version
        && expected != row.version
    {
        return Err(AppError::VersionConflict {
            resource: "role",
            current_version: row.version,
        });
    }
    let before = role_view(caller, row)?;
    let name = change
        .name
        .as_deref()
        .map(role_name)
        .transpose()?
        .unwrap_or_else(|| before.name.clone());
    let description = change
        .description
        .as_deref()
        .map(role_description)
        .transpose()?
        .unwrap_or_else(|| before.description.clone());
    let permissions = match change.permissions.as_deref() {
        None => None,
        Some(_) if before.built_in => {
            return Err(AppError::Refused {
                field: "permissions",
                message: "Built-in roles keep their permissions. Duplicate one to change them.",
            });
        }
        Some(keys) => Some(custom_permissions(caller, base, keys)?.keys()),
    };
    let agent_limit = match change.agent_limit.as_ref() {
        None => before.agent_limit.clone(),
        Some(_) if before.key == "owner" => {
            return Err(AppError::Refused {
                field: "agentLimit",
                message: "Owners' apps can always do what owners can.",
            });
        }
        Some(keys) => limit_keys(keys.as_deref())?,
    };
    let defaults = match change.defaults {
        None => before.defaults.clone(),
        Some(defaults) => checked_defaults(transaction, organization_id, defaults).await?,
    };
    let is_default = change.is_default.unwrap_or(before.is_default);
    if is_default && !before.is_default {
        if matches!(base, MembershipRole::Owner | MembershipRole::Admin) {
            return Err(AppError::Refused {
                field: "isDefault",
                message: "New members can't start as an owner or admin.",
            });
        }
        clear_default(transaction, organization_id).await?;
    } else if !is_default && before.is_default {
        return Err(AppError::Refused {
            field: "isDefault",
            message: "Pick another role as the default instead.",
        });
    }
    sqlx::query(
        r#"
        UPDATE workspace_roles
        SET name = $3, description = $4, permissions = COALESCE($5, permissions),
            agent_limit = $6, defaults = $7, is_default = $8, updated_by_actor_id = $9,
            version = version + 1, updated_at = now()
        WHERE organization_id = $1 AND id = $2
        "#,
    )
    .bind(organization_id)
    .bind(role_id)
    .bind(&name)
    .bind(&description)
    .bind(permissions)
    .bind(agent_limit)
    .bind(serde_json::to_value(&defaults).map_err(|_| AppError::Internal("role defaults"))?)
    .bind(is_default)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(name_taken)?;
    let after = read(transaction, caller, organization_id, role_id).await?;
    auditor
        .record(
            transaction,
            organization_id,
            "role.changed",
            "role",
            role_id,
            json!({ "before": audit_shape(&before), "after": audit_shape(&after) }),
        )
        .await?;
    Ok(after)
}

pub(crate) async fn remove(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    caller: Standing,
    organization_id: Uuid,
    role_id: Uuid,
) -> Result<(), AppError> {
    lock_roles(transaction, organization_id).await?;
    let row = role_row(transaction, organization_id, role_id).await?;
    may_define(caller, row.base()?)?;
    if row.built_in {
        return Err(AppError::Conflict("role", "built_in"));
    }
    if row.is_default {
        return Err(AppError::Conflict("role", "default_role"));
    }
    let in_use = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (SELECT 1 FROM organization_memberships WHERE role_id = $1 AND enabled)
            OR EXISTS (SELECT 1 FROM project_memberships WHERE role_id = $1 AND enabled)
            OR EXISTS (SELECT 1 FROM workspace_invitations
                       WHERE role_id = $1 AND status = 'active' AND expires_at > now())
            OR EXISTS (SELECT 1 FROM project_invitations
                       WHERE role_id = $1 AND status = 'active' AND expires_at > now())
        "#,
    )
    .bind(role_id)
    .fetch_one(&mut **transaction)
    .await?;
    if in_use {
        return Err(AppError::Conflict("role", "in_use"));
    }
    // People who left keep the base role if they come back.
    for table in ["organization_memberships", "project_memberships"] {
        sqlx::query(&format!(
            "UPDATE {table} SET role_id = NULL, updated_at = now() WHERE role_id = $1"
        ))
        .bind(role_id)
        .execute(&mut **transaction)
        .await?;
    }
    let before = role_view(caller, row)?;
    sqlx::query("DELETE FROM workspace_roles WHERE organization_id = $1 AND id = $2")
        .bind(organization_id)
        .bind(role_id)
        .execute(&mut **transaction)
        .await?;
    auditor
        .record(
            transaction,
            organization_id,
            "role.removed",
            "role",
            role_id,
            json!({ "before": audit_shape(&before) }),
        )
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Assigning roles

#[derive(Debug, FromRow)]
struct TargetRow {
    kind: String,
    role: String,
    role_id: Option<Uuid>,
    role_name: Option<String>,
    enabled: bool,
    version: i64,
    agent_limit: Option<Vec<String>>,
}

async fn lock_target(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<TargetRow, AppError> {
    sqlx::query_as::<_, TargetRow>(&format!(
        r#"
        SELECT actor.kind, membership.role, membership.role_id, held.name AS role_name,
               membership.enabled, membership.version, membership.agent_limit
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
         AND actor.kind IN ('human', 'external_agent')
        {held}
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
        FOR UPDATE OF membership
        "#,
        held = held_role_join("membership.organization_id", "membership"),
    ))
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .filter(|target| target.enabled)
    .ok_or(AppError::NotFound("workspace member"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Assignment {
    pub actor_id: Uuid,
    pub role: String,
    pub role_id: Option<Uuid>,
    pub role_name: String,
    /// Projects whose role changed with it.
    pub projects_changed: usize,
    /// Projects left as they were: they own it, or managing their role
    /// there takes an owner.
    pub projects_skipped: usize,
}

/// Give someone a role in the workspace and, when asked, in every project
/// of theirs in it. Owners and admins assign; only owners assign roles based
/// on Admin or change an admin's role; nobody changes their own role, an
/// owner's (ownership moves with "Transfer ownership"), or gives a role
/// that holds more than they do. Agents only hold roles based on Member or
/// Viewer, and only people assign roles: never an agent acting as itself.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn assign(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    caller: Standing,
    organization_id: Uuid,
    actor_id: Uuid,
    role_id: Uuid,
    apply_to_projects: bool,
    expected_version: Option<i64>,
) -> Result<Assignment, AppError> {
    if caller.kind != ActorKind::Human || !caller.may(Permission::MembersManage) {
        return Err(AppError::Forbidden);
    }
    if actor_id == caller.actor_id {
        return Err(AppError::Refused {
            field: "actorId",
            message: "You can't change your own role. Ask another owner or admin.",
        });
    }
    crate::workspace::lock_workspace_memberships(transaction, organization_id).await?;
    let target = lock_target(transaction, organization_id, actor_id).await?;
    if let Some(expected) = expected_version
        && expected != target.version
    {
        return Err(AppError::VersionConflict {
            resource: "workspace_membership",
            current_version: target.version,
        });
    }
    let role = role_row(transaction, organization_id, role_id).await?;
    let base = role.base()?;
    let grant = role.grant()?;
    match (role.built_in, base) {
        (_, MembershipRole::Owner) => {
            return Err(AppError::Refused {
                field: "roleId",
                message: "Make someone an owner with Transfer ownership.",
            });
        }
        (true, MembershipRole::Viewer) => {
            return Err(AppError::Refused {
                field: "roleId",
                message: "Viewer is for projects. For the whole workspace, use a role based on Viewer, such as Read only.",
            });
        }
        _ => {}
    }
    if target.role == "owner" {
        return Err(AppError::Forbidden);
    }
    if (target.role == "admin" || base == MembershipRole::Admin) && !caller.is_owner() {
        return Err(AppError::Forbidden);
    }
    if target.kind == "external_agent"
        && !matches!(base, MembershipRole::Member | MembershipRole::Viewer)
    {
        return Err(AppError::Refused {
            field: "roleId",
            message: "Agents can hold roles based on Member or Viewer.",
        });
    }
    if !grant.permissions().is_subset_of(caller.permissions) {
        return Err(AppError::Forbidden);
    }
    let custom = (!role.built_in).then_some(role.id);
    let workspace_role = if base == MembershipRole::Admin {
        "admin"
    } else {
        "member"
    };
    let changed = target.role_id != custom || target.role != workspace_role;
    if changed {
        sqlx::query(
            r#"
            UPDATE organization_memberships
            SET role = $3, role_id = $4, role_explicit = true,
                version = version + 1, updated_at = now()
            WHERE organization_id = $1 AND actor_id = $2
            "#,
        )
        .bind(organization_id)
        .bind(actor_id)
        .bind(workspace_role)
        .bind(custom)
        .execute(&mut **transaction)
        .await?;
    }
    let (projects_changed, projects_skipped) = if apply_to_projects {
        apply_to_their_projects(transaction, caller, organization_id, actor_id, base, custom)
            .await?
    } else {
        (Vec::new(), 0)
    };
    if changed || !projects_changed.is_empty() {
        auditor
            .record(
                transaction,
                organization_id,
                "workspace_membership.role_changed",
                "actor",
                actor_id,
                json!({
                    "previousRole": target.role,
                    "previousRoleId": target.role_id,
                    "previousRoleName": target.role_name,
                    "role": workspace_role,
                    "roleId": custom,
                    "roleName": role.name,
                    "projects": projects_changed,
                }),
            )
            .await?;
    }
    Ok(Assignment {
        actor_id,
        role: workspace_role.to_owned(),
        role_id: custom,
        role_name: role.name,
        projects_changed: projects_changed.len(),
        projects_skipped,
    })
}

async fn apply_to_their_projects(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Standing,
    organization_id: Uuid,
    actor_id: Uuid,
    base: MembershipRole,
    custom: Option<Uuid>,
) -> Result<(Vec<Uuid>, usize), AppError> {
    let memberships = sqlx::query_as::<_, (Uuid, String, Option<Uuid>)>(
        r#"
        SELECT membership.project_id, membership.role, membership.role_id
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        WHERE project.organization_id = $1 AND membership.actor_id = $2
          AND membership.enabled AND project.kind <> 'direct' AND project.archived_at IS NULL
        ORDER BY membership.project_id
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut changed = Vec::new();
    let mut skipped = 0;
    for (project_id, role, role_id) in memberships {
        // Project owners stay owners, and only owners change an admin's role.
        if role == "owner" || (role == "admin" && !caller.is_owner()) {
            skipped += 1;
            continue;
        }
        if role == base.key() && role_id == custom {
            continue;
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("m7-membership:{project_id}"))
            .execute(&mut **transaction)
            .await?;
        let version = sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE project_memberships
            SET role = $3, role_id = $4, version = version + 1, updated_at = now()
            WHERE project_id = $1 AND actor_id = $2
            RETURNING version
            "#,
        )
        .bind(project_id)
        .bind(actor_id)
        .bind(base.key())
        .bind(custom)
        .fetch_one(&mut **transaction)
        .await?;
        enqueue_typed(
            transaction,
            project_id,
            OutboxEventPayload::ProjectMembershipUpdated(ProjectMembershipUpdatedPayload {
                project_id,
                actor_id,
                version,
            }),
        )
        .await?;
        changed.push(project_id);
    }
    Ok((changed, skipped))
}

/// Set what apps and agents acting for one person may do in the workspace,
/// on top of their roles' limits. People set their own; owners and admins
/// set others' (only owners an admin's). Owners' apps aren't limited by
/// anyone else.
pub(crate) async fn set_agent_limit(
    transaction: &mut Transaction<'_, Postgres>,
    auditor: &Auditor<'_>,
    caller: Standing,
    organization_id: Uuid,
    actor_id: Uuid,
    limit: Option<Vec<String>>,
) -> Result<Option<Vec<&'static str>>, AppError> {
    if caller.kind != ActorKind::Human {
        return Err(AppError::Forbidden);
    }
    let target = lock_target(transaction, organization_id, actor_id).await?;
    if target.kind != "human" {
        return Err(AppError::Refused {
            field: "actorId",
            message: "Agents with their own identity follow their role; limits are for people's apps.",
        });
    }
    if actor_id != caller.actor_id
        && (!caller.may(Permission::MembersManage)
            || (matches!(target.role.as_str(), "owner" | "admin") && !caller.is_owner())
            || target.role == "owner")
    {
        return Err(AppError::Forbidden);
    }
    let keys = limit_keys(limit.as_deref())?;
    let before = target
        .agent_limit
        .as_deref()
        .map(|keys| PermissionSet::from_keys(keys).keys());
    if before == keys {
        return Ok(keys);
    }
    sqlx::query(
        r#"
        UPDATE organization_memberships SET agent_limit = $3, version = version + 1, updated_at = now()
        WHERE organization_id = $1 AND actor_id = $2
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(&keys)
    .execute(&mut **transaction)
    .await?;
    auditor
        .record(
            transaction,
            organization_id,
            "workspace_membership.agent_limit_changed",
            "actor",
            actor_id,
            json!({ "before": before, "after": keys }),
        )
        .await?;
    Ok(keys)
}

/// On arriving in a project, join the private channels their role names,
/// when the person adding them is in that channel.
pub(crate) async fn apply_arrival_defaults(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
    added_by: Uuid,
) -> Result<(), AppError> {
    sqlx::query(&format!(
        r#"
        INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
        SELECT $1, conversation.id, $2, $3
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        {held}
        JOIN conversations conversation ON conversation.project_id = membership.project_id
         AND conversation.kind = 'channel' AND conversation.visibility = 'members'
         AND conversation.archived_at IS NULL
         AND lower(conversation.name) IN (
             SELECT lower(wanted.channel) FROM jsonb_array_elements_text(
                 CASE WHEN jsonb_typeof(held.defaults->'channels') = 'array'
                      THEN held.defaults->'channels' ELSE '[]'::jsonb END) AS wanted (channel))
        WHERE membership.project_id = $1 AND membership.actor_id = $2 AND membership.enabled
          AND $2 <> $3
          AND EXISTS (SELECT 1 FROM project_memberships adder
                      WHERE adder.project_id = $1 AND adder.actor_id = $3 AND adder.enabled)
          AND conversation_visible_to(conversation.id, $3)
        ON CONFLICT DO NOTHING
        "#,
        held = held_role_join("project.organization_id", "membership"),
    ))
    .bind(project_id)
    .bind(actor_id)
    .bind(added_by)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Effective permissions

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopePermissions {
    pub role: String,
    pub role_id: Option<Uuid>,
    pub role_name: String,
    /// What they may do themselves.
    pub permissions: Vec<&'static str>,
    /// What apps and agents acting for them may do.
    pub for_apps: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionsView {
    pub actor_id: Uuid,
    pub workspace: ScopePermissions,
    pub project: Option<ScopePermissions>,
    /// Their own limit on their apps, if they set one.
    pub personal_agent_limit: Option<Vec<&'static str>>,
}

#[derive(Debug, FromRow)]
struct HeldRow {
    kind: String,
    role: String,
    role_id: Option<Uuid>,
    role_name: Option<String>,
    role_permissions: Option<Vec<String>>,
    role_limit: Option<Vec<String>>,
    personal_limit: Option<Vec<String>>,
}

/// What `actor_id` may do in the workspace and, if named, one of its
/// projects; and what their apps may.
pub(crate) async fn permissions_of(
    connection: &mut PgConnection,
    organization_id: Uuid,
    actor_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<PermissionsView, AppError> {
    let workspace = sqlx::query_as::<_, HeldRow>(&format!(
        r#"
        SELECT actor.kind, membership.role, membership.role_id, held.name AS role_name,
               workspace_role_permissions(membership.role_id) AS role_permissions,
               held.agent_limit AS role_limit, membership.agent_limit AS personal_limit
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        {held}
        WHERE membership.organization_id = $1 AND membership.actor_id = $2 AND membership.enabled
        "#,
        held = held_role_join("membership.organization_id", "membership"),
    ))
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("workspace member"))?;
    let kind = if workspace.kind == "human" {
        ActorKind::Human
    } else {
        ActorKind::ExternalAgent
    };
    let workspace_limit = stored_limit(workspace.role_limit.as_deref())
        .intersect(stored_limit(workspace.personal_limit.as_deref()))
        .union(PermissionSet::required());
    let scope = |row: &HeldRow, limit: PermissionSet| -> Result<ScopePermissions, AppError> {
        let grant = RoleGrant::stored(&row.role, row.role_permissions.as_deref())?;
        let own = effective(grant, None, kind);
        Ok(ScopePermissions {
            role: row.role.clone(),
            role_id: row.role_id,
            role_name: row.role_name.clone().unwrap_or_else(|| row.role.clone()),
            permissions: own.keys(),
            for_apps: if kind == ActorKind::Human {
                effective(grant, Some(limit), kind).keys()
            } else {
                own.keys()
            },
        })
    };
    let project = match project_id {
        None => None,
        Some(project_id) => {
            let row = sqlx::query_as::<_, HeldRow>(&format!(
                r#"
                SELECT actor.kind, membership.role, membership.role_id, held.name AS role_name,
                       workspace_role_permissions(membership.role_id) AS role_permissions,
                       held.agent_limit AS role_limit, NULL::text[] AS personal_limit
                FROM project_memberships membership
                JOIN projects project ON project.id = membership.project_id
                 AND project.organization_id = $1
                JOIN actors actor ON actor.id = membership.actor_id
                {held}
                WHERE membership.project_id = $3 AND membership.actor_id = $2 AND membership.enabled
                "#,
                held = held_role_join("project.organization_id", "membership"),
            ))
            .bind(organization_id)
            .bind(actor_id)
            .bind(project_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or(AppError::NotFound("project"))?;
            let limit = stored_limit(row.role_limit.as_deref())
                .intersect(workspace_limit)
                .union(PermissionSet::required());
            Some(scope(&row, limit)?)
        }
    };
    Ok(PermissionsView {
        actor_id,
        workspace: scope(&workspace, workspace_limit)?,
        project,
        personal_agent_limit: workspace
            .personal_limit
            .as_deref()
            .map(|keys| PermissionSet::from_keys(keys).keys()),
    })
}

/// Whether `caller` may see what someone else may do: they manage members,
/// or it's themselves.
pub(crate) fn may_read_permissions_of(caller: Standing, actor_id: Uuid) -> bool {
    actor_id == caller.actor_id || caller.may(Permission::MembersView)
}

// ---------------------------------------------------------------------------
// HTTP

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RolesResponse {
    roles: Vec<RoleView>,
    modules: Vec<ModuleEntry>,
    permissions: Vec<PermissionEntry>,
    starting_points: Vec<StartingPoint>,
    /// Whether the caller may make and change roles.
    can_manage: bool,
    /// The caller's own workspace permissions.
    your_permissions: Vec<&'static str>,
}

async fn list_roles(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<RolesResponse>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let caller = standing(&mut connection, organization_id, principal.actor_id).await?;
    let roles = list(&mut connection, caller, organization_id).await?;
    let (modules, permissions) = catalogue();
    Ok(Json(RolesResponse {
        roles,
        modules,
        permissions,
        starting_points: starting_points(),
        can_manage: caller.may(Permission::RolesManage),
        your_permissions: caller.permissions.keys(),
    }))
}

async fn create_role_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<RoleInput>,
) -> Result<(StatusCode, Json<RoleView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let caller = standing(&mut transaction, organization_id, principal.actor_id).await?;
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    let view = create(&mut transaction, &auditor, caller, organization_id, input).await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn change_role_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, role_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<RoleChange>,
) -> Result<Json<RoleView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version.is_none() {
        return Err(AppError::Validation("expectedVersion"));
    }
    let mut transaction = state.pool.begin().await?;
    let caller = standing(&mut transaction, organization_id, principal.actor_id).await?;
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    let view = change(
        &mut transaction,
        &auditor,
        caller,
        organization_id,
        role_id,
        request,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(view))
}

async fn remove_role_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, role_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let caller = standing(&mut transaction, organization_id, principal.actor_id).await?;
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    remove(&mut transaction, &auditor, caller, organization_id, role_id).await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignRequest {
    role_id: Uuid,
    #[serde(default)]
    apply_to_projects: bool,
    expected_version: i64,
}

async fn assign_role_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, actor_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<AssignRequest>,
) -> Result<Json<Assignment>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let caller = standing(&mut transaction, organization_id, principal.actor_id).await?;
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    let assignment = assign(
        &mut transaction,
        &auditor,
        caller,
        organization_id,
        actor_id,
        request.role_id,
        request.apply_to_projects,
        Some(request.expected_version),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(assignment))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentLimitRequest {
    /// `null`: their apps may do what they may.
    permissions: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentLimitResponse {
    permissions: Option<Vec<&'static str>>,
}

async fn set_agent_limit_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, actor_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<AgentLimitRequest>,
) -> Result<Json<AgentLimitResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let caller = standing(&mut transaction, organization_id, principal.actor_id).await?;
    let auditor = Auditor::Browser {
        headers: &headers,
        principal: &principal,
    };
    let permissions = set_agent_limit(
        &mut transaction,
        &auditor,
        caller,
        organization_id,
        actor_id,
        request.permissions,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(AgentLimitResponse { permissions }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PermissionsQuery {
    #[serde(default)]
    actor_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
}

async fn read_permissions(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<PermissionsQuery>,
) -> Result<Json<PermissionsView>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let caller = standing(&mut connection, organization_id, principal.actor_id).await?;
    let actor_id = query.actor_id.unwrap_or(principal.actor_id);
    if !may_read_permissions_of(caller, actor_id) {
        return Err(AppError::NotFound("workspace member"));
    }
    if let Some(project_id) = query.project_id {
        // A project the caller can't open is as good as missing.
        let visible = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM project_memberships membership
                JOIN projects project ON project.id = membership.project_id
                WHERE membership.project_id = $1 AND membership.actor_id = $2
                  AND membership.enabled AND project.organization_id = $3
            )
            "#,
        )
        .bind(project_id)
        .bind(principal.actor_id)
        .bind(organization_id)
        .fetch_one(&mut *connection)
        .await?;
        if !visible {
            return Err(AppError::NotFound("project"));
        }
    }
    Ok(Json(
        permissions_of(&mut connection, organization_id, actor_id, query.project_id).await?,
    ))
}

/// Built-in role keys and what each holds, for the equivalence test.
#[cfg(test)]
pub(crate) fn built_in_table() -> std::collections::HashMap<&'static str, PermissionSet> {
    [
        MembershipRole::Owner,
        MembershipRole::Admin,
        MembershipRole::Member,
        MembershipRole::Viewer,
    ]
    .into_iter()
    .map(|role| (role.key(), PermissionSet::built_in(role)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(role: MembershipRole) -> Standing {
        Standing {
            actor_id: Uuid::nil(),
            kind: ActorKind::Human,
            role,
            permissions: PermissionSet::built_in(role),
        }
    }

    #[test]
    fn keys_are_slugs_of_the_name() {
        assert_eq!(slug("Family member"), "family-member");
        assert_eq!(slug("  Read-only!! "), "read-only");
        assert_eq!(slug("3D artists"), "role-3d-artists");
        assert_eq!(slug("Ünïcode"), "n-code");
    }

    #[test]
    fn custom_roles_stay_within_their_base_and_the_granter() {
        let member_keys = vec!["tasks.edit".to_owned(), "code.manage".to_owned()];
        assert!(matches!(
            custom_permissions(
                caller(MembershipRole::Owner),
                MembershipRole::Member,
                &member_keys
            ),
            Err(AppError::Refused { .. })
        ));
        let fine = custom_permissions(
            caller(MembershipRole::Admin),
            MembershipRole::Member,
            &["tasks.edit".to_owned()],
        )
        .expect("valid in the test");
        assert!(
            fine.contains(Permission::ProjectsView),
            "required is always held"
        );
        // Someone holding a narrowed admin role can't hand out what they lack.
        let narrowed = Standing {
            permissions: PermissionSet::built_in(MembershipRole::Admin)
                .without(Permission::CodeManage),
            ..caller(MembershipRole::Admin)
        };
        assert!(matches!(
            custom_permissions(narrowed, MembershipRole::Admin, &["code.manage".to_owned()]),
            Err(AppError::Forbidden)
        ));
        assert!(matches!(
            custom_permissions(
                caller(MembershipRole::Owner),
                MembershipRole::Member,
                &["nope".to_owned()]
            ),
            Err(AppError::Validation(_))
        ));
    }

    #[test]
    fn only_owners_define_admin_roles_and_agents_never_do() {
        assert!(may_define(caller(MembershipRole::Owner), MembershipRole::Admin).is_ok());
        assert!(may_define(caller(MembershipRole::Admin), MembershipRole::Admin).is_err());
        assert!(may_define(caller(MembershipRole::Admin), MembershipRole::Member).is_ok());
        assert!(may_define(caller(MembershipRole::Member), MembershipRole::Viewer).is_err());
        let agent = Standing {
            kind: ActorKind::ExternalAgent,
            ..caller(MembershipRole::Owner)
        };
        assert!(may_define(agent, MembershipRole::Member).is_err());
    }

    #[test]
    fn limits_narrow_but_never_take_reading_away() {
        let grant = RoleGrant::built_in(MembershipRole::Member);
        let limited = effective(
            grant,
            Some(PermissionSet::EMPTY.with(Permission::TasksView)),
            ActorKind::Human,
        );
        assert!(limited.contains(Permission::ProjectsView));
        assert!(limited.contains(Permission::TasksView));
        assert!(!limited.contains(Permission::TasksEdit));
        let admin = effective(
            RoleGrant::built_in(MembershipRole::Admin),
            None,
            ActorKind::ExternalAgent,
        );
        assert!(!admin.contains(Permission::RolesManage));
        assert!(!admin.contains(Permission::AgentsManage));
    }

    #[test]
    fn starting_points_are_valid_custom_roles() {
        let owner = caller(MembershipRole::Owner);
        for point in starting_points() {
            let base = MembershipRole::parse(point.base_role).expect("valid in the test");
            let keys = point
                .permissions
                .iter()
                .map(|key| (*key).to_owned())
                .collect::<Vec<_>>();
            let permissions = custom_permissions(owner, base, &keys).expect("valid in the test");
            assert_eq!(permissions.keys(), point.permissions, "{}", point.key);
            for key in point.agent_limit.iter().flatten() {
                assert!(Permission::from_key(key).is_some(), "{key}");
            }
            if let Some(views) = &point.defaults.pinned_views {
                for view in views {
                    assert!(BUILTIN_VIEWS.iter().any(|builtin| builtin.key == view));
                }
            }
            for part in &point.defaults.home_hidden {
                assert!(layouts::part_is_known(part), "{part}");
            }
        }
    }

    #[test]
    fn the_web_app_knows_every_permission() {
        let web = include_str!("../../web/src/settings/workspace/roles.ts");
        for info in PERMISSIONS {
            assert!(
                web.contains(&format!("'{}'", info.key)),
                "add '{}' to PERMISSION_KEYS in apps/web/src/settings/workspace/roles.ts",
                info.key
            );
        }
    }

    #[test]
    fn the_built_in_table_matches_the_old_roles() {
        let table = built_in_table();
        assert_eq!(table["owner"], table["admin"]);
        assert!(table["member"].is_subset_of(table["admin"]));
        assert!(table["viewer"].is_subset_of(table["member"]));
    }
}

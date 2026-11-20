//! S4: manage the workspace.
//!
//! A workspace (an `organizations` row) can be renamed, lists its people and
//! agents with their workspace roles, invites people to the workspace rather
//! than one project, hands ownership to someone else, and lets people leave.
//! A workspace role is set here and nowhere else: project roles never
//! change it (K1.19).
//!
//! Who may do what is the caller's workspace role (`standing_of`), one rule
//! for people and agents alike:
//! - any member reads the workspace, and its member list with "See the
//!   workspace member directory";
//! - "Manage settings" renames it, "Read the audit log" reads its log,
//!   "Invite people" invites and "Change roles and remove people" manages
//!   members; an admin-tier caller only those below admin;
//! - owners give admin-tier roles and transfer ownership;
//! - anyone leaves, except the last owner.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, patch, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

pub mod lifecycle;
pub mod ops;
pub mod reads;
pub mod slug;

use crate::{
    AppState,
    auth::{
        AuthenticatedActor, SessionIdentity, establish_session_in_transaction,
        load_authenticated_actor, normalize_login_name, password, require_allowed_origin,
        require_authenticated_write, respond_with_established_session,
    },
    error::{ApiJson, ApiPath, AppError},
    invitations::engine::{self, Family},
    outbox::{OutboxEventPayload, ProjectMembershipUpdatedPayload, enqueue_typed},
    policy::{MembershipRole, Permission},
    roles::Standing,
};

const MAX_EXPIRY_HOURS: i32 = 30 * 24;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}",
            get(read_workspace).patch(rename_workspace),
        )
        .route(
            "/api/v1/organizations/{organization_id}/members",
            get(list_members),
        )
        .route(
            "/api/v1/organizations/{organization_id}/members/{actor_id}",
            patch(update_member),
        )
        .route(
            "/api/v1/organizations/{organization_id}/transfer-ownership",
            post(transfer_ownership),
        )
        .route(
            "/api/v1/organizations/{organization_id}/leave",
            post(leave_workspace),
        )
        .route(
            "/api/v1/organizations/{organization_id}/invitations",
            get(list_invitations).post(issue_invitation),
        )
        .route(
            "/api/v1/workspace-invitations/{invitation_id}/revoke",
            post(revoke_invitation),
        )
        .route(
            "/api/v1/workspace-invitations/resolve",
            post(resolve_invitation),
        )
        .route(
            "/api/v1/workspace-invitations/accept-new",
            post(accept_new_user),
        )
        .route(
            "/api/v1/workspace-invitations/accept-existing",
            post(accept_existing_user),
        )
        .route(
            "/api/v1/account/default-workspace",
            put(set_default_workspace),
        )
}

// ---------------------------------------------------------------------------
// Roles

/// The caller's standing in the workspace (ruling R19: one rule for
/// everyone): their role's tier and what it lets them do, a custom role and
/// an app's agent limit included. `lock` holds their membership row until
/// the transaction ends.
pub(crate) async fn standing_of(
    connection: &mut sqlx::PgConnection,
    actor_id: Uuid,
    organization_id: Uuid,
    lock: bool,
) -> Result<Standing, AppError> {
    if lock {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT membership.actor_id
            FROM organization_memberships membership
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND membership.enabled
            FOR UPDATE OF membership
            "#,
        )
        .bind(organization_id)
        .bind(actor_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    }
    crate::roles::standing(connection, organization_id, actor_id).await
}

/// What the caller may do to one member.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberCapabilities {
    pub change_role: bool,
    pub remove: bool,
    /// Give them another role (AWD-74): owners for anyone but owners,
    /// admins for members.
    pub assign_role: bool,
    /// Limit what apps acting for them may do.
    pub limit_apps: bool,
}

/// The rules for managing one member, in one place so the list and the
/// write agree. Whoever may change roles and remove people
/// (`members.manage`) does so for anyone below an owner; an admin-tier
/// caller only for those below admin, and nobody for an owner or
/// themselves. `target_connects_apps` is whether apps act for the target.
pub(crate) fn member_capabilities(
    caller: Standing,
    caller_is_target: bool,
    target_tier: MembershipRole,
    target_connects_apps: bool,
    target_enabled: bool,
) -> MemberCapabilities {
    if caller_is_target && target_enabled {
        return MemberCapabilities {
            limit_apps: target_connects_apps,
            ..MemberCapabilities::default()
        };
    }
    if caller_is_target
        || !target_enabled
        || target_tier == MembershipRole::Owner
        || !caller.may(Permission::MembersManage)
    {
        return MemberCapabilities::default();
    }
    if caller.role == MembershipRole::Owner {
        return MemberCapabilities {
            change_role: true,
            remove: true,
            assign_role: true,
            limit_apps: target_connects_apps,
        };
    }
    let below = target_tier != MembershipRole::Admin;
    MemberCapabilities {
        change_role: false,
        remove: below,
        assign_role: below,
        limit_apps: below && target_connects_apps,
    }
}

// ---------------------------------------------------------------------------
// The workspace

#[derive(Debug, FromRow)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
    slug: String,
    version: i64,
    created_at: DateTime<Utc>,
    member_count: i64,
    people_count: i64,
    agent_count: i64,
    project_count: i64,
    owner_count: i64,
    lifecycle: String,
    delete_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceView {
    id: Uuid,
    name: String,
    slug: String,
    version: i64,
    created_at: DateTime<Utc>,
    role: &'static str,
    /// People and agents together; `people_count` and `agent_count` split it.
    member_count: i64,
    people_count: i64,
    agent_count: i64,
    project_count: i64,
    /// `active`, `importing`, `suspended` or `deleting` (H1.24).
    lifecycle: String,
    /// When a deleting workspace goes for good.
    delete_after: Option<DateTime<Utc>>,
    capabilities: WorkspaceCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceCapabilities {
    rename: bool,
    manage_members: bool,
    transfer_ownership: bool,
    leave: bool,
    view_audit_log: bool,
    /// Holds "Delete the workspace" (H1.24): may schedule its deletion, or
    /// keep it.
    delete: bool,
}

/// The workspace as `actor` sees it. Every handler that returns a workspace
/// view goes through here, so its capabilities agree.
async fn load_workspace(
    connection: &mut sqlx::PgConnection,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<WorkspaceView, AppError> {
    let standing = standing_of(&mut *connection, actor_id, organization_id, false).await?;
    let row = sqlx::query_as::<_, WorkspaceRow>(
        r#"
        SELECT organization.id, organization.name, organization.slug, organization.version,
               organization.created_at,
               (SELECT count(*) FROM organization_memberships membership
                 JOIN actors actor ON actor.id = membership.actor_id
                 WHERE membership.organization_id = organization.id AND membership.enabled
                   AND actor.kind IN ('human', 'external_agent', 'integration')) AS member_count, -- // kind-check: mechanical (counts accounts that sign in, as the Members page lists them)
               (SELECT count(*) FROM organization_memberships membership
                 JOIN actors actor ON actor.id = membership.actor_id
                 WHERE membership.organization_id = organization.id AND membership.enabled
                   AND actor.kind = 'human') AS people_count, -- // kind-check: mechanical (a display count)
               (SELECT count(*) FROM organization_memberships membership
                 JOIN actors actor ON actor.id = membership.actor_id
                 WHERE membership.organization_id = organization.id AND membership.enabled
                   AND actor.kind = 'external_agent') AS agent_count, -- // kind-check: mechanical (a display count)
               (SELECT count(*) FROM projects project
                 WHERE project.organization_id = organization.id
                   AND project.archived_at IS NULL AND project.kind <> 'workspace') AS project_count,
               (SELECT count(*) FROM organization_memberships membership
                 WHERE membership.organization_id = organization.id AND membership.enabled
                   AND membership.role = $2) AS owner_count,
               organization.lifecycle, organization.delete_after
        FROM organizations organization
        WHERE organization.id = $1
        "#,
    )
    .bind(organization_id)
    .bind(MembershipRole::Owner.key())
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("workspace"))?;
    let owner = standing.role == MembershipRole::Owner;
    Ok(WorkspaceView {
        id: row.id,
        name: row.name,
        slug: row.slug,
        version: row.version,
        created_at: row.created_at,
        role: standing.role.key(),
        member_count: row.member_count,
        people_count: row.people_count,
        agent_count: row.agent_count,
        project_count: row.project_count,
        lifecycle: row.lifecycle,
        delete_after: row.delete_after,
        capabilities: WorkspaceCapabilities {
            rename: standing.may(Permission::SettingsManage),
            manage_members: standing.may(Permission::MembersManage),
            transfer_ownership: owner,
            leave: !owner || row.owner_count > 1,
            view_audit_log: standing.may(Permission::SettingsAudit),
            delete: standing.may(Permission::WorkspaceDelete),
        },
    })
}

async fn read_workspace(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<WorkspaceView>, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let view = load_workspace(&mut connection, organization_id, principal.actor_id).await?;
    Ok(Json(view))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RenameRequest {
    #[serde(default)]
    name: Option<String>,
    /// A new address (K4.5); the old one keeps working for members.
    #[serde(default)]
    slug: Option<String>,
    expected_version: i64,
}

pub(crate) fn normalize_workspace_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("workspace name"));
    }
    Ok(name.to_owned())
}

async fn rename_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RenameRequest>,
) -> Result<Json<WorkspaceView>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let renamed = rename_workspace_in(
        &mut transaction,
        principal.actor_id,
        organization_id,
        WorkspaceChange {
            name: request.name.as_deref(),
            slug: request.slug.as_deref(),
        },
        request.expected_version,
    )
    .await?;
    if let Some(metadata) = renamed {
        audit(
            &mut transaction,
            &headers,
            &principal,
            organization_id,
            "workspace.renamed",
            "workspace",
            Some(organization_id),
            metadata,
        )
        .await?;
    }
    let view = load_workspace(&mut transaction, organization_id, principal.actor_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

/// What a workspace update changes: its name, its slug, or both.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WorkspaceChange<'a> {
    pub name: Option<&'a str>,
    pub slug: Option<&'a str>,
}

/// Rename a workspace or change its slug, for someone who may manage its
/// settings. Returns the audit metadata when something changed (the caller records the row). A changed slug
/// keeps the old one as a retired slug (K4.5).
pub(crate) async fn rename_workspace_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    organization_id: Uuid,
    change: WorkspaceChange<'_>,
    expected_version: i64,
) -> Result<Option<serde_json::Value>, AppError> {
    let name = change.name.map(normalize_workspace_name).transpose()?;
    let slug = change.slug.map(slug::check_new).transpose()?;
    if name.is_none() && slug.is_none() {
        return Err(AppError::Validation("workspace name"));
    }
    if !standing_of(transaction, actor_id, organization_id, true)
        .await?
        .may(Permission::SettingsManage)
    {
        return Err(AppError::Forbidden);
    }
    let previous = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT name, slug, version FROM organizations WHERE id = $1 FOR UPDATE",
    )
    .bind(organization_id)
    .fetch_one(&mut **transaction)
    .await?;
    if previous.2 != expected_version {
        return Err(AppError::VersionConflict {
            resource: "workspace",
            current_version: previous.2,
        });
    }
    let mut metadata = serde_json::Map::new();
    if let Some(name) = name.filter(|name| *name != previous.0) {
        sqlx::query("UPDATE organizations SET name = $2, updated_at = now() WHERE id = $1")
            .bind(organization_id)
            .bind(&name)
            .execute(&mut **transaction)
            .await?;
        metadata.insert("previousName".into(), previous.0.clone().into());
        metadata.insert("name".into(), name.into());
    }
    if let Some(slug) = slug.filter(|slug| *slug != previous.1) {
        slug::rename(transaction, organization_id, &slug).await?;
        metadata.insert("previousSlug".into(), previous.1.clone().into());
        metadata.insert("slug".into(), slug.into());
    }
    if metadata.is_empty() {
        return Ok(None);
    }
    sqlx::query("UPDATE organizations SET version = version + 1 WHERE id = $1")
        .bind(organization_id)
        .execute(&mut **transaction)
        .await?;
    Ok(Some(serde_json::Value::Object(metadata)))
}

// ---------------------------------------------------------------------------
// Members

#[derive(Debug, FromRow)]
struct MemberRow {
    actor_id: Uuid,
    kind: String,
    display_name: String,
    handle: String,
    login_name: Option<String>,
    role: String,
    role_id: Option<Uuid>,
    role_name: Option<String>,
    agent_limit: Option<Vec<String>>,
    enabled: bool,
    version: i64,
    joined_at: DateTime<Utc>,
    project_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberView {
    actor_id: Uuid,
    kind: String,
    display_name: String,
    handle: String,
    login_name: Option<String>,
    role: String,
    /// The custom role they hold (AWD-74), and the name of the role they
    /// hold, custom or built-in.
    role_id: Option<Uuid>,
    role_name: String,
    /// What apps acting for them may do in the workspace, when limited.
    agent_limit: Option<Vec<String>>,
    enabled: bool,
    version: i64,
    joined_at: DateTime<Utc>,
    project_count: i64,
    is_you: bool,
    /// Whether the workspace can be handed to them: an owner has to sign in
    /// to use it, which an app account can't.
    may_own: bool,
    capabilities: MemberCapabilities,
}

#[derive(Debug, Serialize)]
struct MembersResponse {
    members: Vec<MemberView>,
}

const MEMBER_COLUMNS: &str = r#"
    membership.actor_id, actor.kind,
    COALESCE(profile.display_name, user_account.display_name, 'Unknown') AS display_name,
    COALESCE(profile.handle, user_account.login_name, 'unknown') AS handle,
    user_account.login_name, membership.role, custom_role_id(membership.role_id) AS role_id, held.name AS role_name,
    membership.agent_limit, membership.enabled, membership.version,
    membership.created_at AS joined_at,
    (SELECT count(*) FROM project_memberships project_membership
      JOIN projects project ON project.id = project_membership.project_id
      WHERE project.organization_id = membership.organization_id
        AND project.archived_at IS NULL AND project.kind <> 'workspace'
        AND project_membership.actor_id = membership.actor_id
        AND project_membership.enabled) AS project_count
"#;

const MEMBER_FROM: &str = r#"
    FROM organization_memberships membership
    JOIN actors actor ON actor.id = membership.actor_id
     AND actor.kind IN ('human', 'external_agent', 'integration') -- // kind-check: mechanical (accounts that sign in: people, agents and apps; the system and workers aren't members to list)
    LEFT JOIN organization_actor_profiles profile
      ON profile.organization_id = membership.organization_id
     AND profile.actor_id = membership.actor_id
    LEFT JOIN users user_account ON user_account.actor_id = membership.actor_id
    LEFT JOIN workspace_roles held ON held.organization_id = membership.organization_id
     AND (held.id = membership.role_id OR (membership.role_id IS NULL AND held.key = membership.role))
"#;

/// Whether an account of this kind signs in itself. An app account acts
/// only through the hosts and connections it was made for.
fn signs_in(kind: &str) -> bool {
    kind != "integration" // kind-check: mechanical (whether the account can sign in at all)
}

fn member_view(
    row: MemberRow,
    caller: Standing,
    principal: &AuthenticatedActor,
) -> Result<MemberView, AppError> {
    let target_tier =
        MembershipRole::parse(&row.role).ok_or(AppError::Internal("stored workspace role"))?;
    let is_you = row.actor_id == principal.actor_id;
    let connects_apps = row.kind == "human"; // kind-check: mechanical (apps act for the person who connected them)
    let capabilities = member_capabilities(caller, is_you, target_tier, connects_apps, row.enabled);
    let may_own = signs_in(&row.kind);
    Ok(MemberView {
        actor_id: row.actor_id,
        kind: row.kind,
        display_name: row.display_name,
        handle: row.handle,
        login_name: row.login_name,
        role_name: row.role_name.unwrap_or_else(|| row.role.clone()),
        role: row.role,
        role_id: row.role_id,
        agent_limit: (caller.may(Permission::MembersManage) || is_you)
            .then_some(row.agent_limit)
            .flatten(),
        may_own,
        enabled: row.enabled,
        version: row.version,
        joined_at: row.joined_at,
        project_count: row.project_count,
        is_you,
        capabilities,
    })
}

async fn list_members(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<MembersResponse>, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let caller = standing_of(&mut connection, principal.actor_id, organization_id, false).await?;
    // "See the workspace member directory", by the caller's workspace role:
    // every built-in role holds it; a custom role only when an owner or an
    // admin who manages roles gives it (Settings, Roles). Without it the
    // list is only the caller themselves.
    let sees_members = caller.may(Permission::DirectoryView);
    // Removed members are listed only for those who manage the workspace.
    let rows = sqlx::query_as::<_, MemberRow>(&format!(
        r#"
        SELECT {MEMBER_COLUMNS} {MEMBER_FROM}
        WHERE membership.organization_id = $1 AND (membership.enabled OR $2)
          AND ($3 OR membership.actor_id = $4)
        ORDER BY membership.enabled DESC,
                 CASE membership.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END,
                 lower(COALESCE(profile.display_name, user_account.display_name, '')),
                 membership.actor_id
        LIMIT 1000
        "#
    ))
    .bind(organization_id)
    .bind(caller.may(Permission::MembersManage))
    .bind(sees_members)
    .bind(principal.actor_id)
    .fetch_all(&mut *connection)
    .await?;
    Ok(Json(MembersResponse {
        members: rows
            .into_iter()
            .map(|row| member_view(row, caller, &principal))
            .collect::<Result<_, _>>()?,
    }))
}

async fn load_member(
    connection: &mut sqlx::PgConnection,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<MemberRow, AppError> {
    sqlx::query_as::<_, MemberRow>(&format!(
        "SELECT {MEMBER_COLUMNS} {MEMBER_FROM} WHERE membership.organization_id = $1 AND membership.actor_id = $2"
    ))
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("workspace member"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateMemberRequest {
    expected_version: i64,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// One lock per workspace for membership changes, so two removals or a
/// removal and an ownership change never interleave.
pub(crate) async fn lock_workspace_memberships(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("w8-workspace-membership:{organization_id}"))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn update_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, actor_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateMemberRequest>,
) -> Result<Json<MemberView>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let next_role = match request.role.as_deref() {
        None => None,
        Some("admin") => Some("admin"),
        Some("member") => Some("member"),
        Some(_) => return Err(AppError::Validation("workspace role")),
    };
    match (next_role, request.enabled) {
        (None, Some(false)) | (Some(_), None) => {}
        _ => return Err(AppError::Validation("workspace member update")),
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    lock_workspace_memberships(&mut transaction, organization_id).await?;
    let caller = standing_of(&mut transaction, principal.actor_id, organization_id, true).await?;
    if !caller.may(Permission::MembersManage) {
        return Err(AppError::NotFound("workspace member"));
    }
    let target = lock_member(&mut transaction, organization_id, actor_id).await?;
    if target.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "workspace_membership",
            current_version: target.version,
        });
    }
    let connects_apps = target.kind == "human"; // kind-check: mechanical (apps act for the person who connected them)
    let capabilities = member_capabilities(
        caller,
        actor_id == principal.actor_id,
        MembershipRole::parse(&target.role).ok_or(AppError::Internal("stored workspace role"))?,
        connects_apps,
        target.enabled,
    );
    if let Some(role) = next_role {
        if !capabilities.change_role {
            return Err(AppError::Forbidden);
        }
        if role != target.role {
            sqlx::query(
                r#"
                UPDATE organization_memberships
                SET role = $3, role_explicit = true, version = version + 1, updated_at = now()
                WHERE organization_id = $1 AND actor_id = $2
                "#,
            )
            .bind(organization_id)
            .bind(actor_id)
            .bind(role)
            .execute(&mut *transaction)
            .await?;
            audit(
                &mut transaction,
                &headers,
                &principal,
                organization_id,
                "workspace_membership.role_changed",
                "actor",
                Some(actor_id),
                serde_json::json!({ "previousRole": target.role, "role": role }),
            )
            .await?;
        }
    } else {
        if !capabilities.remove {
            return Err(AppError::Forbidden);
        }
        remove_from_workspace(
            &mut transaction,
            organization_id,
            actor_id,
            principal.actor_id,
        )
        .await?;
        audit(
            &mut transaction,
            &headers,
            &principal,
            organization_id,
            "workspace_membership.removed",
            "actor",
            Some(actor_id),
            serde_json::json!({ "role": target.role, "kind": target.kind }),
        )
        .await?;
    }
    let row = load_member(&mut transaction, organization_id, actor_id).await?;
    transaction.commit().await?;
    Ok(Json(member_view(row, caller, &principal)?))
}

#[derive(Debug, FromRow)]
struct LockedMember {
    kind: String,
    role: String,
    enabled: bool,
    version: i64,
}

async fn lock_member(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<LockedMember, AppError> {
    sqlx::query_as::<_, LockedMember>(
        r#"
        SELECT actor.kind, membership.role, membership.enabled, membership.version
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
         AND actor.kind IN ('human', 'external_agent', 'integration') -- // kind-check: mechanical (accounts that sign in: people, agents and apps, as the Members page lists them; the system and workers aren't members to manage)
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
        FOR UPDATE OF membership
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("workspace member"))
}

/// Take someone out of the workspace and every project in it, and revoke
/// what they could use there. Refuses when they're the only owner of a
/// project, which would leave it without one.
pub(crate) async fn remove_from_workspace(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    actor_id: Uuid,
    removed_by: Uuid,
) -> Result<(), AppError> {
    let projects = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        WHERE project.organization_id = $1 AND membership.actor_id = $2 AND membership.enabled
        ORDER BY project.id
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_all(&mut **transaction)
    .await?;
    for project_id in &projects {
        // The same lock project membership changes take, in a fixed order.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("m7-membership:{project_id}"))
            .execute(&mut **transaction)
            .await?;
    }
    let sole_owner = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM project_memberships mine
            JOIN projects project ON project.id = mine.project_id
            WHERE project.organization_id = $1 AND project.archived_at IS NULL
              AND project.kind <> 'workspace'
              AND mine.actor_id = $2 AND mine.enabled AND mine.role = $3
              AND NOT EXISTS (
                  SELECT 1 FROM project_memberships other
                  WHERE other.project_id = mine.project_id AND other.actor_id <> $2
                    AND other.enabled AND other.role = $3
              )
        )
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(MembershipRole::Owner.key())
    .fetch_one(&mut **transaction)
    .await?;
    if sole_owner {
        return Err(AppError::Conflict(
            "workspace_membership",
            "the only owner of a project",
        ));
    }
    let changed = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        UPDATE project_memberships membership
        SET enabled = false, disabled_by_actor_id = $3, disabled_at = now(),
            version = membership.version + 1, updated_at = now()
        FROM projects project
        WHERE project.id = membership.project_id AND project.organization_id = $1
          AND membership.actor_id = $2 AND membership.enabled
        RETURNING membership.project_id, membership.version
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(removed_by)
    .fetch_all(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE account_credentials credential
        SET status = 'revoked', version = credential.version + 1, revoked_at = now(),
            updated_at = now()
        FROM projects project
        WHERE project.id = credential.project_id AND project.organization_id = $1
          AND credential.actor_id = $2 AND credential.status = 'active'
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE mcp_oauth_grants grant_row
        SET revoked_at = now()
        FROM projects project
        WHERE project.id = grant_row.project_id AND project.organization_id = $1
          AND grant_row.actor_id = $2 AND grant_row.revoked_at IS NULL
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE organization_memberships
        SET enabled = false, version = version + 1, updated_at = now()
        WHERE organization_id = $1 AND actor_id = $2
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE users SET default_organization_id = NULL, updated_at = now() WHERE actor_id = $1 AND default_organization_id = $2",
    )
    .bind(actor_id)
    .bind(organization_id)
    .execute(&mut **transaction)
    .await?;
    for (project_id, version) in changed {
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
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransferRequest {
    actor_id: Uuid,
}

async fn transfer_ownership(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<TransferRequest>,
) -> Result<Json<WorkspaceView>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    if request.actor_id == principal.actor_id {
        return Err(AppError::Validation("new owner"));
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    lock_workspace_memberships(&mut transaction, organization_id).await?;
    let caller = standing_of(&mut transaction, principal.actor_id, organization_id, true).await?;
    if caller.role != MembershipRole::Owner {
        return Err(AppError::Forbidden);
    }
    // Any active member who signs in can be given the workspace, a person
    // or an agent.
    let target = lock_member(&mut transaction, organization_id, request.actor_id).await?;
    let target_active = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM actors WHERE id = $1 AND status = 'active')",
    )
    .bind(request.actor_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !target.enabled || !target_active {
        return Err(AppError::NotFound("workspace member"));
    }
    // An owner has to sign in to use the workspace or hand it on.
    if !signs_in(&target.kind) {
        return Err(AppError::Validation("new owner"));
    }
    sqlx::query(
        r#"
        UPDATE organization_memberships
        SET role = CASE WHEN actor_id = $2 THEN $4 ELSE $5 END, role_id = NULL,
            -- An owner's membership lasts.
            expires_at = CASE WHEN actor_id = $2 THEN NULL ELSE expires_at END,
            role_explicit = true, version = version + 1, updated_at = now()
        WHERE organization_id = $1 AND actor_id IN ($2, $3)
        "#,
    )
    .bind(organization_id)
    .bind(request.actor_id)
    .bind(principal.actor_id)
    .bind(MembershipRole::Owner.key())
    .bind(MembershipRole::Admin.key())
    .execute(&mut *transaction)
    .await?;
    audit(
        &mut transaction,
        &headers,
        &principal,
        organization_id,
        "workspace.ownership_transferred",
        "actor",
        Some(request.actor_id),
        serde_json::json!({ "previousRole": target.role }),
    )
    .await?;
    let view = load_workspace(&mut transaction, organization_id, principal.actor_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

async fn leave_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    lock_workspace_memberships(&mut transaction, organization_id).await?;
    let role = standing_of(&mut transaction, principal.actor_id, organization_id, true)
        .await?
        .role;
    if role == MembershipRole::Owner {
        let other_owners = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*) FROM organization_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
            WHERE membership.organization_id = $1 AND membership.actor_id <> $2
              AND membership.enabled AND membership.role = $3
            "#,
        )
        .bind(organization_id)
        .bind(principal.actor_id)
        .bind(MembershipRole::Owner.key())
        .fetch_one(&mut *transaction)
        .await?;
        if other_owners == 0 {
            return Err(AppError::Conflict("workspace_membership", "last_owner"));
        }
    }
    remove_from_workspace(
        &mut transaction,
        organization_id,
        principal.actor_id,
        principal.actor_id,
    )
    .await?;
    audit(
        &mut transaction,
        &headers,
        &principal,
        organization_id,
        "workspace_membership.left",
        "actor",
        Some(principal.actor_id),
        serde_json::json!({ "role": role.key() }),
    )
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Invitations to the workspace

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvitationView {
    pub(crate) id: Uuid,
    pub(crate) role: String,
    /// A custom role it gives (AWD-74), and that role's name.
    pub(crate) role_id: Option<Uuid>,
    pub(crate) role_name: Option<String>,
    pub(crate) token_prefix: String,
    pub(crate) status: String,
    pub(crate) version: i64,
    pub(crate) issued_by_display_name: String,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) accepted_by_display_name: Option<String>,
    pub(crate) accepted_at: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) note: Option<String>,
}

const INVITATION_COLUMNS: &str = r#"
    invitation.id, invitation.role, invitation.role_id, invited_role.name AS role_name,
    invitation.token_prefix,
    CASE WHEN invitation.status = 'active' AND invitation.expires_at <= now()
         THEN 'expired' ELSE invitation.status END AS status,
    invitation.version,
    COALESCE(issuer.display_name, issuer_user.display_name, 'Unknown') AS issued_by_display_name,
    invitation.expires_at,
    COALESCE(acceptor.display_name, acceptor_user.display_name) AS accepted_by_display_name,
    invitation.accepted_at, invitation.created_at, invitation.note
    FROM workspace_invitations invitation
    LEFT JOIN workspace_roles invited_role ON invited_role.id = invitation.role_id
    LEFT JOIN organization_actor_profiles issuer
      ON issuer.organization_id = invitation.organization_id
     AND issuer.actor_id = invitation.issued_by_actor_id
    LEFT JOIN users issuer_user ON issuer_user.actor_id = invitation.issued_by_actor_id
    LEFT JOIN organization_actor_profiles acceptor
      ON acceptor.organization_id = invitation.organization_id
     AND acceptor.actor_id = invitation.accepted_by_actor_id
    LEFT JOIN users acceptor_user ON acceptor_user.actor_id = invitation.accepted_by_actor_id
"#;

#[derive(Debug, Serialize)]
struct InvitationsResponse {
    invitations: Vec<InvitationView>,
}

async fn list_invitations(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<InvitationsResponse>, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let invitations =
        workspace_invitations_for(&mut connection, principal.actor_id, organization_id).await?;
    Ok(Json(InvitationsResponse { invitations }))
}

/// A workspace's invitations, newest first, for someone who may invite
/// people.
pub(crate) async fn workspace_invitations_for(
    connection: &mut sqlx::PgConnection,
    actor_id: Uuid,
    organization_id: Uuid,
) -> Result<Vec<InvitationView>, AppError> {
    if !standing_of(&mut *connection, actor_id, organization_id, false)
        .await?
        .may(Permission::MembersInvite)
    {
        return Err(AppError::Forbidden);
    }
    Ok(sqlx::query_as::<_, InvitationView>(&format!(
        "SELECT {INVITATION_COLUMNS} WHERE invitation.organization_id = $1 ORDER BY invitation.created_at DESC LIMIT 200"
    ))
    .bind(organization_id)
    .fetch_all(&mut *connection)
    .await?)
}

pub(crate) async fn fetch_invitation(
    connection: &mut sqlx::PgConnection,
    invitation_id: Uuid,
) -> Result<InvitationView, AppError> {
    sqlx::query_as::<_, InvitationView>(&format!(
        "SELECT {INVITATION_COLUMNS} WHERE invitation.id = $1"
    ))
    .bind(invitation_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("workspace_invitation"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IssueRequest {
    role: String,
    /// A custom role of the workspace, instead of `role`.
    #[serde(default)]
    role_id: Option<Uuid>,
    expires_in_hours: i32,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuedInvitation {
    #[serde(flatten)]
    invitation: InvitationView,
    invitation_url: String,
}

/// Whether `issuer`, who may invite people, may invite someone with `role`:
/// an admin-tier role comes only from an owner.
pub(crate) fn may_invite(issuer: Standing, role: &str) -> bool {
    issuer.may(Permission::MembersInvite)
        && match MembershipRole::parse(role) {
            Some(MembershipRole::Admin) => issuer.role == MembershipRole::Owner,
            Some(MembershipRole::Member) => true,
            _ => false,
        }
}

fn generate_token() -> (String, String, [u8; 32]) {
    let issued = engine::issue(Family::Workspace);
    (issued.token, issued.prefix, issued.digest)
}

async fn issue_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<IssueRequest>,
) -> Result<(StatusCode, Json<IssuedInvitation>), AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let issued = issue_workspace_invitation_in(
        &mut transaction,
        principal.actor_id,
        organization_id,
        &request.role,
        request.role_id,
        request.expires_in_hours,
        request.note.as_deref(),
    )
    .await?;
    audit(
        &mut transaction,
        &headers,
        &principal,
        organization_id,
        "workspace.invitation.issue",
        "workspace_invitation",
        Some(issued.invitation_id),
        issued.metadata,
    )
    .await?;
    let invitation = fetch_invitation(&mut transaction, issued.invitation_id).await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(IssuedInvitation {
            invitation,
            invitation_url: crate::invitations::join_url(&state, &issued.token),
        }),
    ))
}

/// A new workspace invitation's ID, its one-time token (only the link
/// carries it) and what the caller's audit row records.
pub(crate) struct IssuedWorkspaceInvitation {
    pub invitation_id: Uuid,
    pub token: String,
    pub metadata: serde_json::Value,
}

/// Invite someone into the workspace with `role` (admin or member) or a
/// custom role, for `expires_in_hours`: owners invite admins and members,
/// admins invite members, and nobody invites to a role holding more than
/// they do. The caller records the audit row.
pub(crate) async fn issue_workspace_invitation_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    organization_id: Uuid,
    role: &str,
    role_id: Option<Uuid>,
    expires_in_hours: i32,
    note: Option<&str>,
) -> Result<IssuedWorkspaceInvitation, AppError> {
    if !(1..=MAX_EXPIRY_HOURS).contains(&expires_in_hours) {
        return Err(AppError::Validation("invitation expiry"));
    }
    let note = note
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_owned);
    if note
        .as_deref()
        .is_some_and(|note| note.chars().count() > 200 || note.chars().any(char::is_control))
    {
        return Err(AppError::Validation("invitation note"));
    }
    let standing = standing_of(&mut *transaction, actor_id, organization_id, true).await?;
    if !standing.may(Permission::MembersInvite) {
        return Err(AppError::Forbidden);
    }
    // A custom role is invited as the workspace role it's based on, and
    // nobody invites to a role holding more than they do.
    let (invited_role, role_id) = match role_id {
        None => {
            if matches!(role, "admin" | "member")
                && !crate::policy::RoleGrant::stored(role, None::<&[String]>)?
                    .permissions()
                    .is_subset_of(standing.permissions)
            {
                return Err(AppError::Forbidden);
            }
            (role.to_owned(), None)
        }
        Some(role_id) => {
            let (built_in, base, permissions) =
                sqlx::query_as::<_, (bool, String, Vec<String>)>(
                    "SELECT built_in, base_role, permissions FROM workspace_roles WHERE organization_id = $1 AND id = $2",
                )
                .bind(organization_id)
                .bind(role_id)
                .fetch_optional(&mut **transaction)
                .await?
                .ok_or(AppError::NotFound("role"))?;
            let grant = if built_in {
                crate::policy::RoleGrant::stored(&base, None::<&[String]>)?
            } else {
                crate::policy::RoleGrant::stored(&base, Some(&permissions))?
            };
            if !grant.permissions().is_subset_of(standing.permissions) {
                return Err(AppError::Forbidden);
            }
            match (built_in, base.as_str()) {
                (true, "admin" | "member") => (base, None),
                (false, "admin") => ("admin".to_owned(), Some(role_id)),
                (false, "member" | "viewer") => ("member".to_owned(), Some(role_id)),
                _ => return Err(AppError::Validation("invitation role")),
            }
        }
    };
    if !may_invite(standing, &invited_role) {
        return Err(if matches!(invited_role.as_str(), "admin" | "member") {
            AppError::Forbidden
        } else {
            AppError::Validation("invitation role")
        });
    }
    let (token, token_prefix, token_digest) = generate_token();
    let invitation_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO workspace_invitations (
            id, organization_id, role, note, token_prefix, token_digest,
            issued_by_actor_id, expires_at, role_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(hours => $8), $9)
        "#,
    )
    .bind(invitation_id)
    .bind(organization_id)
    .bind(&invited_role)
    .bind(&note)
    .bind(&token_prefix)
    .bind(token_digest.to_vec())
    .bind(actor_id)
    .bind(expires_in_hours)
    .bind(role_id)
    .execute(&mut **transaction)
    .await?;
    Ok(IssuedWorkspaceInvitation {
        invitation_id,
        token,
        metadata: serde_json::json!({
            "role": invited_role,
            "roleId": role_id,
            "expiresInHours": expires_in_hours,
        }),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevokeRequest {
    expected_version: i64,
}

async fn revoke_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(invitation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RevokeRequest>,
) -> Result<Json<InvitationView>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let (organization_id, role) = revoke_workspace_invitation_in(
        &mut transaction,
        principal.actor_id,
        invitation_id,
        request.expected_version,
    )
    .await?;
    audit(
        &mut transaction,
        &headers,
        &principal,
        organization_id,
        "workspace.invitation.revoke",
        "workspace_invitation",
        Some(invitation_id),
        serde_json::json!({ "role": role }),
    )
    .await?;
    let view = fetch_invitation(&mut transaction, invitation_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

/// Revoke an active workspace invitation, for someone who may send one with
/// its role. Returns its workspace and role for the caller's audit row.
pub(crate) async fn revoke_workspace_invitation_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    invitation_id: Uuid,
    expected_version: i64,
) -> Result<(Uuid, String), AppError> {
    let (organization_id, role, status, version) =
        sqlx::query_as::<_, (Uuid, String, String, i64)>(
            "SELECT organization_id, role, status, version FROM workspace_invitations WHERE id = $1 FOR UPDATE",
        )
        .bind(invitation_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("workspace_invitation"))?;
    let caller = standing_of(&mut *transaction, actor_id, organization_id, true)
        .await
        .map_err(|_| AppError::NotFound("workspace_invitation"))?;
    if !may_invite(caller, &role) {
        return Err(AppError::Forbidden);
    }
    if version != expected_version {
        return Err(AppError::VersionConflict {
            resource: "workspace_invitation",
            current_version: version,
        });
    }
    if status != "active" {
        return Err(AppError::Conflict("workspace_invitation", "not active"));
    }
    sqlx::query(
        r#"
        UPDATE workspace_invitations
        SET status = 'revoked', revoked_at = now(), version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(invitation_id)
    .execute(&mut **transaction)
    .await?;
    Ok((organization_id, role))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveRequest {
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedInvitation {
    organization_name: String,
    role: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct ValidInvitation {
    id: Uuid,
    organization_id: Uuid,
    organization_name: String,
    role: String,
    role_id: Option<Uuid>,
    token_digest: Vec<u8>,
    issued_by_actor_id: Uuid,
    expires_at: DateTime<Utc>,
}

/// An invitation that can still be accepted: active, unexpired, and its
/// issuer still may issue it.
fn valid_invitation_sql(lock: bool) -> String {
    format!(
        r#"
        SELECT invitation.id, invitation.organization_id,
               organization.name AS organization_name, invitation.role, invitation.role_id,
               invitation.token_digest, invitation.issued_by_actor_id, invitation.expires_at
        FROM workspace_invitations invitation
        JOIN organizations organization ON organization.id = invitation.organization_id
        JOIN actors issuer_actor ON issuer_actor.id = invitation.issued_by_actor_id
          AND issuer_actor.status = 'active'
        JOIN organization_memberships issuer
          ON issuer.organization_id = invitation.organization_id
         AND issuer.actor_id = invitation.issued_by_actor_id AND issuer.enabled
        WHERE invitation.token_prefix = $1 AND invitation.status = 'active'
          AND invitation.expires_at > now()
        {}
        "#,
        if lock {
            "FOR UPDATE OF invitation, issuer"
        } else {
            ""
        }
    )
}

/// Check a token's shape and rate limit, then forget it. Returns its lookup
/// prefix and digest.
async fn consume_token(
    token: &mut String,
    state: &AppState,
) -> Result<(String, [u8; 32]), AppError> {
    engine::consume(token, Family::Workspace, state, "workspace_invitation").await
}

fn digest_matches(stored: &[u8], candidate: &[u8; 32]) -> bool {
    engine::digest_matches(stored, candidate)
}

async fn find_invitation(
    connection: &mut sqlx::PgConnection,
    prefix: &str,
    digest: &[u8; 32],
    lock: bool,
) -> Result<ValidInvitation, AppError> {
    let row = sqlx::query_as::<_, ValidInvitation>(&valid_invitation_sql(lock))
        .bind(prefix)
        .fetch_optional(&mut *connection)
        .await?
        .filter(|row| digest_matches(&row.token_digest, digest))
        .ok_or(AppError::NotFound("workspace_invitation"))?;
    // The issuer's role may have narrowed since they sent it.
    if !crate::roles::issuer_still_grants(
        connection,
        row.organization_id,
        row.issued_by_actor_id,
        &row.role,
        row.role_id,
    )
    .await?
    {
        return Err(AppError::NotFound("workspace_invitation"));
    }
    Ok(row)
}

async fn resolve_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<ResolveRequest>,
) -> Result<Json<ResolvedInvitation>, AppError> {
    // A link's token names the one row it opens, before any scope is known.
    crate::tenancy::system(
        "auth.link",
        Box::pin(resolve_invitation_here(
            State(state),
            headers,
            ApiJson(request),
        )),
    )
    .await
}

async fn resolve_invitation_here(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<ResolveRequest>,
) -> Result<Json<ResolvedInvitation>, AppError> {
    let mut token = Zeroizing::new(std::mem::take(&mut request.token));
    require_allowed_origin(&state.config, &headers)?;
    let (prefix, digest) = consume_token(&mut token, &state).await?;
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let found = find_invitation(&mut connection, &prefix, &digest, false).await;
    let (row, _connection) = state
        .rate_limits
        .release_on_link_miss(connection, found)
        .await?;
    Ok(Json(ResolvedInvitation {
        organization_name: row.organization_name,
        role: row.role,
        expires_at: row.expires_at,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptNewRequest {
    token: String,
    login_name: String,
    display_name: String,
    handle: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptExistingRequest {
    token: String,
    handle: String,
}

pub(crate) fn normalize_display_name(value: &str) -> Result<String, AppError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.chars().count() > 120
        || normalized.chars().any(char::is_control)
    {
        return Err(AppError::Validation("display name"));
    }
    Ok(normalized.to_owned())
}

/// A workspace handle: as `organization_actor_profiles` requires.
pub(crate) fn normalize_handle(value: &str) -> Result<String, AppError> {
    let normalized = value.trim().to_ascii_lowercase();
    let bytes = normalized.as_bytes();
    if !(2..=32).contains(&normalized.len())
        || !bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !normalized.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(AppError::Validation("handle"));
    }
    Ok(normalized)
}

pub(crate) fn map_join_error(error: sqlx::Error) -> AppError {
    let constraint = match &error {
        sqlx::Error::Database(database) => database.constraint(),
        _ => None,
    };
    match constraint {
        Some("users_login_name_unique") => AppError::Conflict("user", "login name already exists"),
        Some("organization_actor_profiles_organization_id_handle_key") => {
            AppError::Conflict("workspace_membership", "handle already exists")
        }
        _ => AppError::Database(error),
    }
}

/// Make `actor_id` a member of the invitation's workspace with its role,
/// with a workspace profile, and mark the invitation used.
async fn join_workspace(
    transaction: &mut Transaction<'_, Postgres>,
    invitation: &ValidInvitation,
    actor_id: Uuid,
    display_name: &str,
    handle: &str,
) -> Result<(), AppError> {
    // One invitation engine (K1.29): the same admission as every other way in.
    let admitted = engine::admit(
        transaction,
        engine::Admission {
            workspace_id: invitation.organization_id,
            account_id: actor_id,
            role: &invitation.role,
            role_id: invitation.role_id,
            display_name,
            handle,
        },
    )
    .await
    .map_err(|error| match error {
        AppError::Database(database) => map_join_error(database),
        other => other,
    })?;
    if admitted == engine::Admitted::AlreadyMember {
        return Err(AppError::Conflict(
            "workspace_invitation",
            "already a member of this workspace",
        ));
    }
    let updated = sqlx::query(
        r#"
        UPDATE workspace_invitations
        SET status = 'accepted', accepted_by_actor_id = $2, accepted_at = now(),
            version = version + 1, updated_at = now()
        WHERE id = $1 AND status = 'active' AND expires_at > now()
        "#,
    )
    .bind(invitation.id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::NotFound("workspace_invitation"));
    }
    Ok(())
}

async fn accept_new_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<AcceptNewRequest>,
) -> Result<Response, AppError> {
    // A link's token names the one row it opens, before any scope is known.
    crate::tenancy::system(
        "auth.link",
        Box::pin(accept_new_user_here(
            State(state),
            headers,
            ApiJson(request),
        )),
    )
    .await
}

async fn accept_new_user_here(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<AcceptNewRequest>,
) -> Result<Response, AppError> {
    let mut token = Zeroizing::new(std::mem::take(&mut request.token));
    let mut submitted_password = Zeroizing::new(std::mem::take(&mut request.password));
    require_allowed_origin(&state.config, &headers)?;
    if load_authenticated_actor(&state, &headers).await?.is_some() {
        return Err(AppError::Conflict(
            "workspace_invitation",
            "signed-in users must accept with their existing identity",
        ));
    }
    let (prefix, digest) = consume_token(&mut token, &state).await?;
    let login_name = normalize_login_name(&request.login_name)
        .map_err(|_| AppError::Validation("login name"))?;
    let display_name = normalize_display_name(&request.display_name)?;
    let handle = normalize_handle(&request.handle)?;
    if !(12..=1024).contains(&submitted_password.len()) {
        return Err(AppError::Validation("password"));
    }
    let _password_kdf_permit = state.rate_limits.try_acquire_password_kdf()?;
    let password_phc = password::hash_password(std::mem::take(&mut *submitted_password)).await?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let found = find_invitation(&mut transaction, &prefix, &digest, true).await;
    let (invitation, mut transaction) = state
        .rate_limits
        .release_on_link_miss(transaction, found)
        .await?;
    let crate::accounts::Person {
        account_id: actor_id,
        user_id,
    } = crate::accounts::create_person(
        &mut transaction,
        crate::accounts::NewPerson {
            login_name: &login_name,
            display_name: &display_name,
            default_workspace: Some(invitation.organization_id),
            created_by: Some(invitation.issued_by_actor_id),
            claimed: true,
        },
    )
    .await
    .map_err(map_join_error)?;
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(user_id)
        .bind(password_phc)
        .execute(&mut *transaction)
        .await?;
    join_workspace(
        &mut transaction,
        &invitation,
        actor_id,
        &display_name,
        &handle,
    )
    .await?;
    let established = establish_session_in_transaction(
        &state.config,
        &headers,
        &mut transaction,
        SessionIdentity {
            user_id,
            actor_id,
            actor_kind: "human".to_owned(),
            login_name,
            display_name,
            authentication_epoch: 1,
        },
    )
    .await?;
    crate::account_security::record_sign_in_method(
        &mut transaction,
        established.principal.session_id,
        "invitation",
    )
    .await?;
    acceptance_audit(
        &mut transaction,
        &established.principal,
        &invitation,
        "invitation",
        true,
    )
    .await?;
    transaction.commit().await?;
    respond_with_established_session(&state, established).await
}

async fn accept_existing_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<AcceptExistingRequest>,
) -> Result<Response, AppError> {
    // A link's token names the one row it opens, before any scope is known.
    crate::tenancy::system(
        "auth.link",
        Box::pin(accept_existing_user_here(
            State(state),
            headers,
            principal,
            ApiJson(request),
        )),
    )
    .await
}

async fn accept_existing_user_here(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(mut request): ApiJson<AcceptExistingRequest>,
) -> Result<Response, AppError> {
    let mut token = Zeroizing::new(std::mem::take(&mut request.token));
    require_authenticated_write(&state, &headers, &principal).await?;
    let (prefix, digest) = consume_token(&mut token, &state).await?;
    let handle = normalize_handle(&request.handle)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let found = find_invitation(&mut transaction, &prefix, &digest, true).await;
    let (invitation, mut transaction) = state
        .rate_limits
        .release_on_link_miss(transaction, found)
        .await?;
    let epoch = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT user_account.authentication_epoch
        FROM users user_account
        JOIN actors actor ON actor.id = user_account.actor_id
        WHERE user_account.id = $1 AND user_account.status = 'active'
          AND actor.status = 'active' AND actor.kind = 'human' -- // kind-check: mechanical (a signed-in user accepts an invitation)
        FOR UPDATE OF user_account
        "#,
    )
    .bind(principal.user_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("workspace_invitation"))?;
    join_workspace(
        &mut transaction,
        &invitation,
        principal.actor_id,
        &principal.display_name,
        &handle,
    )
    .await?;
    // Joining is not choosing (P0.37): the person's default workspace moves
    // here only when they had none and belong nowhere else, so an
    // invitation can't make itself the workspace that carries their
    // account mail or their Home.
    sqlx::query(
        r#"
        UPDATE users SET default_organization_id = $2, updated_at = now()
        WHERE id = $1 AND default_organization_id IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM organization_memberships other
              WHERE other.actor_id = users.actor_id AND other.enabled
                AND other.organization_id <> $2
          )
        "#,
    )
    .bind(principal.user_id)
    .bind(invitation.organization_id)
    .execute(&mut *transaction)
    .await?;
    let established = establish_session_in_transaction(
        &state.config,
        &headers,
        &mut transaction,
        SessionIdentity {
            user_id: principal.user_id,
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind.clone(),
            login_name: principal.login_name.clone(),
            display_name: principal.display_name.clone(),
            authentication_epoch: epoch,
        },
    )
    .await?;
    crate::account_security::copy_session_details(
        &mut transaction,
        principal.session_id,
        established.principal.session_id,
    )
    .await?;
    acceptance_audit(
        &mut transaction,
        &established.principal,
        &invitation,
        "password_session",
        false,
    )
    .await?;
    transaction.commit().await?;
    respond_with_established_session(&state, established).await
}

async fn acceptance_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    invitation: &ValidInvitation,
    authentication_class: &'static str,
    created_user: bool,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, organization_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, metadata, channel
        ) VALUES ($1, $2, $3, $4, $5, 'workspace.invitation.accept',
                  'workspace_invitation', $6, 'succeeded', $7, 'browser')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(invitation.organization_id)
    .bind(principal.session_id)
    .bind(authentication_class)
    .bind(invitation.id)
    .bind(serde_json::json!({
        "role": invitation.role,
        "issuedByActorId": invitation.issued_by_actor_id,
        "createdUser": created_user,
    }))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The workspace a person works in by default

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefaultWorkspaceRequest {
    organization_id: Uuid,
}

async fn set_default_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<DefaultWorkspaceRequest>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    standing_of(
        &mut transaction,
        principal.actor_id,
        request.organization_id,
        true,
    )
    .await?;
    sqlx::query("UPDATE users SET default_organization_id = $2, updated_at = now() WHERE id = $1")
        .bind(principal.user_id)
        .bind(request.organization_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The workspace a workspace-level request acts in (K4.6): the one it
/// names, if the account is an enabled member of it and it is active; with
/// none named, the account's only workspace. It is never inferred from the
/// last one used: an account in several must say which.
pub(crate) async fn named_or_only(
    connection: &mut sqlx::PgConnection,
    actor_id: Uuid,
    named: Option<Uuid>,
) -> Result<Uuid, AppError> {
    let reach = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.organization_id
        FROM organization_memberships membership
        JOIN organizations workspace
          ON workspace.id = membership.organization_id AND workspace.lifecycle = 'active'
        JOIN actors account ON account.id = membership.actor_id AND account.status = 'active'
        WHERE membership.actor_id = $1 AND membership.enabled
          AND ($2::uuid IS NULL OR membership.organization_id = $2)
        LIMIT 2
        "#,
    )
    .bind(actor_id)
    .bind(named)
    .fetch_all(&mut *connection)
    .await?;
    match reach.as_slice() {
        [only] => Ok(*only),
        [] => Err(AppError::NotFound("organization")),
        _ => Err(AppError::Refused {
            field: "workspaceId",
            message: "You belong to more than one workspace: say which one.",
        }),
    }
}

/// Where `/` lands and what an old address without a workspace means: the
/// workspace the person last chose if they still belong to it, else the
/// earliest they joined. Never the target of a request (K4.6).
pub(crate) async fn default_workspace<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    actor_id: Uuid,
) -> Result<Option<(Uuid, String)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT membership.organization_id, membership.role
        FROM organization_memberships membership
        LEFT JOIN users user_account ON user_account.actor_id = membership.actor_id
        WHERE membership.actor_id = $1 AND membership.enabled
        ORDER BY (membership.organization_id = user_account.default_organization_id) DESC NULLS LAST,
                 membership.created_at, membership.organization_id
        LIMIT 1
        "#,
    )
    .bind(actor_id)
    .fetch_optional(executor)
    .await?)
}

// ---------------------------------------------------------------------------
// Audit

#[allow(clippy::too_many_arguments)]
pub(crate) async fn audit(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
    action: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, organization_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, channel, metadata
        ) VALUES ($1, $2, $3, $4, 'password_session', $5, $6, $7, 'succeeded', $8, 'browser', $9)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(organization_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn standing(role: MembershipRole, permissions: crate::policy::PermissionSet) -> Standing {
        Standing {
            actor_id: Uuid::nil(),
            role,
            permissions,
        }
    }

    fn built_in(role: MembershipRole) -> Standing {
        standing(role, crate::policy::PermissionSet::built_in(role))
    }

    #[test]
    fn only_owners_change_roles_and_nobody_touches_owners_or_themselves() {
        use MembershipRole::{Admin, Member, Owner};
        let owner_on_admin = member_capabilities(built_in(Owner), false, Admin, true, true);
        assert!(owner_on_admin.change_role && owner_on_admin.remove);
        let owner_on_agent = member_capabilities(built_in(Owner), false, Member, false, true);
        assert!(owner_on_agent.change_role && owner_on_agent.remove && !owner_on_agent.limit_apps);
        assert_eq!(
            member_capabilities(built_in(Owner), false, Owner, true, true),
            MemberCapabilities::default()
        );
        assert_eq!(
            member_capabilities(built_in(Owner), true, Owner, true, true),
            MemberCapabilities {
                limit_apps: true,
                ..MemberCapabilities::default()
            },
            "people limit their own apps and nothing else about themselves"
        );
        let admin_on_member = member_capabilities(built_in(Admin), false, Member, true, true);
        assert!(!admin_on_member.change_role && admin_on_member.remove);
        assert_eq!(
            member_capabilities(built_in(Admin), false, Admin, true, true),
            MemberCapabilities::default()
        );
        assert_eq!(
            member_capabilities(built_in(Member), false, Member, true, true),
            MemberCapabilities::default()
        );
        assert_eq!(
            member_capabilities(built_in(Owner), false, Member, true, false),
            MemberCapabilities::default()
        );
        // An owner whose custom role leaves out "manage members" manages nobody.
        let narrowed = standing(
            Owner,
            crate::policy::PermissionSet::built_in(Owner).without(Permission::MembersManage),
        );
        assert_eq!(
            member_capabilities(narrowed, false, Member, true, true),
            MemberCapabilities::default()
        );
    }

    #[test]
    fn admins_invite_members_and_owners_invite_admins() {
        use MembershipRole::{Admin, Member, Owner};
        assert!(may_invite(built_in(Owner), "admin"));
        assert!(may_invite(built_in(Owner), "member"));
        assert!(!may_invite(built_in(Admin), "admin"));
        assert!(may_invite(built_in(Admin), "member"));
        assert!(!may_invite(built_in(Member), "member"));
        assert!(!may_invite(built_in(Owner), "owner"));
    }

    #[test]
    fn workspace_tokens_are_high_entropy_and_distinct_from_project_ones() {
        let (token, prefix, digest) = generate_token();
        assert_eq!(token.len(), 50);
        assert!(token.starts_with("aw_wsi_"));
        assert_eq!(prefix.len(), engine::PREFIX_LENGTH);
        assert_eq!(digest, <[u8; 32]>::from(Sha256::digest(token.as_bytes())));
        assert_ne!(generate_token().0, token);
    }

    #[test]
    fn workspace_names_and_handles_are_bounded() {
        assert_eq!(
            normalize_workspace_name("  Acme  ").expect("test value"),
            "Acme"
        );
        assert!(normalize_workspace_name("   ").is_err());
        assert!(normalize_workspace_name(&"a".repeat(121)).is_err());
        assert!(normalize_workspace_name("a\nb").is_err());
        assert_eq!(normalize_handle("Mischa").expect("test value"), "mischa");
        assert!(normalize_handle("-x").is_err());
        assert!(normalize_handle("a").is_err());
    }
}

//! What a caller holds in one scope (K1 §3.5 steps 1 to 6), read from the
//! memberships, roles, module switches and the credential's limits. The
//! only place that loads them to make a decision.

use sqlx::PgConnection;
use uuid::Uuid;

use super::{Caller, Holding, Reach, Scope, Tier};
use crate::{
    error::AppError,
    policy::{MembershipRole, Permission, PermissionSet, RoleGrant},
};

/// What a run delegates to its agent in the run's space when the agent
/// holds only a worker membership there (the built-in host's agents, A1.16):
/// seeing the space, reporting on the run and asking people through it.
pub const RUN_DELEGATION: PermissionSet = PermissionSet::EMPTY
    .with(Permission::ProjectsView)
    .with(Permission::RunsReport)
    .with(Permission::RequestsRaise);

#[derive(sqlx::FromRow)]
struct RoleRow {
    workspace_id: Uuid,
    role: String,
    role_permissions: Option<Vec<String>>,
    act_within_requester: bool,
}

/// Whether the membership's role is held to its requester (K1.25).
const WITHIN_REQUESTER: &str = "COALESCE((SELECT held.act_within_requester FROM workspace_roles held \
     WHERE held.id = membership.role_id), false) AS act_within_requester";

/// The caller's holding in `scope`, or `None` when they can't act there at
/// all (no enabled membership, an inactive account, an archived space, or
/// a space their credential doesn't reach). `lock` takes the membership rows
/// `FOR SHARE`, for checks inside a write transaction.
pub async fn load(
    connection: &mut PgConnection,
    caller: &Caller,
    scope: Scope,
    lock: bool,
) -> Result<Option<Holding>, AppError> {
    load_with(connection, caller, scope, lock, false).await
}

/// [`load`], also for an archived space: what its members held there, for
/// the one thing they may do to it (bring it back, K4.7).
pub async fn load_archived(
    connection: &mut PgConnection,
    caller: &Caller,
    space: Uuid,
    lock: bool,
) -> Result<Option<Holding>, AppError> {
    load_with(connection, caller, Scope::Space(space), lock, true).await
}

/// The caller's holding in a DM or group (K4.17), which is in no space:
/// `None` when they can't see it; else what their roles allow there
/// (`direct_permissions`), narrowed by an app's delegation limit. A
/// credential confined to one space reaches no DM.
pub async fn load_direct(
    connection: &mut PgConnection,
    caller: &Caller,
    conversation: Uuid,
) -> Result<Option<Holding>, AppError> {
    if let Reach::Space(_) = caller.reach() {
        return Ok(None);
    }
    let row = sqlx::query_as::<_, (Uuid, Vec<String>)>(
        "SELECT conversation.workspace_id, direct_permissions(conversation.id, $2) \
         FROM conversations conversation \
         WHERE conversation.id = $1 AND conversation.project_id IS NULL \
           AND conversation_visible_to(conversation.id, $2)",
    )
    .bind(conversation)
    .bind(caller.account_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((workspace_id, keys)) = row else {
        return Ok(None);
    };
    let mut permissions = PermissionSet::from_keys(&keys);
    if let Some(limits) = &caller.limits {
        // A kept DM follows its workspace's limit; a personal one what the
        // limit allows in any workspace.
        let limit = match limits.organizations.get(&workspace_id) {
            Some(limit) => *limit,
            None => limits
                .organizations
                .values()
                .fold(PermissionSet::EMPTY, |all, limit| all.union(*limit)),
        };
        permissions = permissions.intersect(limit);
    }
    if let Some(requester) = caller.requested_by
        && requester != caller.account_id
    {
        let theirs = sqlx::query_scalar::<_, Vec<String>>(
            "SELECT direct_permissions($1, $2) WHERE conversation_visible_to($1, $2)",
        )
        .bind(conversation)
        .bind(requester)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(theirs) = theirs
            && within_requester_held(connection, caller.account_id, workspace_id).await?
        {
            permissions = permissions.intersect(PermissionSet::from_keys(&theirs));
        }
    }
    Ok(Some(Holding {
        scope: Scope::Workspace(workspace_id),
        workspace_id,
        permissions,
        tier: Tier::Member,
        modules: None,
        delegated: caller.limits.is_some(),
    }))
}

/// Whether any role `account` holds in `workspace` acts only within its
/// requester (K1.25).
async fn within_requester_held(
    connection: &mut PgConnection,
    account: Uuid,
    workspace: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_memberships membership \
         JOIN workspace_roles held ON held.id = membership.role_id \
         WHERE membership.actor_id = $1 AND membership.organization_id = $2 \
           AND held.act_within_requester)",
    )
    .bind(account)
    .bind(workspace)
    .fetch_one(&mut *connection)
    .await?)
}

/// The permissions of each role a group of `account` holds in `space`.
async fn group_grants(
    connection: &mut PgConnection,
    account: Uuid,
    space: Uuid,
) -> Result<Vec<Vec<String>>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Vec<String>>>(
        "SELECT workspace_role_permissions(grant_row.role_id) FROM role_grants grant_row \
         JOIN account_groups grp ON grp.id = grant_row.group_id AND grp.archived_at IS NULL \
         JOIN account_group_members member \
           ON member.group_id = grant_row.group_id AND member.account_id = $1 \
         WHERE grant_row.space_id = $2",
    )
    .bind(account)
    .bind(space)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .flatten()
    .collect())
}

async fn load_with(
    connection: &mut PgConnection,
    caller: &Caller,
    scope: Scope,
    lock: bool,
    archived: bool,
) -> Result<Option<Holding>, AppError> {
    let lock_clause = if lock { "FOR SHARE OF membership" } else { "" };
    let row = match scope {
        Scope::Space(space) => {
            if let Reach::Space(confined) = caller.reach()
                && confined != space
            {
                return Ok(None);
            }
            // A run's agent on the built-in host is a member of the run's
            // space only (a worker membership); acting for its run, it needs
            // no workspace membership, but a disabled one still stops it.
            // (Its credential was checked against the run, its binding and
            // this space when it signed in.)
            let (workspace_membership, reaches) = if caller.for_run {
                (
                    "LEFT JOIN organization_memberships workspace_membership
                  ON workspace_membership.organization_id = project.organization_id
                 AND workspace_membership.actor_id = $1",
                    "($3::uuid IS NULL OR TRUE)",
                )
            } else {
                (
                    "JOIN organization_memberships workspace_membership
                  ON workspace_membership.organization_id = project.organization_id
                 AND workspace_membership.actor_id = $1
                 AND workspace_membership.enabled",
                    "actor_can_access_project($1, $3, $2)",
                )
            };
            sqlx::query_as::<_, RoleRow>(&format!(
                r#"
                SELECT project.organization_id AS workspace_id, membership.role,
                       workspace_role_permissions(membership.role_id) AS role_permissions,
                       {WITHIN_REQUESTER}
                FROM project_memberships membership
                JOIN projects project ON project.id = membership.project_id
                {workspace_membership}
                JOIN actors actor ON actor.id = $1 AND actor.status = 'active'
                WHERE membership.project_id = $2 AND membership.actor_id = $1 AND membership.enabled
                  AND (project.archived_at IS NULL OR $4)
                  AND workspace_membership.enabled IS DISTINCT FROM false
                  AND (
                      {reaches}
                      -- An archived space, for bringing it back: its members
                      -- with an active sign-in, as they were.
                      OR ($4 AND project.archived_at IS NOT NULL
                          AND NOT EXISTS (
                              SELECT 1 FROM users user_account
                              WHERE user_account.actor_id = $1
                                AND user_account.status <> 'active')
                          AND ($3::uuid IS NULL OR EXISTS (
                              SELECT 1 FROM external_agent_connections agent_connection
                              WHERE agent_connection.id = $3
                                AND agent_connection.status = 'active'
                                AND agent_connection.workspace_id = project.organization_id)))
                  )
                {lock_clause}
                "#
            ))
            .bind(caller.account_id)
            .bind(space)
            .bind(caller.connection_id)
            .bind(archived)
            .fetch_optional(&mut *connection)
            .await?
        }
        Scope::Workspace(workspace) => {
            if let Reach::Space(confined) = caller.reach() {
                let reached = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM projects WHERE id = $1 AND organization_id = $2)",
                )
                .bind(confined)
                .bind(workspace)
                .fetch_one(&mut *connection)
                .await?;
                if !reached {
                    return Ok(None);
                }
            }
            sqlx::query_as::<_, RoleRow>(&format!(
                r#"
                SELECT membership.organization_id AS workspace_id, membership.role,
                       workspace_role_permissions(membership.role_id) AS role_permissions,
                       {WITHIN_REQUESTER}
                FROM organization_memberships membership
                JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
                WHERE membership.organization_id = $2 AND membership.actor_id = $1
                  AND membership.enabled
                {lock_clause}
                "#
            ))
            .bind(caller.account_id)
            .bind(workspace)
            .fetch_optional(&mut *connection)
            .await?
        }
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let grant = RoleGrant::stored(&row.role, row.role_permissions.as_deref())?;
    // A worker membership holds nothing of its own: it acts only within
    // what a run delegates (the role's tier, not the account's kind).
    let mut permissions = if grant.role() == MembershipRole::Worker {
        if caller.for_run && matches!(scope, Scope::Space(_)) {
            RUN_DELEGATION
        } else {
            PermissionSet::EMPTY
        }
    } else {
        grant.permissions()
    };
    // K1.27: and whatever the roles given to their groups in this space
    // allow.
    if let Scope::Space(space) = scope
        && grant.role() != MembershipRole::Worker
    {
        for keys in group_grants(connection, caller.account_id, space).await? {
            permissions = permissions.union(PermissionSet::from_keys(&keys));
        }
    }
    // A credential confined to one space acts at workspace level only
    // within what it holds in that space, and never with a permission that
    // reaches the whole workspace (members, roles, new spaces).
    if let (Scope::Workspace(_), Reach::Space(confined)) = (scope, caller.reach()) {
        let in_space = Box::pin(load(connection, caller, Scope::Space(confined), lock))
            .await?
            .map_or(PermissionSet::EMPTY, |holding| holding.permissions);
        permissions = permissions
            .intersect(in_space)
            .intersect(PermissionSet::all().without_all(PermissionSet::workspace_only()));
    }
    let delegated = caller.limits.is_some();
    if let Some(limits) = &caller.limits {
        let limit = match scope {
            Scope::Space(space) => limits.projects.get(&space),
            Scope::Workspace(workspace) => limits.organizations.get(&workspace),
        }
        .copied()
        .unwrap_or(PermissionSet::EMPTY);
        permissions = permissions.intersect(limit);
    }
    // K1 §3.5 step 7: a role held to its requester acts, on someone's
    // request, only within what that account holds here too.
    if row.act_within_requester
        && let Some(requester) = caller.requested_by
        && requester != caller.account_id
    {
        let as_requester = Caller::person(requester, caller.kind);
        let theirs = Box::pin(load_with(connection, &as_requester, scope, lock, archived))
            .await?
            .map_or(PermissionSet::EMPTY, |holding| holding.permissions);
        permissions = permissions.intersect(theirs);
    }
    let modules = match scope {
        Scope::Space(space) => {
            crate::modules::state::for_project(connection, space, caller.account_id).await?
        }
        Scope::Workspace(workspace) => {
            Some(crate::modules::state::for_member(connection, workspace, caller.account_id).await?)
        }
    };
    Ok(Some(Holding {
        scope,
        workspace_id: row.workspace_id,
        permissions,
        tier: Tier::from(grant.role()),
        modules,
        delegated,
    }))
}

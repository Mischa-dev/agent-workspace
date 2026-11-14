//! What a caller holds in one scope (K1 §3.5 steps 1 to 6), read from the
//! memberships, roles, module switches and the credential's limits. The
//! only place that loads them to make a decision.

use sqlx::PgConnection;
use uuid::Uuid;

use super::{Caller, Holding, Reach, Scope, Tier};
use crate::{
    error::AppError,
    policy::{MembershipRole, PermissionSet, RoleGrant},
};

#[derive(sqlx::FromRow)]
struct RoleRow {
    workspace_id: Uuid,
    role: String,
    role_permissions: Option<Vec<String>>,
}

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
    let lock_clause = if lock { "FOR SHARE OF membership" } else { "" };
    let row = match scope {
        Scope::Space(space) => {
            if let Reach::Space(confined) = caller.reach()
                && confined != space
            {
                return Ok(None);
            }
            sqlx::query_as::<_, RoleRow>(&format!(
                r#"
                SELECT project.organization_id AS workspace_id, membership.role,
                       workspace_role_permissions(membership.role_id) AS role_permissions
                FROM project_memberships membership
                JOIN projects project ON project.id = membership.project_id
                JOIN organization_memberships workspace_membership
                  ON workspace_membership.organization_id = project.organization_id
                 AND workspace_membership.actor_id = $1
                 AND workspace_membership.enabled
                JOIN actors actor ON actor.id = $1 AND actor.status = 'active'
                WHERE membership.project_id = $2 AND membership.actor_id = $1 AND membership.enabled
                  AND project.archived_at IS NULL
                  AND actor_can_access_project($1, $3, $2)
                {lock_clause}
                "#
            ))
            .bind(caller.account_id)
            .bind(space)
            .bind(caller.connection_id)
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
                       workspace_role_permissions(membership.role_id) AS role_permissions
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
        PermissionSet::EMPTY
    } else {
        grant.permissions()
    };
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

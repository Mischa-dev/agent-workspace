//! Today's access rules in the shape K1's access module will answer them
//! (ruling R2): allow, suggest only, or deny. Built on the stored roles,
//! `workspace_role_permissions`, `actor_can_access_project` (migration
//! 0042) and a person's agent limits, exactly as the MCP tools and web
//! handlers check them today. Deleted by K2.17 once K1.2, K1.10 and K1.12
//! have merged; nothing outside `ops` calls it.

use sqlx::PgConnection;
use uuid::Uuid;

use super::{Caller, DenyReason, Located, OpError, Perm, Target};
use crate::policy::{MembershipRole, PermissionSet, RoleGrant};

/// How a write may be applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteMode {
    /// The change applies.
    Direct,
    /// The change is held as a proposal for someone who may edit (C1).
    Propose,
}

/// The answer to "may this caller do this here".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Grant {
    pub mode: WriteMode,
}

#[derive(sqlx::FromRow)]
struct RoleRow {
    role: String,
    role_permissions: Option<Vec<String>>,
}

/// What the caller's role grants in a space, narrowed by their agent
/// limit; `None` when they can't act there at all.
async fn space_permissions(
    connection: &mut PgConnection,
    caller: &Caller,
    space_id: Uuid,
) -> Result<Option<PermissionSet>, OpError> {
    let row = sqlx::query_as::<_, RoleRow>(
        r#"
        SELECT membership.role,
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
        "#,
    )
    .bind(caller.account_id)
    .bind(space_id)
    .bind(caller.connection_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| OpError::logged("access", &error.into()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let held = granted(&row)?;
    Ok(Some(match &caller.limits {
        Some(limits) => held.intersect(
            limits
                .projects
                .get(&space_id)
                .copied()
                .unwrap_or(PermissionSet::EMPTY),
        ),
        None => held,
    }))
}

/// What the caller's workspace role grants, narrowed by their agent limit.
async fn workspace_permissions(
    connection: &mut PgConnection,
    caller: &Caller,
    workspace_id: Uuid,
) -> Result<Option<PermissionSet>, OpError> {
    let row = sqlx::query_as::<_, RoleRow>(
        r#"
        SELECT membership.role,
               workspace_role_permissions(membership.role_id) AS role_permissions
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
        WHERE membership.organization_id = $2 AND membership.actor_id = $1 AND membership.enabled
        "#,
    )
    .bind(caller.account_id)
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| OpError::logged("access", &error.into()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let held = granted(&row)?;
    Ok(Some(match &caller.limits {
        Some(limits) => held.intersect(
            limits
                .organizations
                .get(&workspace_id)
                .copied()
                .unwrap_or(PermissionSet::EMPTY),
        ),
        None => held,
    }))
}

fn granted(row: &RoleRow) -> Result<PermissionSet, OpError> {
    let grant = RoleGrant::stored(&row.role, row.role_permissions.as_deref())
        .map_err(|_| OpError::Unavailable)?;
    // Workers act only within what a run delegates, never on their own.
    if grant.role() == MembershipRole::Worker {
        return Ok(PermissionSet::EMPTY);
    }
    Ok(grant.permissions())
}

/// The workspaces a caller reaches: where they're an active member, or the
/// one their space-scoped credential is in.
pub async fn reach(connection: &mut PgConnection, caller: &Caller) -> Result<Vec<Uuid>, OpError> {
    let workspaces = match caller.confined_space {
        Some(space) => {
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(space)
                .fetch_all(&mut *connection)
                .await
        }
        None => {
            sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT membership.organization_id FROM organization_memberships membership
                JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
                WHERE membership.actor_id = $1 AND membership.enabled
                ORDER BY membership.organization_id
                "#,
            )
            .bind(caller.account_id)
            .fetch_all(&mut *connection)
            .await
        }
    };
    workspaces.map_err(|error| OpError::logged("access", &error.into()))
}

/// Check that `caller` may run an operation needing `permission` (or, with
/// none, may see the target) where it located. A target they can't reach is
/// `not_found`, never `permission_denied`, so a refusal reveals nothing.
pub async fn check(
    connection: &mut PgConnection,
    caller: &Caller,
    target: &Target,
    located: &Located,
    permission: Option<Perm>,
    writes: bool,
    op: &'static str,
) -> Result<Grant, OpError> {
    if writes && !caller.can_write {
        return Err(OpError::Denied {
            reason: DenyReason::ReadOnlyCredential,
            message: "This connection is read-only. Reconnect it with full access to make changes."
                .into(),
        });
    }
    if !writes && !caller.can_read {
        return Err(OpError::Denied {
            reason: DenyReason::ReadOnlyCredential,
            message: "This connection can't read.".into(),
        });
    }
    if matches!(target, Target::Account) {
        return Ok(Grant {
            mode: WriteMode::Direct,
        });
    }
    let not_found = || match located.space_id {
        Some(_) => OpError::not_found("That space"),
        None => OpError::not_found("That workspace"),
    };
    // An older credential tied to one space stays there.
    if let Some(confined) = caller.confined_space
        && located.space_id.is_some_and(|space| space != confined)
    {
        return Err(not_found());
    }
    let held = match located.space_id {
        Some(space) => space_permissions(connection, caller, space).await?,
        None => {
            if caller.confined_space.is_some()
                && !reach(connection, caller)
                    .await?
                    .contains(&located.workspace_id)
            {
                return Err(not_found());
            }
            workspace_permissions(connection, caller, located.workspace_id).await?
        }
    }
    .ok_or_else(not_found)?;
    if let Some(permission) = permission
        && !held.contains(permission)
    {
        return Err(OpError::Denied {
            reason: DenyReason::Role { permission },
            message: format!(
                "{op} needs \"{}\" here, which your role doesn't give you{}. Ask a workspace owner or admin.",
                permission.info().label,
                if caller.limits.is_some() {
                    " through this app"
                } else {
                    ""
                },
            )
            .into(),
        });
    }
    Ok(Grant {
        mode: WriteMode::Direct,
    })
}

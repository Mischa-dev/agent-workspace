//! Managing an account (K1 §3.6; rulings R19, R34): one rule for every path
//! that manages an agent or app account or acts as it (the web, `agent_list`,
//! OAuth's "connect as agent", connected apps).
//!
//! A caller may manage an agent or app account when they are its owning
//! account; or hold `agents.manage_any` in its owning workspace; or are
//! named in `account_managers` for it and hold `agents.manage` in a
//! workspace the account is a member of. Holding `agents.manage` alone is
//! not enough. What they give it is within what they hold (`may_grant`).

use sqlx::PgConnection;
use uuid::Uuid;

use super::{Caller, Denied, DenyReason, Grant, Need, Scope, Target, WriteMode};
use crate::{
    accounts::{self, Owner},
    error::AppError,
    policy::{Permission, PermissionSet},
};

/// May `caller` manage the agent or app account `account_id`? People and
/// the system have no managers: `NotFound`, as for an account the caller
/// can't see.
pub async fn may_manage_account(
    connection: &mut PgConnection,
    caller: &Caller,
    account_id: Uuid,
) -> Result<Result<Grant, Denied>, AppError> {
    let Some(account) = accounts::load(connection, account_id).await? else {
        return Ok(Err(Denied::NotFound));
    };
    // A disabled agent is still managed (re-enabling it is managing it);
    // acting as one is refused where the credential is resolved.
    if !account.kind.has_owner() {
        return Ok(Err(Denied::NotFound));
    }
    let manage = Need::permission(Permission::AgentsManage).writing(true);
    // A credential that can't write manages nothing, whoever holds it.
    if let Err(denied) = super::check(connection, caller, manage, Target::Account).await? {
        return Ok(Err(denied));
    }
    let granted = Grant {
        permission: Some(Permission::AgentsManage),
        mode: WriteMode::Direct,
    };
    match account.owner {
        Some(Owner::Account(owner)) if owner == caller.account_id => return Ok(Ok(granted)),
        Some(Owner::Workspace(workspace)) => {
            let any = Need::permission(Permission::AgentsManageAny).writing(true);
            if super::check(connection, caller, any, Target::Workspace(workspace))
                .await?
                .is_ok()
            {
                return Ok(Ok(granted));
            }
        }
        _ => {}
    }
    // A named manager, in a workspace the account belongs to.
    let workspaces = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.organization_id
        FROM account_managers manager
        JOIN organization_memberships membership
          ON membership.actor_id = manager.account_id AND membership.enabled
        WHERE manager.account_id = $1 AND manager.manager_account_id = $2
        ORDER BY membership.organization_id
        "#,
    )
    .bind(account_id)
    .bind(caller.account_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut seen = false;
    for workspace in workspaces {
        match super::check(connection, caller, manage, Target::Workspace(workspace)).await? {
            Ok(_) => return Ok(Ok(granted)),
            Err(Denied::NotFound) => {}
            Err(_) => seen = true,
        }
    }
    // Someone who shares a workspace with the account sees it but may not
    // manage it; anyone else doesn't learn it exists.
    if seen || shares_a_workspace(connection, caller.account_id, account_id).await? {
        Ok(Err(Denied::Permission {
            permission: Some(Permission::AgentsManage),
            because: DenyReason::Role,
        }))
    } else {
        Ok(Err(Denied::NotFound))
    }
}

async fn shares_a_workspace(
    connection: &mut PgConnection,
    caller: Uuid,
    account: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM organization_memberships mine
            JOIN organization_memberships theirs
              ON theirs.organization_id = mine.organization_id
             AND theirs.actor_id = $2 AND theirs.enabled
            WHERE mine.actor_id = $1 AND mine.enabled
        )
        "#,
    )
    .bind(caller)
    .bind(account)
    .fetch_one(&mut *connection)
    .await?)
}

/// Nobody grants more than they hold: `grants` (a role's permissions, a
/// token's scope, a delegation limit) is within the caller's holding in
/// `scope`.
pub async fn may_grant(
    connection: &mut PgConnection,
    caller: &Caller,
    scope: Scope,
    grants: PermissionSet,
) -> Result<bool, AppError> {
    if !caller.can_write {
        return Ok(false);
    }
    let Some(holding) = super::holding(connection, caller, scope).await? else {
        return Ok(false);
    };
    Ok(grants.intersect(holding.permissions) == grants)
}

/// The accounts in `candidates` the caller may manage, for lists (the
/// consent screen, connected apps, `agent_list`).
pub async fn manageable(
    connection: &mut PgConnection,
    caller: &Caller,
    candidates: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    let mut managed = Vec::new();
    for account in candidates {
        if may_manage_account(connection, caller, *account)
            .await?
            .is_ok()
        {
            managed.push(*account);
        }
    }
    Ok(managed)
}

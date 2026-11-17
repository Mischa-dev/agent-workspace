//! The workspace's own space (AWD-96, K4.10): one per workspace, of kind
//! `SpaceKind::Workspace`, holding what belongs to the whole workspace
//! instead of to one project: its pages, its boards and its channels.
//!
//! It is a plain space. What sets it apart is data, not code paths:
//!
//! - membership is implicit: every enabled member of the workspace, people
//!   and agents, is a member with their workspace role: everyone when the
//!   space is made, anyone who joins or comes back as their workspace
//!   membership commits, disabled with their other memberships when they
//!   leave, and kept in step by the workspace membership triggers
//!   (`workspace_space_add_member`);
//! - its starting shape (`SpaceKind::starting_modules`) says which modules
//!   it holds, ready for M1's per-space switches;
//! - every workspace has one from the start (`workspace_space_make`);
//! - it is never listed as a project.

use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::SpaceKind;
use crate::error::AppError;

/// The workspace's own space, if it has been made.
pub async fn find(
    connection: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM spaces WHERE workspace_id = $1 AND kind = $2",
        )
        .bind(workspace_id)
        .bind(SpaceKind::Workspace.stored())
        .fetch_optional(&mut *connection)
        .await?,
    )
}

/// The workspace's own space, made if it doesn't exist yet. Every
/// workspace gets one as it is made (`workspace_space_make`, run by a
/// trigger on new workspaces), so this only finds it in practice.
pub async fn ensure(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Uuid, AppError> {
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT workspace_space_make($1)")
            .bind(workspace_id)
            .fetch_one(&mut **transaction)
            .await?,
    )
}

/// The workspace's own space, with the account a member of it, when they're
/// an enabled member of the workspace; `None` when they aren't. Making the
/// space adds everyone in the workspace; later joiners are added as their
/// workspace membership commits (`workspace_space_add_member`).
pub async fn join(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    account_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let member = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM organization_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND membership.enabled
        )
        "#,
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !member {
        return Ok(None);
    }
    let space_id = ensure(transaction, workspace_id).await?;
    sqlx::query("SELECT workspace_space_add_member($1, $2)")
        .bind(space_id)
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(Some(space_id))
}

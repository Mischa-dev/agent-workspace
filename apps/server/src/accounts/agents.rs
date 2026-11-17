//! One way to create an agent account (K1.6). An agent belongs to its
//! owner (a person or a workspace, ruling R19) and joins a workspace by a
//! membership with a role, like anyone. Its creator is named as one of its
//! managers. Connections, credentials and the spaces it joins are added by
//! the caller.

use sqlx::PgConnection;
use uuid::Uuid;

use super::Owner;

/// An agent account to make.
#[derive(Clone, Copy, Debug)]
pub struct NewAgent<'a> {
    /// The workspace it joins first.
    pub workspace_id: Uuid,
    /// Its built-in role there, and the custom role when it holds one.
    pub role: &'a str,
    pub role_id: Option<Uuid>,
    pub owner: Owner,
    /// Who made it: named as one of its managers.
    pub created_by: Uuid,
}

/// Make an agent account, its workspace membership, and name its creator
/// as a manager. Returns the account's id.
pub async fn create_agent(
    connection: &mut PgConnection,
    new: NewAgent<'_>,
) -> Result<Uuid, sqlx::Error> {
    let account_id = Uuid::now_v7();
    let (owner_account, owner_workspace) = match new.owner {
        Owner::Account(account) => (Some(account), None),
        Owner::Workspace(workspace) => (None, Some(workspace)),
    };
    sqlx::query(
        "INSERT INTO actors \
             (id, kind, account_kind, status, owner_account_id, owner_workspace_id, created_by_actor_id) \
         VALUES ($1, 'external_agent', 'agent', 'active', $2, $3, $4)",
    )
    .bind(account_id)
    .bind(owner_account)
    .bind(owner_workspace)
    .bind(new.created_by)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, role_id, role_explicit) \
         VALUES ($1, $2, $3, $4, $4 IS NOT NULL)",
    )
    .bind(new.workspace_id)
    .bind(account_id)
    .bind(new.role)
    .bind(new.role_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO account_managers (account_id, manager_account_id, workspace_id, added_by_actor_id) \
         VALUES ($1, $2, $3, $2) ON CONFLICT DO NOTHING",
    )
    .bind(account_id)
    .bind(new.created_by)
    .bind(new.workspace_id)
    .execute(&mut *connection)
    .await?;
    Ok(account_id)
}

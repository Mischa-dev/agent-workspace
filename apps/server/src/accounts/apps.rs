//! One way to create an app account and a run's worker account (K1.31).
//! Like an agent, each belongs to an owner from the moment it exists (ruling
//! R19), so `actors_owner` holds for every row.

use sqlx::PgConnection;
use uuid::Uuid;

use super::Owner;

const fn owner_columns(owner: Owner) -> (Option<Uuid>, Option<Uuid>) {
    match owner {
        Owner::Account(account) => (Some(account), None),
        Owner::Workspace(workspace) => (None, Some(workspace)),
    }
}

/// Make an app account (an integration, or the account a host signs in
/// as) belonging to `owner`. Returns its id.
pub async fn create_app(
    connection: &mut PgConnection,
    owner: Owner,
    created_by: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    let account_id = Uuid::now_v7();
    let (owner_account, owner_workspace) = owner_columns(owner);
    sqlx::query(
        "INSERT INTO actors \
             (id, kind, account_kind, status, owner_account_id, owner_workspace_id, created_by_actor_id) \
         VALUES ($1, 'integration', 'app', 'active', $2, $3, $4)",
    )
    .bind(account_id)
    .bind(owner_account)
    .bind(owner_workspace)
    .bind(created_by)
    .execute(&mut *connection)
    .await?;
    Ok(account_id)
}

/// Make the unlisted worker account `id` that a space's runs act through,
/// belonging to the space's workspace, unless it exists already.
pub async fn ensure_worker(
    connection: &mut PgConnection,
    id: Uuid,
    workspace: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO actors (id, kind, account_kind, status, owner_workspace_id, listed) \
         VALUES ($1, 'execution_worker', 'agent', 'active', $2, false) \
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(workspace)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

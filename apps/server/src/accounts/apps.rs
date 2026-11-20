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
    // The development seed also runs on schemas from before accounts had
    // owners (the upgrade tests seed old data with it), where the worker is
    // a bare row and the K1.5 migration gives it its owner later.
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_attribute \
         WHERE attrelid = 'actors'::regclass AND attname = 'owner_workspace_id' AND NOT attisdropped)",
    )
    .fetch_one(&mut *connection)
    .await?;
    let insert = if owned {
        sqlx::query(
            "INSERT INTO actors (id, kind, owner_workspace_id, listed) \
             VALUES ($1, 'execution_worker', $2, false) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(workspace)
    } else {
        sqlx::query(
            "INSERT INTO actors (id, kind) VALUES ($1, 'execution_worker') ON CONFLICT DO NOTHING",
        )
        .bind(id)
    };
    insert.execute(&mut *connection).await?;
    Ok(())
}

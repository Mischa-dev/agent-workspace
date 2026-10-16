//! Test helpers for provider accounts (S6): a profile now points at an
//! account that holds the sign-in and the private home directory.

use sqlx::PgConnection;
use uuid::Uuid;

/// Inserts an account for `owner_actor_id` in the project's workspace on the
/// connection's harness, and a profile in the project that uses it. Returns
/// the account id.
#[allow(clippy::too_many_arguments)]
pub async fn insert_account_profile(
    connection: &mut PgConnection,
    profile_id: Uuid,
    project_id: Uuid,
    provider_connection_id: Uuid,
    owner_actor_id: Uuid,
    name: &str,
    home_path: &str,
    status: &str,
    account_state: &str,
) -> Result<Uuid, sqlx::Error> {
    let account_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO provider_accounts (
            id, organization_id, owner_actor_id, adapter_kind, name, home_path,
            status, account_state
        )
        SELECT $1, project.organization_id, $3, connection.kind, $5, $6,
               CASE WHEN $7 = 'disabled' THEN 'authentication_required' ELSE $7 END,
               $8
        FROM projects project
        JOIN provider_connections connection ON connection.id = $4
        WHERE project.id = $2
        "#,
    )
    .bind(account_id)
    .bind(project_id)
    .bind(owner_actor_id)
    .bind(provider_connection_id)
    .bind(name)
    .bind(home_path)
    .bind(status)
    .bind(account_state)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id, name,
            provider_account_id, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(provider_connection_id)
    .bind(owner_actor_id)
    .bind(name)
    .bind(account_id)
    .bind(status)
    .execute(&mut *connection)
    .await?;
    Ok(account_id)
}

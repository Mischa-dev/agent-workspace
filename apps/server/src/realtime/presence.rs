//! Presence (K5.10): who has a live connection open, on any instance. Each
//! connection is a row in `realtime_connections`, renewed every 15 seconds;
//! one older than [`FRESH`] no longer counts, so an instance that stopped
//! without closing its connections drops out by itself. Whether someone is
//! online is read from these rows, never from one instance's memory.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

/// How long a connection counts without being renewed.
pub const FRESH_SECONDS: i64 = 60;
/// Rows this much older than fresh are deleted when a connection opens.
const SWEEP_SECONDS: i64 = 600;

/// Records a connection that just opened; true when it is the account's
/// only one, so it has just come online.
pub(crate) async fn open(
    pool: &PgPool,
    connection: Uuid,
    account: Uuid,
    instance: Uuid,
) -> Result<bool, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    sqlx::query(
        "DELETE FROM realtime_connections WHERE seen_at < now() - make_interval(secs => $1)",
    )
    .bind(SWEEP_SECONDS as f64)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO realtime_connections (id, account_id, instance_id) VALUES ($1, $2, $3)",
    )
    .bind(connection)
    .bind(account)
    .bind(instance)
    .execute(&mut *transaction)
    .await?;
    let others = fresh_others(&mut transaction, account, connection).await?;
    transaction.commit().await?;
    Ok(others == 0)
}

/// Renews a connection's row.
pub(crate) async fn renew(pool: &PgPool, connection: Uuid) -> Result<(), AppError> {
    sqlx::query("UPDATE realtime_connections SET seen_at = now() WHERE id = $1")
        .bind(connection)
        .execute(crate::db::scoped(pool))
        .await?;
    Ok(())
}

/// Forgets a connection that closed; true when the account has no other,
/// so it has just gone offline.
pub(crate) async fn close(
    pool: &PgPool,
    connection: Uuid,
    account: Uuid,
) -> Result<bool, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    sqlx::query("DELETE FROM realtime_connections WHERE id = $1")
        .bind(connection)
        .execute(&mut *transaction)
        .await?;
    let others = fresh_others(&mut transaction, account, connection).await?;
    transaction.commit().await?;
    Ok(others == 0)
}

async fn fresh_others(
    connection: &mut sqlx::PgConnection,
    account: Uuid,
    except: Uuid,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM realtime_connections \
         WHERE account_id = $1 AND id <> $2 AND seen_at > now() - make_interval(secs => $3)",
    )
    .bind(account)
    .bind(except)
    .bind(FRESH_SECONDS as f64)
    .fetch_one(&mut *connection)
    .await?)
}

/// The accounts online in a workspace now: an enabled member with a fresh
/// connection on any instance.
pub async fn online_in(pool: &PgPool, workspace: Uuid) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT DISTINCT member.actor_id
        FROM organization_memberships member
        JOIN realtime_connections live ON live.account_id = member.actor_id
        WHERE member.organization_id = $1 AND member.enabled
          AND live.seen_at > now() - make_interval(secs => $2)
        ORDER BY member.actor_id
        "#,
    )
    .bind(workspace)
    .bind(FRESH_SECONDS as f64)
    .fetch_all(crate::db::scoped(pool))
    .await?)
}

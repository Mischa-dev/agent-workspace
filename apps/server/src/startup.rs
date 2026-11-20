//! Startup under a lease (H1.15): the work a server does once before it
//! listens (legacy preflights, migrations, role reconciliation, default
//! workflows) runs under the `server.startup` lease, so two processes
//! started at once against one database never run it side by side. The
//! one that takes the lease does it; the others wait, then find nothing
//! left to do. A process that waits longer than its limit exits non-zero,
//! so the orchestrator starts it again.
//!
//! The lease lives in the `leases` table (K5.2). A database without that
//! table yet (a first install, or one from before K5) has nothing to share
//! the work with but another first start; the migrator's own lock covers
//! that once, and the next start takes the lease.

use std::{future::Future, time::Duration};

use sqlx::PgPool;
use uuid::Uuid;

/// The lease's name; the same as `LeaseName::StartupMaintenance`.
pub const LEASE: &str = "server.startup";
/// How long the lease lives between renewals.
const TTL: Duration = Duration::from_secs(60);
/// How often a holder renews it, and a waiter looks again.
const RENEW: Duration = Duration::from_secs(5);

/// What a waiting process saw.
#[derive(Debug, PartialEq, Eq)]
pub enum Waited {
    /// It took the lease.
    Took(i64),
    /// No `leases` table yet: nothing to take.
    NoTable,
}

async fn leases_exist(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT to_regclass('leases') IS NOT NULL")
        .fetch_one(crate::db::scoped(pool))
        .await
}

/// Takes the lease for `holder`, or renews it; the token when held.
async fn take(pool: &PgPool, holder: Uuid) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO leases AS l (name, holder_instance_id, token, acquired_at, expires_at)
        VALUES ($1, $2, 1, now(), now() + make_interval(secs => $3))
        ON CONFLICT (name) DO UPDATE SET
            token = CASE WHEN l.holder_instance_id = EXCLUDED.holder_instance_id
                              AND l.expires_at > now()
                         THEN l.token ELSE l.token + 1 END,
            acquired_at = CASE WHEN l.holder_instance_id = EXCLUDED.holder_instance_id
                                    AND l.expires_at > now()
                               THEN l.acquired_at ELSE now() END,
            holder_instance_id = EXCLUDED.holder_instance_id,
            expires_at = EXCLUDED.expires_at
        WHERE l.expires_at <= now() OR l.holder_instance_id = EXCLUDED.holder_instance_id
        RETURNING token
        "#,
    )
    .bind(LEASE)
    .bind(holder)
    .bind(TTL.as_secs_f64())
    .fetch_optional(crate::db::scoped(pool))
    .await
}

async fn release(pool: &PgPool, holder: Uuid, token: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE leases SET expires_at = now() \
         WHERE name = $1 AND holder_instance_id = $2 AND token = $3 AND expires_at > now()",
    )
    .bind(LEASE)
    .bind(holder)
    .bind(token)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(())
}

/// Waits until `holder` takes the lease, for at most `limit`.
pub async fn wait_for(pool: &PgPool, holder: Uuid, limit: Duration) -> anyhow::Result<Waited> {
    let started = tokio::time::Instant::now();
    loop {
        if !leases_exist(pool).await? {
            return Ok(Waited::NoTable);
        }
        if let Some(token) = take(pool, holder).await? {
            return Ok(Waited::Took(token));
        }
        if started.elapsed() >= limit {
            anyhow::bail!(
                "another server held {LEASE} for longer than {}s; exiting so this one is \
                 started again",
                limit.as_secs()
            );
        }
        tokio::time::sleep(RENEW).await;
    }
}

/// Runs `work` under the lease: waits for it (reporting "waiting" through
/// `waiting`), keeps it while `work` runs, and lets it go after. `work`
/// runs on this task (rule 5): the renewals share it through `select!`.
pub async fn under_lease<T>(
    pool: &PgPool,
    limit: Duration,
    waiting: impl FnOnce(),
    work: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let holder = Uuid::now_v7();
    let token = match tokio::time::timeout(RENEW, wait_for(pool, holder, Duration::ZERO)).await {
        Ok(Ok(waited)) => waited,
        // Held elsewhere (or slow to answer): say so, then wait in earnest.
        Ok(Err(_)) | Err(_) => {
            waiting();
            wait_for(pool, holder, limit).await?
        }
    };
    let Waited::Took(token) = token else {
        return work.await;
    };
    tokio::pin!(work);
    let mut renew = tokio::time::interval(RENEW);
    renew.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut work => break result,
            _ = renew.tick() => {
                match take(pool, holder).await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        break Err(anyhow::anyhow!(
                            "{LEASE} was taken over while starting; exiting"
                        ));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "{LEASE} not renewed; trying again");
                    }
                }
            }
        }
    };
    if let Err(error) = release(pool, holder, token).await {
        tracing::warn!(%error, "{LEASE} not released; it runs out instead");
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_lease_is_the_one_the_cluster_names() {
        assert_eq!(
            super::LEASE,
            crate::cluster::LeaseName::StartupMaintenance.to_string()
        );
    }
}

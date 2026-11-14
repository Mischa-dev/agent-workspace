//! Leases (K5 3.2): one holder at a time for work that must not run twice,
//! with a fencing token that grows on every takeover. A lease lives
//! `ttl`; its holder renews all it holds in one statement with the
//! heartbeat, and another instance may take it only after it expires.

use std::{fmt, future::Future, time::Duration};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::Cluster;
use crate::error::AppError;

/// A lease's name. Kinds are listed here so names can't drift.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum LeaseName {
    /// The built-in host: child processes and disk on one machine (K5.3).
    BuiltinHost,
    /// One-off work at startup (the name H1 uses).
    StartupMaintenance,
    /// An ordered consumer of the event log (K3, A2).
    Consumer(&'static str),
    /// One partition of a partitioned consumer.
    Partition(&'static str, u16),
}

impl fmt::Display for LeaseName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuiltinHost => formatter.write_str("builtin-host"),
            Self::StartupMaintenance => formatter.write_str("server.startup"),
            Self::Consumer(name) => write!(formatter, "consumer:{name}"),
            Self::Partition(name, number) => write!(formatter, "partition:{name}.{number}"),
        }
    }
}

/// A lease this instance holds.
#[derive(Clone, Debug)]
pub struct Lease {
    pub name: LeaseName,
    pub token: i64,
    /// Cancelled when the lease is lost or released.
    pub lost: CancellationToken,
}

/// Take `name` for `ttl`, or renew it if this instance holds it. The token
/// when taken; None while another instance holds it.
pub(super) async fn try_acquire(
    cluster: &Cluster,
    name: &str,
    ttl: Duration,
) -> Result<Option<i64>, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
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
    .bind(name)
    .bind(cluster.instance_id().0)
    .bind(ttl.as_secs_f64())
    .fetch_optional(cluster.pool())
    .await?)
}

/// Let `name` go at once, and wake whoever waits for it.
pub(super) async fn release(cluster: &Cluster, name: &str, token: i64) -> Result<(), AppError> {
    let mut transaction = cluster.pool().begin().await?;
    let released = sqlx::query(
        "UPDATE leases SET expires_at = now() \
         WHERE name = $1 AND holder_instance_id = $2 AND token = $3 AND expires_at > now()",
    )
    .bind(name)
    .bind(cluster.instance_id().0)
    .bind(token)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if released > 0 {
        // Delivered at commit on the control channel (K5.4's listener); a
        // standby elsewhere also retries on its own.
        sqlx::query("SELECT pg_notify(realtime_channel('control'), $1)")
            .bind(format!("lease:{name}"))
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Inside the caller's transaction: hold the lease row (`FOR SHARE`) so a
/// takeover waits for this transaction, or fail if the lease is gone.
pub(super) async fn fence(
    transaction: &mut sqlx::PgConnection,
    cluster: &Cluster,
    lease: &Lease,
) -> Result<(), AppError> {
    let held = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM leases \
         WHERE name = $1 AND holder_instance_id = $2 AND token = $3 AND expires_at > now() \
         FOR SHARE",
    )
    .bind(lease.name.to_string())
    .bind(cluster.instance_id().0)
    .bind(lease.token)
    .fetch_optional(&mut *transaction)
    .await?;
    if held.is_none() {
        return Err(AppError::LeaseLost);
    }
    Ok(())
}

/// The heartbeat and the renewal of `names` in one statement; the names
/// and tokens still held. Only leases this instance still believes it holds
/// are renewed, so one it gave up (self-fencing) runs out.
pub(super) async fn renew(
    cluster: &Cluster,
    names: &[String],
    ttl: Duration,
) -> Result<Vec<(String, i64)>, AppError> {
    let mut transaction = cluster.pool().begin().await?;
    // A blocked renewal fails rather than waits: the holder fences itself
    // long before this runs out.
    sqlx::query(&format!(
        "SET LOCAL lock_timeout = '{}ms'",
        ttl.as_millis().max(1)
    ))
    .execute(&mut *transaction)
    .await?;
    let renewed = sqlx::query_as::<_, (String, i64)>(
        r#"
        WITH beat AS (
            UPDATE server_instances SET heartbeat_at = now()
            WHERE id = $1 AND stopped_at IS NULL RETURNING id
        )
        UPDATE leases SET expires_at = now() + make_interval(secs => $2)
        WHERE holder_instance_id = (SELECT id FROM beat) AND expires_at > now()
          AND name = ANY($3)
        RETURNING name, token
        "#,
    )
    .bind(cluster.instance_id().0)
    .bind(ttl.as_secs_f64())
    .bind(names)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(renewed)
}

/// Runs `work` whenever this instance holds `name`. Tries again every
/// third of the lease's time to live and whenever this instance releases a
/// lease. `work` must return soon after `lease.lost` is cancelled, which
/// also happens at shutdown; the lease is then released.
pub async fn while_held<F, Fut>(
    cluster: Cluster,
    name: LeaseName,
    mut shutdown: watch::Receiver<bool>,
    work: F,
) where
    F: Fn(Lease) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), AppError>> + Send,
{
    loop {
        if *shutdown.borrow() {
            return;
        }
        let retry = cluster.renew_every();
        match cluster.try_acquire(name.clone()).await {
            Ok(Some(lease)) => {
                let running = work(lease.clone());
                tokio::pin!(running);
                let result = tokio::select! {
                    result = &mut running => result,
                    _ = shutdown.changed() => {
                        lease.lost.cancel();
                        running.await
                    }
                };
                if let Err(error) = result {
                    tracing::warn!(
                        lease = %name,
                        error_category = error.log_category(),
                        "work under a lease failed"
                    );
                }
                if let Err(error) = cluster.release(&lease).await {
                    tracing::warn!(
                        lease = %name,
                        error_category = error.log_category(),
                        "lease not released; it runs out instead"
                    );
                }
                // Let a standby take it before trying again.
                tokio::select! {
                    () = tokio::time::sleep(retry) => {}
                    _ = shutdown.changed() => {}
                }
            }
            Ok(None) => {
                tokio::select! {
                    () = tokio::time::sleep(retry) => {}
                    () = cluster.released() => {}
                    _ = shutdown.changed() => {}
                }
            }
            Err(error) => {
                tracing::warn!(
                    lease = %name,
                    error_category = error.log_category(),
                    "lease not taken"
                );
                tokio::select! {
                    () = tokio::time::sleep(retry) => {}
                    _ = shutdown.changed() => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table's check, as a function: `kind` or `kind:detail`.
    fn well_formed(name: &str) -> bool {
        let (kind, detail) = match name.split_once(':') {
            Some((kind, detail)) => (kind, Some(detail)),
            None => (name, None),
        };
        let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c);
        name.len() <= 160
            && kind.starts_with(|c: char| c.is_ascii_lowercase())
            && kind.chars().all(allowed)
            && detail.is_none_or(|detail| {
                (1..=120).contains(&detail.len()) && detail.chars().all(allowed)
            })
    }

    #[test]
    fn lease_names_have_the_stored_format() {
        let names = [
            LeaseName::BuiltinHost,
            LeaseName::StartupMaintenance,
            LeaseName::Consumer("webhooks"),
            LeaseName::Partition("deliveries", 3),
        ]
        .map(|name| name.to_string());
        assert_eq!(
            names,
            [
                "builtin-host",
                "server.startup",
                "consumer:webhooks",
                "partition:deliveries.3"
            ]
        );
        assert!(names.iter().all(|name| well_formed(name)));
        assert!(!well_formed("Consumer:x"));
        assert!(!well_formed("consumer:"));
    }
}

//! A1.9: which server process drives a built-in run (A1 section 3.11,
//! "Recovery with leases").
//!
//! The process that starts a run's turn takes the run's lease and renews it
//! while it drives the turn. Recovery takes a run only when its lease is
//! free (a run from before leases, or one no process has picked up), has
//! lapsed (its driver stopped renewing: the process died or lost the
//! database), or is this process's own; and it takes it under
//! `FOR UPDATE SKIP LOCKED`, so two processes recovering at once never both
//! resume one run. A run whose driver is alive elsewhere is left alone.
//!
//! This is the stopgap the plan names until K5.3's single-holder
//! `builtin-host` lease: the columns `agent_runs.driver_instance_id` and
//! `driver_lease_expires_at` go when K5.3 lands.

use std::{sync::OnceLock, time::Duration};

use sqlx::{PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, runs::ACTIVE_RUN_STATES};

/// How long a lease lasts without a renewal.
pub const LEASE: Duration = Duration::from_secs(90);
/// How often a driver renews (well inside [`LEASE`]).
pub const RENEW_EVERY: Duration = Duration::from_secs(30);

/// This process's identity as a driver. Fixed for the life of the process;
/// nothing else is kept in memory, so any process can serve any request.
pub fn instance_id() -> Uuid {
    static INSTANCE: OnceLock<Uuid> = OnceLock::new();
    *INSTANCE.get_or_init(Uuid::now_v7)
}

fn lease_seconds() -> f64 {
    LEASE.as_secs_f64()
}

/// Take (or renew) the lease on a run this process is about to drive.
/// Returns false when another process holds a live lease on it.
pub async fn take(executor: impl PgExecutor<'_>, run_id: Uuid) -> Result<bool, AppError> {
    let taken = sqlx::query(
        r#"
        UPDATE agent_runs
           SET driver_instance_id = $2,
               driver_lease_expires_at = now() + make_interval(secs => $3)
         WHERE id = $1
           AND state = ANY($4)
           AND (driver_instance_id IS NULL
                OR driver_instance_id = $2
                OR driver_lease_expires_at <= now())
        "#,
    )
    .bind(run_id)
    .bind(instance_id())
    .bind(lease_seconds())
    .bind(&ACTIVE_RUN_STATES[..])
    .execute(executor)
    .await?;
    Ok(taken.rows_affected() == 1)
}

/// Renew this process's lease on a run it drives. Returns false when the
/// lease is no longer this process's (the run finished, or it lapsed and
/// another process recovered the run): the driver should stop.
pub async fn renew(executor: impl PgExecutor<'_>, run_id: Uuid) -> Result<bool, AppError> {
    let renewed = sqlx::query(
        r#"
        UPDATE agent_runs
           SET driver_lease_expires_at = now() + make_interval(secs => $3)
         WHERE id = $1 AND driver_instance_id = $2
        "#,
    )
    .bind(run_id)
    .bind(instance_id())
    .bind(lease_seconds())
    .execute(executor)
    .await?;
    Ok(renewed.rows_affected() == 1)
}

/// Which runs recovery may look at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Recover {
    /// At start-up: runs with no lease (as before leases), lapsed leases and
    /// this process's own.
    AtStart,
    /// While running: only runs whose driver held a lease and stopped
    /// renewing it. A run nobody has leased is not this sweep's to take.
    Lapsed,
}

/// The runs recovery may take now, oldest first. Each is taken again under
/// [`claim_for_recovery`] before anything is done to it.
pub async fn recoverable(
    executor: impl PgExecutor<'_>,
    which: Recover,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM agent_runs
        WHERE state IN ('starting', 'working', 'blocked', 'recovering')
          AND CASE WHEN $2
                   THEN driver_instance_id IS NOT NULL AND driver_lease_expires_at <= now()
                   ELSE driver_instance_id IS NULL
                     OR driver_instance_id = $1
                     OR driver_lease_expires_at <= now()
              END
        ORDER BY created_at, id
        "#,
    )
    .bind(instance_id())
    .bind(which == Recover::Lapsed)
    .fetch_all(executor)
    .await?)
}

/// Lock one run for recovery and make this process its driver, or `None`
/// when another process got to it first (it holds the row, or took a live
/// lease since [`recoverable`] listed it), or it has finished.
pub async fn claim_for_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<String>, AppError> {
    let Some(state) = sqlx::query_scalar::<_, String>(
        r#"
        SELECT state FROM agent_runs
        WHERE id = $1
          AND state IN ('starting', 'working', 'blocked', 'recovering')
          AND (driver_instance_id IS NULL
               OR driver_instance_id = $2
               OR driver_lease_expires_at <= now())
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(run_id)
    .bind(instance_id())
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(None);
    };
    take(&mut **transaction, run_id).await?;
    Ok(Some(state))
}

/// A driver's renewals: [`Renewal::keep`] renews at most every
/// [`RENEW_EVERY`], so a loop may call it on every tick.
#[derive(Debug)]
pub struct Renewal {
    last: std::time::Instant,
}

impl Default for Renewal {
    fn default() -> Self {
        Self {
            last: std::time::Instant::now(),
        }
    }
}

impl Renewal {
    /// Renew when due. Returns false when this process no longer drives the
    /// run, and the caller must stop driving it.
    pub async fn keep(
        &mut self,
        executor: impl PgExecutor<'_>,
        run_id: Uuid,
    ) -> Result<bool, AppError> {
        if self.last.elapsed() < RENEW_EVERY {
            return Ok(true);
        }
        self.last = std::time::Instant::now();
        renew(executor, run_id).await
    }
}

/// The job that recovers runs whose driver stopped renewing.
pub const RECOVER_LAPSED_KIND: &str = "builtin_host.recover_lapsed";

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register_with_timeout(
        RECOVER_LAPSED_KIND,
        Duration::from_secs(600),
        |context: crate::jobs::JobContext| async move {
            crate::recovery::recover_lapsed_runs(&context.state)
                .await
                .map_err(|error| crate::jobs::JobError::retry(error.log_category()))?;
            Ok(())
        },
    );
    registry.every(RECOVER_LAPSED_KIND, Duration::from_secs(60));
}

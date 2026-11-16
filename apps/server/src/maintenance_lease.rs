//! Leases for server-wide maintenance that must not overlap (H1.16): a
//! backup's dump and a master key rotation both take [`KEYRING`], so a dump
//! never holds secrets sealed under two keys.
//!
//! A lease is a `maintenance_leases` row with a holder and an expiry on the
//! database's clock. It is taken by a conditional insert, renewed while the
//! work runs and released at the end; every step is one short transaction,
//! so it works through a transaction-mode pooler where a session advisory
//! lock would not (R24, AGENTS.md rule 14). A holder that dies leaves a row
//! that expires. K5.2's lease service replaces this module.

use std::{future::Future, time::Duration};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{db, error::AppError, tenancy};

/// The keyring's lease: held by a backup's database dump and by a master
/// key rotation (and by `deploy/redhat/agent-workspace-backup`).
pub const KEYRING: &str = "secrets.keyring";

/// How long a lease lasts without renewal.
const TIME_TO_LIVE_SECONDS: f64 = 60.0;
/// How often a held lease is renewed: well inside its time to live.
const RENEW_EVERY: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("{name} is in use by another process until {until}; try again then")]
    Held {
        name: &'static str,
        until: DateTime<Utc>,
    },
    #[error("lost the {0} lease while working; the work was abandoned")]
    Lost(&'static str),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<LeaseError> for AppError {
    fn from(error: LeaseError) -> Self {
        match error {
            LeaseError::Held { .. } => AppError::Conflict(
                "keyring",
                "a backup or another key rotation is running; try again when it finishes",
            ),
            LeaseError::Lost(_) => AppError::Conflict(
                "keyring",
                "the lease was lost partway; run it again, it picks up where it stopped",
            ),
            LeaseError::Database(error) => AppError::Database(error),
        }
    }
}

/// A lease this process holds. Release it with [`Lease::release`]; one
/// dropped without that expires on its own.
#[derive(Debug)]
pub struct Lease {
    name: &'static str,
    holder: Uuid,
}

impl Lease {
    /// Takes `name` if nobody holds it or its holder let it expire.
    pub async fn take(pool: &PgPool, name: &'static str) -> Result<Self, LeaseError> {
        let holder = Uuid::new_v4();
        let outcome = tenancy::system(
            "maintenance.lease",
            db::scoped_one(pool, async |connection| {
                let taken = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO maintenance_leases AS lease (name, holder, acquired_at, expires_at)
                    VALUES ($1, $2, now(), now() + make_interval(secs => $3))
                    ON CONFLICT (name) DO UPDATE
                    SET holder = EXCLUDED.holder,
                        acquired_at = EXCLUDED.acquired_at,
                        expires_at = EXCLUDED.expires_at
                    WHERE lease.expires_at <= now()
                    RETURNING holder
                    "#,
                )
                .bind(name)
                .bind(holder)
                .bind(TIME_TO_LIVE_SECONDS)
                .fetch_optional(&mut *connection)
                .await?;
                if taken.is_some() {
                    return Ok::<_, sqlx::Error>(None);
                }
                sqlx::query_scalar::<_, DateTime<Utc>>(
                    "SELECT expires_at FROM maintenance_leases WHERE name = $1",
                )
                .bind(name)
                .fetch_optional(&mut *connection)
                .await
            }),
        )
        .await?;
        match outcome {
            None => Ok(Self { name, holder }),
            Some(until) => Err(LeaseError::Held { name, until }),
        }
    }

    /// Extends the lease, if this process still holds it.
    pub async fn renew(&self, pool: &PgPool) -> Result<(), LeaseError> {
        let renewed = tenancy::system(
            "maintenance.lease",
            db::scoped_one(pool, async |connection| {
                sqlx::query(
                    r#"
                    UPDATE maintenance_leases
                    SET expires_at = now() + make_interval(secs => $3)
                    WHERE name = $1 AND holder = $2 AND expires_at > now()
                    "#,
                )
                .bind(self.name)
                .bind(self.holder)
                .bind(TIME_TO_LIVE_SECONDS)
                .execute(connection)
                .await
            }),
        )
        .await?;
        if renewed.rows_affected() == 1 {
            Ok(())
        } else {
            Err(LeaseError::Lost(self.name))
        }
    }

    /// Runs `work`, renewing the lease while it runs, and confirms the lease
    /// was held throughout. Lost on the way, `work` is dropped and the
    /// result is [`LeaseError::Lost`].
    pub async fn hold_while<T>(
        &self,
        pool: &PgPool,
        work: impl Future<Output = T>,
    ) -> Result<T, LeaseError> {
        let renewals = async {
            loop {
                tokio::time::sleep(RENEW_EVERY).await;
                if let Err(error) = self.renew(pool).await {
                    return error;
                }
            }
        };
        let value = tokio::select! {
            value = work => value,
            error = renewals => return Err(error),
        };
        self.renew(pool).await?;
        Ok(value)
    }

    /// Gives the lease up, so the next taker needn't wait for it to expire.
    pub async fn release(self, pool: &PgPool) -> Result<(), LeaseError> {
        tenancy::system(
            "maintenance.lease",
            db::scoped_one(pool, async |connection| {
                sqlx::query("DELETE FROM maintenance_leases WHERE name = $1 AND holder = $2")
                    .bind(self.name)
                    .bind(self.holder)
                    .execute(connection)
                    .await
            }),
        )
        .await?;
        Ok(())
    }
}

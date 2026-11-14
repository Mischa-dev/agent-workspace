//! Any number of servers (K5 3.2): each process registers as an instance in
//! `server_instances`, beats every third of the lease time to live, and
//! holds leases for the work that must run in only one place at a time.
//!
//! Rules: every time is the database's clock; a lease is renewed with the
//! heartbeat in one statement; another instance takes it only after it
//! expires; an instance that hasn't renewed for two thirds of the time to
//! live (on its own monotonic clock, from the start of its last good
//! renewal) treats every lease as lost and stops the work under them before
//! anyone else may take over. A write that must happen only under a lease
//! calls `fence` in its own transaction.

pub mod instance;
pub mod leases;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sqlx::PgPool;
use tokio::{sync::Notify, sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;

pub use instance::{InstanceId, Roles};
pub use leases::{Lease, LeaseName, while_held};

use crate::error::AppError;

/// The default time to live of a lease.
pub const LEASE_TTL: Duration = Duration::from_secs(15);

/// This instance, its leases and the heartbeat's state. Clones share them.
#[derive(Clone)]
pub struct Cluster {
    inner: Arc<Inner>,
}

struct Inner {
    pool: PgPool,
    id: InstanceId,
    roles: Roles,
    ttl: Duration,
    /// The leases this process holds. They are this process's own by
    /// definition (the database has the shared truth); a short list.
    held: Mutex<Vec<Lease>>,
    /// Wakes `while_held` loops here when a lease is released, here or (by
    /// the wake listener) on another instance.
    released: Arc<Notify>,
}

impl Cluster {
    /// Register this process as a new instance.
    pub async fn register(
        pool: PgPool,
        roles: Roles,
        label: Option<String>,
        ttl: Duration,
    ) -> Result<Self, AppError> {
        let id = InstanceId::new();
        sqlx::query(
            "INSERT INTO server_instances (id, roles, label, release) VALUES ($1, $2, $3, $4)",
        )
        .bind(id.0)
        .bind(roles.names())
        .bind(label)
        .bind(env!("CARGO_PKG_VERSION"))
        .execute(&pool)
        .await?;
        Ok(Self {
            inner: Arc::new(Inner {
                pool,
                id,
                roles,
                ttl,
                held: Mutex::new(Vec::new()),
                released: Arc::new(Notify::new()),
            }),
        })
    }

    pub fn instance_id(&self) -> InstanceId {
        self.inner.id
    }

    pub fn roles(&self) -> Roles {
        self.inner.roles
    }

    pub fn ttl(&self) -> Duration {
        self.inner.ttl
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.inner.pool
    }

    /// How often the heartbeat renews, and how often a standby tries.
    pub fn renew_every(&self) -> Duration {
        self.inner.ttl / 3
    }

    /// How long without a good renewal before this instance fences itself.
    pub fn fence_after(&self) -> Duration {
        self.inner.ttl * 2 / 3
    }

    /// Take `name`, or None while another instance holds it. Taking a lease
    /// this instance already holds returns the one it holds.
    pub async fn try_acquire(&self, name: LeaseName) -> Result<Option<Lease>, AppError> {
        let key = name.to_string();
        let Some(token) = leases::try_acquire(self, &key, self.inner.ttl).await? else {
            return Ok(None);
        };
        let mut held = self.held();
        if let Some(lease) = held
            .iter()
            .find(|lease| lease.name == name && lease.token == token && !lease.lost.is_cancelled())
        {
            return Ok(Some(lease.clone()));
        }
        // A lease of the same name held under an older token is gone.
        held.retain(|lease| {
            let stale = lease.name == name;
            if stale {
                lease.lost.cancel();
            }
            !stale
        });
        let lease = Lease {
            name,
            token,
            lost: CancellationToken::new(),
        };
        held.push(lease.clone());
        Ok(Some(lease))
    }

    /// Let `lease` go at once: its work is cancelled and a standby may take
    /// it without waiting for it to run out.
    pub async fn release(&self, lease: &Lease) -> Result<(), AppError> {
        lease.lost.cancel();
        self.held()
            .retain(|held| !(held.name == lease.name && held.token == lease.token));
        let result = leases::release(self, &lease.name.to_string(), lease.token).await;
        self.inner.released.notify_waiters();
        result
    }

    /// Inside a transaction: fails with `AppError::LeaseLost` unless `lease`
    /// is still held, and keeps it from being taken until the transaction
    /// ends.
    pub async fn fence(
        &self,
        transaction: &mut sqlx::PgConnection,
        lease: &Lease,
    ) -> Result<(), AppError> {
        if lease.lost.is_cancelled() {
            return Err(AppError::LeaseLost);
        }
        leases::fence(transaction, self, lease).await
    }

    pub fn holds(&self, name: &LeaseName) -> bool {
        self.held()
            .iter()
            .any(|lease| &lease.name == name && !lease.lost.is_cancelled())
    }

    /// Resolves when a lease is next released.
    pub(crate) async fn released(&self) {
        self.inner.released.notified().await;
    }

    /// What the wake listener notifies when any instance releases a lease.
    pub fn release_wake(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.released)
    }

    /// One heartbeat: renew what this instance holds; anything not renewed
    /// is lost and its work cancelled.
    pub async fn beat(&self) -> Result<(), AppError> {
        let asked = self
            .held()
            .iter()
            .map(|lease| (lease.name.to_string(), lease.token))
            .collect::<Vec<_>>();
        let names = asked
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let renewed = leases::renew(self, &names, self.inner.ttl).await?;
        let mut orphans = Vec::new();
        {
            let mut held = self.held();
            // Leases taken while the renewal ran weren't asked about.
            held.retain(|lease| {
                let key = (lease.name.to_string(), lease.token);
                let kept = !asked.contains(&key) || renewed.contains(&key);
                if !kept {
                    lease.lost.cancel();
                }
                kept
            });
            // Renewed but given up meanwhile (self-fencing): let them go.
            for (name, token) in &renewed {
                if !held
                    .iter()
                    .any(|lease| lease.name.to_string() == *name && lease.token == *token)
                {
                    orphans.push((name.clone(), *token));
                }
            }
        }
        for (name, token) in orphans {
            leases::release(self, &name, token).await?;
        }
        Ok(())
    }

    /// Treat every lease as lost: their work stops and they aren't renewed,
    /// so they run out.
    pub fn fence_all(&self) {
        let lost = std::mem::take(&mut *self.held());
        if !lost.is_empty() {
            tracing::warn!(
                leases = lost.len(),
                "lease renewal is late; this instance gave up its leases"
            );
        }
        for lease in lost {
            lease.lost.cancel();
        }
    }

    /// The heartbeat loop: renews every third of the time to live, fences
    /// itself when renewal is late, and at shutdown releases what it holds
    /// and marks the instance stopped. A database error is logged and
    /// retried; it never stops the server.
    pub async fn run_heartbeat(&self, mut shutdown: watch::Receiver<bool>) {
        let mut last_good = Instant::now();
        loop {
            if *shutdown.borrow() {
                break;
            }
            let started = Instant::now();
            let fence_at = last_good + self.fence_after();
            let renewal = self.beat();
            tokio::pin!(renewal);
            let mut fenced = false;
            let result = loop {
                tokio::select! {
                    result = &mut renewal => break result,
                    () = tokio::time::sleep_until(fence_at), if !fenced => {
                        self.fence_all();
                        fenced = true;
                    }
                }
            };
            match result {
                Ok(()) => last_good = started,
                Err(error) => {
                    tracing::warn!(
                        error_category = error.log_category(),
                        "instance heartbeat failed"
                    );
                    if Instant::now() >= last_good + self.fence_after() {
                        self.fence_all();
                    }
                }
            }
            let next = started + self.renew_every();
            let deadline = next.min(last_good + self.fence_after());
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => {}
                _ = shutdown.changed() => {}
            }
            if Instant::now() >= last_good + self.fence_after() {
                self.fence_all();
            }
        }
        let held = std::mem::take(&mut *self.held());
        for lease in held {
            if let Err(error) = self.release(&lease).await {
                tracing::warn!(
                    error_category = error.log_category(),
                    "lease not released at shutdown; it runs out instead"
                );
            }
        }
        if let Err(error) = self.mark_stopped().await {
            tracing::warn!(
                error_category = error.log_category(),
                "instance not marked stopped"
            );
        }
    }

    pub async fn mark_draining(&self) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE server_instances SET draining_at = coalesce(draining_at, now()) WHERE id = $1",
        )
        .bind(self.inner.id.0)
        .execute(&self.inner.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_stopped(&self) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE server_instances \
             SET draining_at = coalesce(draining_at, now()), stopped_at = coalesce(stopped_at, now()) \
             WHERE id = $1",
        )
        .bind(self.inner.id.0)
        .execute(&self.inner.pool)
        .await?;
        Ok(())
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Vec<Lease>> {
        self.inner
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

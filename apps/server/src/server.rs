//! The server's assembly (K5.1): the state, the work done once at startup,
//! the listeners and the loops. `main.rs` parses the command line and runs
//! one; the cluster tests run two or more against one database, each with
//! its own state, listener and loops, and stop one gracefully or abruptly.

use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, bail};
use sqlx::PgPool;
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};

use crate::{
    AppState, cluster::Cluster, config::AppConfig, error::AppError, github, jobs,
    provider::SupervisorRegistry, rate_limit::RateLimiters, realtime::RealtimeHub, recovery,
    router,
};

/// A server, assembled and ready to listen.
pub struct Server {
    state: AppState,
    cluster: Cluster,
    jobs: Arc<jobs::JobRegistry>,
}

impl Server {
    /// The state, and the one-time startup work: harnesses, run recovery,
    /// secret migration and the module registries.
    pub async fn build(config: Arc<AppConfig>, pool: PgPool) -> anyhow::Result<Self> {
        let supervisors =
            SupervisorRegistry::new(&config).context("configure provider supervisors")?;
        let state = AppState {
            config: Arc::clone(&config),
            pool: pool.clone(),
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(256),
            supervisors,
        };
        state
            .supervisors
            .set_host(Arc::new(crate::harness_host::ServerHost::new(
                pool.clone(),
                Arc::clone(&config),
            )));
        // Registered harnesses load before runs of their kinds are recovered.
        if let Err(error) = crate::harnesses::reload(&state).await {
            tracing::warn!(
                error_category = error.log_category(),
                "registered harnesses not loaded"
            );
        }
        let recovered = recovery::reconcile_non_terminal_runs(&state)
            .await
            .context("reconcile non-terminal Agent Runs")?;
        if recovered > 0 {
            tracing::warn!(recovered, "reconciled non-terminal Agent Runs");
        }
        let inspected_worktrees = recovery::reconcile_registered_worktrees(&state)
            .await
            .context("reconcile registered Git worktrees")?;
        let canceled_approvals = recovery::cancel_terminal_run_approvals(&state)
            .await
            .context("cancel approvals owned by terminal Agent Runs")?;
        tracing::info!(
            inspected_worktrees,
            canceled_approvals,
            "startup reconciliation completed"
        );

        let moved_secrets = crate::secrets::migrate_plaintext_webhook_secrets(&pool, &config)
            .await
            .context("encrypt plaintext agent webhook secrets")?;
        if moved_secrets > 0 {
            tracing::info!(
                moved_secrets,
                "moved agent webhook secrets into the secrets store"
            );
        }

        // This process as an instance among any number (K5.2); its
        // heartbeat starts with the loops.
        let cluster = Cluster::register(
            pool.clone(),
            crate::cluster::Roles::ALL,
            config.instance_label.clone(),
            config.lease_ttl,
        )
        .await
        .context("register this server instance")?;

        // Modules (and their operations) that don't fit together stop the
        // server here, before it takes a request.
        let modules = crate::modules::registry::Registry::global();
        let operations = crate::ops::registry();
        tracing::info!(
            modules = modules.descriptions().len(),
            operations = operations.entries().count(),
            "modules registered"
        );
        Ok(Self {
            state,
            cluster,
            jobs: Arc::new(jobs::default_registry()),
        })
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn cluster(&self) -> &Cluster {
        &self.cluster
    }

    /// Serve the app on `listener` and GitHub webhooks on `webhooks`, and
    /// start the loops.
    pub fn spawn(self, listener: TcpListener, webhooks: TcpListener) -> RunningServer {
        let Self {
            state,
            cluster,
            jobs,
        } = self;
        let address = listener.local_addr().ok();
        let (shutdown, receiver) = watch::channel(false);
        // Wakes the sequencer; K5.4's listener notifies it.
        let wake = Arc::new(tokio::sync::Notify::new());
        crate::harnesses::spawn_reloader(state.clone());
        let scheduler = tokio::spawn(crate::scheduler::run(state.clone(), receiver.clone()));
        let github = tokio::spawn(github::run_jobs(state.clone(), receiver.clone()));
        let outbox = tokio::spawn(crate::realtime::run_outbox_publisher(
            state.pool.clone(),
            state.realtime.clone(),
            crate::realtime::outbox_worker_id(),
            receiver.clone(),
        ));
        // Not supervised like the loops above: a failure here delays
        // wakeups and lease expiry but never loses data, so it must not
        // stop the server.
        // The log's sequencer and the instance's heartbeat run beside it on
        // the same task, for the same reason: a failed step is retried on
        // the next round (a heartbeat that stays late fences this instance,
        // it never stops it).
        let replies = tokio::spawn({
            let replies = crate::replies::run(state.clone(), receiver.clone());
            let sequencer = crate::events::sequencer::run(
                state.pool.clone(),
                Arc::clone(&wake),
                receiver.clone(),
            );
            let heartbeat_cluster = cluster.clone();
            let heartbeat_shutdown = receiver.clone();
            async move {
                let heartbeat = heartbeat_cluster.run_heartbeat(heartbeat_shutdown);
                tokio::join!(replies, sequencer, heartbeat);
            }
        });
        // Background jobs (F9). A job lost to a crash is retried once its
        // lease runs out, so like the reply worker this doesn't stop the
        // server.
        let job_workers = tokio::spawn(jobs::run_workers(
            state.clone(),
            jobs,
            jobs::WorkerOptions {
                concurrency: state.config.job_concurrency,
                ..jobs::WorkerOptions::default()
            },
            receiver.clone(),
        ));
        let application_shutdown = receiver.clone();
        let webhook_shutdown = receiver;
        let http_state = state.clone();
        let http = tokio::spawn(async move {
            let application = axum::serve(listener, router(http_state.clone()).into_make_service())
                .with_graceful_shutdown(wait_for_shutdown(application_shutdown));
            let webhooks = axum::serve(
                webhooks,
                github::webhook_router(http_state).into_make_service(),
            )
            .with_graceful_shutdown(wait_for_shutdown(webhook_shutdown));
            tokio::try_join!(application, webhooks).map(|_| ())
        });
        RunningServer {
            state,
            cluster,
            address,
            shutdown,
            http,
            scheduler,
            github,
            outbox,
            replies,
            job_workers,
        }
    }
}

/// A server that is listening and running its loops.
pub struct RunningServer {
    state: AppState,
    cluster: Cluster,
    address: Option<SocketAddr>,
    shutdown: watch::Sender<bool>,
    http: JoinHandle<std::io::Result<()>>,
    scheduler: JoinHandle<Result<(), AppError>>,
    github: JoinHandle<Result<(), AppError>>,
    outbox: JoinHandle<Result<(), AppError>>,
    replies: JoinHandle<()>,
    job_workers: JoinHandle<anyhow::Result<()>>,
}

impl RunningServer {
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// This instance among the others.
    pub fn cluster(&self) -> &Cluster {
        &self.cluster
    }

    /// Where the app listens.
    pub fn address(&self) -> Option<SocketAddr> {
        self.address
    }

    /// Ask for a graceful stop; `wait` then returns.
    pub fn shutdown(&self) -> watch::Sender<bool> {
        self.shutdown.clone()
    }

    /// Stop gracefully: listeners close, loops finish their step.
    pub async fn stop(self) -> anyhow::Result<()> {
        let _ = self.shutdown.send(true);
        self.wait().await
    }

    /// Stop abruptly, as a crash would: every task is aborted and nothing
    /// is released or finished.
    pub fn abort(self) {
        self.http.abort();
        self.scheduler.abort();
        self.github.abort();
        self.outbox.abort();
        self.replies.abort();
        self.job_workers.abort();
    }

    /// Run until a shutdown is asked for or a supervised loop stops; a loop
    /// that stops on its own is an error.
    pub async fn wait(self) -> anyhow::Result<()> {
        let Self {
            state,
            shutdown,
            http,
            mut scheduler,
            mut github,
            mut outbox,
            replies,
            job_workers,
            ..
        } = self;
        let mut http = http;
        let unexpected =
            |name: &str| -> anyhow::Result<()> { bail!("{name} stopped unexpectedly") };
        let result = tokio::select! {
            served = &mut http => {
                let _ = shutdown.send(true);
                let outbox_result = outbox.await.context("join outbox publisher")?;
                let scheduler_result = scheduler.await.context("join Agent Run scheduler")?;
                let github_result = github.await.context("join GitHub job worker")?;
                served.context("join HTTP application")?.context("serve HTTP application")?;
                outbox_result.context("run outbox publisher")?;
                scheduler_result.context("run Agent Run scheduler")?;
                github_result.context("run GitHub job worker")
            }
            outbox_result = &mut outbox => {
                let already_shutting_down = *shutdown.borrow();
                let _ = shutdown.send(true);
                http.await.context("join HTTP application")?.context("serve HTTP application")?;
                let scheduler_result = scheduler.await.context("join Agent Run scheduler")?;
                let github_result = github.await.context("join GitHub job worker")?;
                outbox_result.context("join outbox publisher")?.context("run outbox publisher")?;
                scheduler_result.context("run Agent Run scheduler")?;
                github_result.context("run GitHub job worker")?;
                if already_shutting_down { Ok(()) } else { unexpected("outbox publisher") }
            }
            scheduler_result = &mut scheduler => {
                let already_shutting_down = *shutdown.borrow();
                let _ = shutdown.send(true);
                http.await.context("join HTTP application")?.context("serve HTTP application")?;
                let outbox_result = outbox.await.context("join outbox publisher")?;
                let github_result = github.await.context("join GitHub job worker")?;
                scheduler_result
                    .context("join Agent Run scheduler")?
                    .context("run Agent Run scheduler")?;
                outbox_result.context("run outbox publisher")?;
                github_result.context("run GitHub job worker")?;
                if already_shutting_down { Ok(()) } else { unexpected("Agent Run scheduler") }
            }
            github_result = &mut github => {
                let already_shutting_down = *shutdown.borrow();
                let _ = shutdown.send(true);
                http.await.context("join HTTP application")?.context("serve HTTP application")?;
                let outbox_result = outbox.await.context("join outbox publisher")?;
                let scheduler_result = scheduler.await.context("join Agent Run scheduler")?;
                github_result
                    .context("join GitHub job worker")?
                    .context("run GitHub job worker")?;
                outbox_result.context("run outbox publisher")?;
                scheduler_result.context("run Agent Run scheduler")?;
                if already_shutting_down { Ok(()) } else { unexpected("GitHub job worker") }
            }
        };
        let _ = replies.await;
        if let Err(error) = job_workers.await {
            tracing::warn!(%error, "background job workers stopped abnormally");
        }
        state.supervisors.shutdown_all().await;
        result
    }
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

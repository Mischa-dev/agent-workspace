//! The server's assembly (K5.1): the state, the work done once at startup,
//! the listeners and the loops. `main.rs` parses the command line and runs
//! one; the cluster tests run two or more against one database, each with
//! its own state, listener and loops, and stop one gracefully or abruptly.
//!
//! Roles (K5.7) say what an instance runs: `api` serves the app's routes
//! and GitHub's webhooks; `live` holds browsers' sockets and the tail that
//! feeds them; `work` runs the jobs, the log's sequencer and consumers, the
//! reply worker, GitHub's jobs, the one-off startup work and, unless the
//! built-in host is switched off, the host (runs, recovery, repositories).
//! An instance without `api` or `live` answers health checks only. Every
//! instance beats its heartbeat and listens for wakes.
//!
//! With `AGENT_WORKSPACE_WORK_LOOP=off` a `work` instance runs its queue
//! work only when ticked (K5.18, `tick.rs`); the log's sequencer then runs
//! on the `live` instances, so live updates never wait for a tick. Only
//! `work` instances migrate and do the startup work, so an `api` starts
//! cold, with nothing but its pool.

pub mod tick;

use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use sqlx::PgPool;
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};

use crate::{
    AppState,
    cluster::{Cluster, LeaseName},
    config::AppConfig,
    error::AppError,
    github,
    health::{self, HealthState, Process},
    jobs,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
};

/// A server, assembled and ready to listen.
pub struct Server {
    state: AppState,
    cluster: Cluster,
    /// The log's head when this instance started: its tail starts there.
    tail_start: i64,
    jobs: Arc<jobs::JobRegistry>,
}

impl Server {
    /// The state, and the one-time startup work: harnesses, secret
    /// migration (under `server.startup`) and the module registries.
    pub async fn build(config: Arc<AppConfig>, pool: PgPool) -> anyhow::Result<Self> {
        let supervisors =
            SupervisorRegistry::new(&config).context("configure provider supervisors")?;
        let state = AppState {
            config: Arc::clone(&config),
            pool: pool.clone(),
            rate_limits: RateLimiters::for_config(&config),
            realtime: RealtimeHub::with_contract(256, config.live_url.is_some()),
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
        // Run recovery belongs to the built-in host's lease holder (K5.3),
        // which recovers as it takes the lease, here or after taking over.

        // This process as an instance among any number (K5.2); its
        // heartbeat starts with the loops.
        let cluster = Cluster::register(
            pool.clone(),
            config.roles,
            config.instance_label.clone(),
            config.lease_ttl,
        )
        .await
        .context("register this server instance")?;

        // One-off work at startup runs once however many instances start
        // together: under `server.startup`, which nobody else can hold
        // meanwhile. One that finds it held leaves it to that instance.
        // Only `work` instances do it, so an `api` starts cold and fast.
        let startup = if config.roles.work {
            cluster
                .try_acquire(LeaseName::StartupMaintenance)
                .await
                .context("take the startup lease")?
        } else {
            None
        };
        match startup {
            Some(lease) => {
                let moved = crate::secrets::migrate_plaintext_webhook_secrets(&pool, &config)
                    .await
                    .context("encrypt plaintext agent webhook secrets");
                if let Err(error) = cluster.release(&lease).await {
                    tracing::warn!(
                        error_category = error.log_category(),
                        "startup lease not released; it runs out instead"
                    );
                }
                let moved_secrets = moved?;
                if moved_secrets > 0 {
                    tracing::info!(
                        moved_secrets,
                        "moved agent webhook secrets into the secrets store"
                    );
                }
            }
            None if config.roles.work => {
                tracing::info!("another instance is doing the startup work");
            }
            None => {}
        }

        // Modules (and their operations) that don't fit together stop the
        // server here, before it takes a request.
        let modules = crate::modules::registry::Registry::global();
        let operations = crate::ops::registry();
        tracing::info!(
            modules = modules.descriptions().len(),
            operations = operations.entries().count(),
            "modules registered"
        );
        let tail_start = {
            let mut connection = crate::db::scoped_tx(&pool)
                .await
                .context("read the log's head")?;
            crate::events::head(&mut connection)
                .await
                .context("read the log's head")?
        };
        Ok(Self {
            state,
            cluster,
            tail_start,
            jobs: Arc::new(jobs::default_registry()),
        })
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn cluster(&self) -> &Cluster {
        &self.cluster
    }

    /// Serve on `listener` what this instance's roles serve (the app with
    /// `api`, sockets with `live`, otherwise health checks only), and
    /// GitHub webhooks on `webhooks` with `api`; and start the loops its
    /// roles run.
    pub fn spawn(self, listener: TcpListener, webhooks: Option<TcpListener>) -> RunningServer {
        let Self {
            state,
            cluster,
            tail_start,
            jobs,
        } = self;
        let roles = state.config.roles;
        let host = roles.work && state.config.builtin_host;
        // Queue work loops here, or waits for the tick.
        let looping = roles.work && state.config.work_loop;
        let sequencing = looping || roles.live;
        let ticks = roles
            .work
            .then(|| tick::router(state.clone(), Arc::clone(&jobs)));
        let address = listener.local_addr().ok();
        let (shutdown, receiver) = watch::channel(false);
        // The wake listener (K5.4) runs the sequencer, the tail and lease
        // standbys; each also polls, so it is only ever an optimisation.
        let wakes = crate::realtime::wake::Wakes {
            control: cluster.release_wake(),
            ephemeral: roles.live.then(|| state.realtime.clone()),
            ..crate::realtime::wake::Wakes::default()
        };
        let tail = crate::realtime::tail::channel();
        let progress = crate::realtime::tail::Progress::default();
        let process = Process::new(
            cluster.instance_id().0,
            roles,
            roles.live.then(|| progress.clone()),
        );
        if !roles.live {
            // No sockets here: the hub refuses them from the start.
            state.realtime.drain();
        }
        // Contract connections here hear this instance's tail from its start
        // (or, fed from outside, the publisher from its first frame).
        if !state.realtime.contract().external() {
            state.realtime.contract().begin(tail_start);
        }
        // With an outside live service, one `work` instance publishes the log
        // there (K5.19).
        let live_publisher = (roles.work && state.config.live_url.is_some()).then(|| {
            crate::realtime::contract::publish::run(
                state.clone(),
                cluster.clone(),
                Arc::clone(&wakes.events),
                receiver.clone(),
            )
        });
        // This instance's sockets hear the log through the tail (K5.5).
        let feed =
            crate::realtime::tail::feed(state.realtime.clone(), tail.subscribe(), receiver.clone());
        // Every instance serves harness manifests (launches, catalogues,
        // accounts), so each reloads them: one registered or switched off
        // through another instance shows here too.
        crate::harnesses::spawn_reloader(state.clone());
        // Every instance knows its cluster, host or not, so a run started
        // through this one's API wakes the lease holder instead of being
        // admitted here without the lease.
        state.supervisors.set_cluster(cluster.clone());
        // The run scheduler, recovery and host jobs, on whichever instance
        // holds the built-in host's lease (K5.3).
        let scheduler = tokio::spawn({
            let state = state.clone();
            let cluster = cluster.clone();
            let receiver = receiver.clone();
            async move {
                if host {
                    crate::scheduler::run_host(state, cluster, receiver).await
                } else {
                    wait_for_shutdown(receiver).await;
                    Ok(())
                }
            }
        });
        let github = tokio::spawn({
            let state = state.clone();
            let receiver = receiver.clone();
            async move {
                if looping {
                    github::run_jobs(state, receiver).await
                } else {
                    wait_for_shutdown(receiver).await;
                    Ok(())
                }
            }
        });
        // Not supervised like the loops above: a failure here delays
        // wakeups and lease expiry but never loses data, so it must not
        // stop the server.
        // The log's sequencer, the instance's heartbeat, the wake listener
        // and the tail run beside it on the same task, for the same reason:
        // a failed step is retried on the next round (a heartbeat that
        // stays late fences this instance, it never stops it).
        let replies = tokio::spawn({
            let replies = crate::replies::run(state.clone(), receiver.clone());
            let sequencer = crate::events::sequencer::run(
                state.pool.clone(),
                Arc::clone(&wakes.log),
                receiver.clone(),
            );
            let tail = crate::realtime::tail::run(
                state.pool.clone(),
                tail_start,
                Arc::clone(&wakes.events),
                tail.clone(),
                progress,
                receiver.clone(),
            );
            let listener = crate::realtime::wake::run(
                state
                    .config
                    .database_listen_url
                    .as_ref()
                    .map(|url| url.expose().to_owned()),
                format!("aw-listen-{}", cluster.instance_id().0.simple()),
                wakes,
                receiver.clone(),
            );
            // Webhooks and automations read the log (K3.4), on whichever
            // instance holds the router's lease.
            let router = crate::events::run_consumer(
                cluster.clone(),
                crate::events::Router,
                receiver.clone(),
            );
            // Notifications read the log too (K3.9).
            let notifier = crate::events::run_consumer(
                cluster.clone(),
                crate::notifications::Notifier,
                receiver.clone(),
            );
            let heartbeat_cluster = cluster.clone();
            let heartbeat_shutdown = receiver.clone();
            async move {
                let heartbeat = heartbeat_cluster.run_heartbeat(heartbeat_shutdown);
                tokio::join!(
                    only_if(looping, replies),
                    only_if(sequencing, sequencer),
                    only_if(roles.live, tail),
                    only_if(roles.live, feed),
                    listener,
                    only_if(looping, router),
                    only_if(looping, notifier),
                    async move {
                        if let Some(publisher) = live_publisher {
                            publisher.await;
                        }
                    },
                    heartbeat
                );
            }
        });
        // Background jobs (F9). A job lost to a crash is retried once its
        // lease runs out, so like the reply worker this doesn't stop the
        // server.
        let job_workers = tokio::spawn({
            let state = state.clone();
            let receiver = receiver.clone();
            async move {
                if looping {
                    let options = jobs::WorkerOptions {
                        concurrency: state.config.job_concurrency,
                        ..jobs::WorkerOptions::default()
                    };
                    jobs::run_workers(state, jobs, options, receiver).await
                } else {
                    wait_for_shutdown(receiver).await;
                    Ok(())
                }
            }
        });
        let application_shutdown = receiver.clone();
        let webhook_shutdown = receiver;
        let http_state = state.clone();
        let http_process = process.clone();
        let http = tokio::spawn(async move {
            let application = if roles.api {
                router(http_state.clone())
            } else if roles.live {
                live_router(http_state.clone())
            } else {
                health_router(http_state.clone())
            };
            let application = match ticks {
                Some(ticks) => application.merge(ticks),
                None => application,
            }
            .layer(axum::Extension(http_process));
            let application = axum::serve(listener, application.into_make_service())
                .with_graceful_shutdown(wait_for_shutdown(application_shutdown));
            let webhooks = async move {
                match webhooks.filter(|_| roles.api) {
                    Some(webhooks) => {
                        axum::serve(
                            webhooks,
                            github::webhook_router(http_state).into_make_service(),
                        )
                        .with_graceful_shutdown(wait_for_shutdown(webhook_shutdown))
                        .await
                    }
                    None => Ok(()),
                }
            };
            tokio::try_join!(application, webhooks).map(|_| ())
        });
        RunningServer {
            drainer: Drainer {
                process,
                cluster: cluster.clone(),
                hub: state.realtime.clone(),
                drain: state.config.drain,
            },
            state,
            cluster,
            tail,
            address,
            shutdown,
            http,
            scheduler,
            github,
            replies,
            job_workers,
        }
    }
}

/// An instance without `api`: browsers' sockets and health checks.
fn live_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(health::router(HealthState {
            pool: state.pool.clone(),
        }))
        .route(
            "/api/v1/realtime",
            axum::routing::get(crate::realtime::websocket),
        )
        .merge(crate::realtime::contract::live::router())
        .with_state(state)
        .layer(axum::middleware::from_fn(crate::tenancy::scope_requests))
}

/// An instance with neither `api` nor `live`: health checks only.
fn health_router(state: AppState) -> axum::Router {
    health::router(HealthState { pool: state.pool })
        .layer(axum::middleware::from_fn(crate::tenancy::scope_requests))
}

async fn only_if(on: bool, work: impl Future<Output = ()>) {
    if on {
        work.await;
    }
}

/// The first steps of a graceful stop (K5.7): say "not ready" at once, give
/// the balancer the drain time to move off, then close every socket with
/// 1012 so its browser reconnects to another instance. Stopping the
/// listeners and loops, and releasing the leases, follows.
#[derive(Clone)]
pub struct Drainer {
    process: Process,
    cluster: Cluster,
    hub: crate::realtime::RealtimeHub,
    drain: Duration,
}

impl Drainer {
    pub async fn drain(&self) {
        self.process.drain();
        if let Err(error) = self.cluster.mark_draining().await {
            tracing::warn!(
                error_category = error.log_category(),
                "instance not marked draining"
            );
        }
        tokio::time::sleep(self.drain).await;
        self.hub.drain();
    }
}

/// A server that is listening and running its loops.
pub struct RunningServer {
    drainer: Drainer,
    state: AppState,
    cluster: Cluster,
    tail: crate::realtime::tail::TailSender,
    address: Option<SocketAddr>,
    shutdown: watch::Sender<bool>,
    http: JoinHandle<std::io::Result<()>>,
    scheduler: JoinHandle<Result<(), AppError>>,
    github: JoinHandle<Result<(), AppError>>,
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

    /// Every event this instance's tail hands on, from now.
    pub fn subscribe_tail(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::realtime::tail::Tailed> {
        self.tail.subscribe()
    }

    /// Where the app listens.
    pub fn address(&self) -> Option<SocketAddr> {
        self.address
    }

    /// Ask for a graceful stop; `wait` then returns. Drain first.
    pub fn shutdown(&self) -> watch::Sender<bool> {
        self.shutdown.clone()
    }

    /// What drains this instance before it stops.
    pub fn drainer(&self) -> Drainer {
        self.drainer.clone()
    }

    /// Stop gracefully: drain, then listeners close and loops finish their
    /// step.
    pub async fn stop(self) -> anyhow::Result<()> {
        self.drainer.drain().await;
        let _ = self.shutdown.send(true);
        self.wait().await
    }

    /// Stop abruptly, as a crash would: every task is aborted and nothing
    /// is released or finished.
    pub fn abort(self) {
        self.http.abort();
        self.scheduler.abort();
        self.github.abort();
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
                let scheduler_result = scheduler.await.context("join Agent Run scheduler")?;
                let github_result = github.await.context("join GitHub job worker")?;
                served.context("join HTTP application")?.context("serve HTTP application")?;
                scheduler_result.context("run Agent Run scheduler")?;
                github_result.context("run GitHub job worker")
            }
            scheduler_result = &mut scheduler => {
                let already_shutting_down = *shutdown.borrow();
                let _ = shutdown.send(true);
                http.await.context("join HTTP application")?.context("serve HTTP application")?;
                let github_result = github.await.context("join GitHub job worker")?;
                scheduler_result
                    .context("join Agent Run scheduler")?
                    .context("run Agent Run scheduler")?;
                github_result.context("run GitHub job worker")?;
                if already_shutting_down { Ok(()) } else { unexpected("Agent Run scheduler") }
            }
            github_result = &mut github => {
                let already_shutting_down = *shutdown.borrow();
                let _ = shutdown.send(true);
                http.await.context("join HTTP application")?.context("serve HTTP application")?;
                let scheduler_result = scheduler.await.context("join Agent Run scheduler")?;
                github_result
                    .context("join GitHub job worker")?
                    .context("run GitHub job worker")?;
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

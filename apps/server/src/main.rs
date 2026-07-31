#![forbid(unsafe_code)]

use std::sync::Arc;

use agent_workspace_server::{
    AppState, bootstrap, config::AppConfig, migration_preflight, rate_limit::RateLimiters,
    realtime::RealtimeHub, recovery, router, seed, shutdown_signal, telemetry::init_tracing,
};
use anyhow::{Context, bail};
use sqlx::postgres::PgPoolOptions;
use tokio::{net::TcpListener, sync::watch};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Arc::new(AppConfig::load().context("load application configuration")?);
    init_tracing(&config.log_filter)?;

    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(config.database_url.expose())
        .await
        .context("connect to PostgreSQL")?;

    let migration_preflight = migration_preflight::prepare_m3_upgrade(&pool, &config)
        .await
        .context("prepare legacy data for M3 migration")?;
    if migration_preflight.required {
        tracing::warn!(
            profile_paths_repaired = migration_preflight.profile_paths_repaired,
            repository_paths_repaired = migration_preflight.repository_paths_repaired,
            workers_disabled = migration_preflight.workers_disabled,
            "prepared legacy execution data for the M3 migration"
        );
    }

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("apply database migrations")?;

    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("bootstrap-admin") => {
            let command = bootstrap::parse_command(arguments)
                .context("parse host-local bootstrap command")?;
            let password =
                bootstrap::read_password_interactively().context("read administrator password")?;
            let bootstrapped = bootstrap::bootstrap_admin(&pool, command, password)
                .await
                .context("bootstrap administrator")?;
            println!("{}", serde_json::to_string_pretty(&bootstrapped)?);
            return Ok(());
        }
        Some("bootstrap-member") => {
            if !config.environment.is_development() || !config.server_addr.ip().is_loopback() {
                bail!("member bootstrap is available only in development on a loopback listener");
            }
            let command = bootstrap::parse_member_command(arguments)
                .context("parse development member bootstrap command")?;
            let password =
                bootstrap::read_password_interactively().context("read member password")?;
            let bootstrapped = bootstrap::bootstrap_development_member(&pool, command, password)
                .await
                .context("bootstrap development member")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "userId": bootstrapped.user_id,
                    "actorId": bootstrapped.actor_id,
                    "organizationId": bootstrapped.organization_id,
                }))?
            );
            return Ok(());
        }
        Some("dev-seed") => {
            let seeded = seed::dev_seed(&pool, &config)
                .await
                .context("seed development data")?;
            println!("{}", serde_json::to_string_pretty(&seeded)?);
            return Ok(());
        }
        Some(_) => bail!("unknown command"),
        None => {}
    }

    let realtime = RealtimeHub::new(256);
    let state = AppState {
        config: Arc::clone(&config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: realtime.clone(),
    };

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

    let listener = TcpListener::bind(config.server_addr)
        .await
        .with_context(|| format!("bind application server to {}", config.server_addr))?;

    tracing::info!(
        address = %config.server_addr,
        environment = %config.environment,
        "Agent Workspace server listening"
    );

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let mut outbox_publisher =
        tokio::spawn(agent_workspace_server::realtime::run_outbox_publisher(
            pool,
            realtime,
            agent_workspace_server::realtime::outbox_worker_id(),
            shutdown_receiver.clone(),
        ));
    let signal_shutdown_sender = shutdown_sender.clone();
    let coordinated_shutdown = async move {
        tokio::select! {
            () = shutdown_signal() => {
                let _ = signal_shutdown_sender.send(true);
            }
            () = wait_for_shutdown(shutdown_receiver) => {}
        }
    };
    let server = async move {
        axum::serve(listener, router(state).into_make_service())
            .with_graceful_shutdown(coordinated_shutdown)
            .await
    };
    tokio::pin!(server);

    tokio::select! {
        server_result = &mut server => {
            let _ = shutdown_sender.send(true);
            let worker_result = outbox_publisher
                .await
                .context("join outbox publisher")?;
            server_result.context("serve HTTP application")?;
            worker_result.context("run outbox publisher")
        }
        worker_result = &mut outbox_publisher => {
            let already_shutting_down = *shutdown_sender.borrow();
            let _ = shutdown_sender.send(true);
            server.await.context("serve HTTP application")?;
            worker_result
                .context("join outbox publisher")?
                .context("run outbox publisher")?;
            if already_shutting_down {
                Ok(())
            } else {
                bail!("outbox publisher stopped unexpectedly")
            }
        }
    }
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

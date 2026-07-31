#![forbid(unsafe_code)]

use std::sync::Arc;

use agent_workspace_server::{
    AppState, bootstrap, config::AppConfig, rate_limit::RateLimiters, realtime::RealtimeHub,
    recovery, router, seed, shutdown_signal, telemetry::init_tracing,
};
use anyhow::{Context, bail};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Arc::new(AppConfig::load().context("load application configuration")?);
    init_tracing(&config.log_filter)?;

    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(config.database_url.expose())
        .await
        .context("connect to PostgreSQL")?;

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

    let state = AppState {
        config: Arc::clone(&config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(256),
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

    axum::serve(listener, router(state).into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serve HTTP application")
}

#![forbid(unsafe_code)]

use std::sync::Arc;

use agent_workspace_server::{
    AppState, config::AppConfig, router, shutdown_signal, telemetry::init_tracing,
};
use anyhow::Context;
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

    let listener = TcpListener::bind(config.server_addr)
        .await
        .with_context(|| format!("bind application server to {}", config.server_addr))?;

    tracing::info!(
        address = %config.server_addr,
        environment = %config.environment,
        "Agent Workspace server listening"
    );

    axum::serve(
        listener,
        router(AppState { config, pool }).into_make_service(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("serve HTTP application")
}

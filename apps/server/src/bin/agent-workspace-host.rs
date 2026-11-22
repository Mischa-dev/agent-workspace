//! `agent-workspace-host`: the built-in host as its own program (A1.17).
//! See `hosts::builtin::program` for what it runs and the settings it reads.
//!
//! It lives in the server's package for now because the built-in host's
//! store is still the server's code; it moves into `crates/builtin-host`
//! with that store.

#![forbid(unsafe_code)]

use agent_workspace_server::{
    config::AppConfig,
    hosts::builtin::program::{self, ProgramSettings},
    shutdown_signal,
    telemetry::init_tracing,
};
use anyhow::Context;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load().context("load application configuration")?;
    init_tracing(&config.log_filter)?;
    let settings = ProgramSettings::from_env(&config)?;
    let token = program::token(&config, &settings).await?;
    // Health checks on their own address when one is set, else the
    // application address (the server it serves is elsewhere).
    let address = config.health_addr.unwrap_or(config.server_addr);
    let config = program::host_config(config, settings.server_url, token);
    let pool = agent_workspace_server::db::connect(&config)
        .await
        .context("connect to PostgreSQL")?;
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("bind health checks to {address}"))?;
    tracing::info!(%address, "the built-in host is running");
    program::run(config, pool, listener, shutdown_signal()).await
}

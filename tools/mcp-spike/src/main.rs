use std::env;

use mcp_spike::{FakeStore, loopback_address, router};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = env::var("AGENT_WORKSPACE_MCP_BEARER_TOKEN")
        .map_err(|_| "AGENT_WORKSPACE_MCP_BEARER_TOKEN is required")?;
    let port = env::var("AGENT_WORKSPACE_MCP_PORT")
        .ok()
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(3100);
    let cancellation = CancellationToken::new();
    let app = router(FakeStore::seeded().await, &token, cancellation.clone())?;
    let listener = tokio::net::TcpListener::bind(loopback_address(port)).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancellation.cancel();
        })
        .await?;
    Ok(())
}

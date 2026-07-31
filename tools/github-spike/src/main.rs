use std::{env, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use github_spike::{VerifiedWebhook, WEBHOOK_PORT, WebhookVerifier};

#[derive(Clone)]
struct AppState {
    verifier: Arc<WebhookVerifier>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secret = env::var("AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET")
        .map_err(|_| "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET is required")?;
    let repository = env::var("AGENT_WORKSPACE_GITHUB_REPOSITORY")
        .unwrap_or_else(|_| "Mischa-dev/agent-workspace-m0-spike".to_owned());
    let verifier = Arc::new(WebhookVerifier::new(
        secret.into_bytes(),
        repository,
        10_000,
    )?);
    let app = Router::new()
        .route("/github/webhooks", post(handle_webhook))
        .with_state(AppState { verifier });
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], WEBHOOK_PORT))).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, String), StatusCode> {
    let VerifiedWebhook {
        delivery_id,
        event,
        duplicate,
    } = state
        .verifier
        .verify(&headers, &body)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    println!(
        "{}",
        serde_json::json!({
            "delivery_id": delivery_id,
            "event": event,
            "duplicate": duplicate,
        })
    );
    let status = if duplicate {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((status, delivery_id))
}

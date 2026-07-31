use std::{env, error::Error, fs};

use github_spike::GitHubAppClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app_id = required_u64("AGENT_WORKSPACE_GITHUB_APP_ID")?;
    let private_key_path = env::var("AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH")
        .map_err(|_| "AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH is required")?;
    let webhook_url = env::var("AGENT_WORKSPACE_GITHUB_WEBHOOK_URL")
        .map_err(|_| "AGENT_WORKSPACE_GITHUB_WEBHOOK_URL is required")?;
    let webhook_secret_path = env::var("AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH")
        .map_err(|_| "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH is required")?;
    let private_key = fs::read(private_key_path)?;
    let webhook_secret = fs::read_to_string(webhook_secret_path)?;
    let client = GitHubAppClient::new_app(app_id, private_key)?;
    let config = client
        .update_webhook_config(&webhook_url, webhook_secret.trim())
        .await?;
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

fn required_u64(name: &'static str) -> Result<u64, Box<dyn Error>> {
    let value = env::var(name).map_err(|_| format!("{name} is required"))?;
    let value = value.parse::<u64>()?;
    if value == 0 {
        return Err(format!("{name} must be non-zero").into());
    }
    Ok(value)
}

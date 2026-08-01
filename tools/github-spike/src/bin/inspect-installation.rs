use std::{env, error::Error, fs};

use github_spike::GitHubAppClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app_id = required_u64("AGENT_WORKSPACE_GITHUB_APP_ID")?;
    let installation_id = required_u64("AGENT_WORKSPACE_GITHUB_INSTALLATION_ID")?;
    let private_key_path = env::var("AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH")
        .map_err(|_| "AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH is required")?;
    let private_key = fs::read(private_key_path)?;
    let scope = GitHubAppClient::new(app_id, installation_id, private_key)?
        .installation_scope()
        .await?;
    println!("{}", serde_json::to_string_pretty(&scope)?);
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

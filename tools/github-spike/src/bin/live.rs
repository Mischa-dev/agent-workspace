use std::{
    env,
    error::Error,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
};

use github_spike::{CredentialedGit, GitHubAppClient, unique_test_branch};
use serde_json::json;
use tempfile::TempDir;

const OWNER: &str = "Mischa-dev";
const REPOSITORY: &str = "agent-workspace-m0-spike";
const BASE_BRANCH: &str = "main";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app_id = required_u64("AGENT_WORKSPACE_GITHUB_APP_ID")?;
    let installation_id = required_u64("AGENT_WORKSPACE_GITHUB_INSTALLATION_ID")?;
    let private_key_path = env::var("AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH")
        .map_err(|_| "AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH is required")?;
    let private_key = fs::read(private_key_path)?;
    let client = GitHubAppClient::new(app_id, installation_id, private_key)?;
    let git = CredentialedGit::new(Path::new("/usr/bin/git").to_path_buf())?;
    let checkout = TempDir::new()?;
    let repository_path = checkout.path().join("repository");
    let branch = unique_test_branch();

    {
        let token = client.installation_token(REPOSITORY).await?;
        git.clone_repository(OWNER, REPOSITORY, &repository_path, &token)?;
    }

    run_git(
        &repository_path,
        &["checkout", "-b", &branch],
        "create the test branch",
    )?;
    let known_file = repository_path.join("KNOWN_FILE.txt");
    let mut file = OpenOptions::new().append(true).open(&known_file)?;
    writeln!(file, "M0 GitHub App live delivery proof.")?;
    run_git(
        &repository_path,
        &["add", "KNOWN_FILE.txt"],
        "stage the known file",
    )?;
    run_git(
        &repository_path,
        &[
            "-c",
            "user.name=Agent Workspace M0",
            "-c",
            "user.email=agent-workspace-m0@users.noreply.github.com",
            "commit",
            "-m",
            "test: prove GitHub App delivery",
        ],
        "commit the known file",
    )?;

    {
        let token = client.installation_token(REPOSITORY).await?;
        git.push_branch(&repository_path, &branch, &token)?;
    }

    let pull_request = {
        let token = client.installation_token(REPOSITORY).await?;
        client
            .create_pull_request(
                OWNER,
                REPOSITORY,
                "M0 GitHub App live delivery proof",
                &branch,
                BASE_BRANCH,
                &token,
            )
            .await?
    };

    let checks = {
        let token = client.installation_token(REPOSITORY).await?;
        client
            .check_runs(OWNER, REPOSITORY, &branch, &token)
            .await?
    };

    {
        let token = client.installation_token(REPOSITORY).await?;
        client
            .close_pull_request(OWNER, REPOSITORY, pull_request.number, &token)
            .await?;
    }
    {
        let token = client.installation_token(REPOSITORY).await?;
        client
            .delete_branch(OWNER, REPOSITORY, &branch, &token)
            .await?;
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "app_id": app_id,
            "installation_id": installation_id,
            "repository": format!("{OWNER}/{REPOSITORY}"),
            "branch": branch,
            "pull_request": {
                "number": pull_request.number,
                "url": pull_request.html_url,
                "closed": true,
            },
            "checks": checks,
            "branch_deleted": true,
            "credentials_redacted": true,
        }))?
    );
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

fn run_git(
    repository: &Path,
    arguments: &[&str],
    operation: &'static str,
) -> Result<(), Box<dyn Error>> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(repository)
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .args(arguments)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "failed to {operation}: {}",
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

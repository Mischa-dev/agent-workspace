//! A1.17: the built-in host as its own program, `agent-workspace-host`.
//!
//! It runs what the embedded host runs (recovery, the run queue, the
//! harness turns, repository preparation and the account jobs) under the
//! same `builtin-host` lease, against the same database, and speaks the
//! host protocol to the server over HTTP with the install's token
//! ([`super::install`]). It serves health checks only, and does no other
//! work: no migrations, no queue workers, no log consumers. The server it
//! serves runs with `AGENT_WORKSPACE_BUILTIN_HOST=off`, so restarting the
//! server leaves running agents alone, and restarting the host recovers its
//! runs as the embedded host does.
//!
//! Settings beyond the server's own (`AppConfig`):
//!
//! - `AGENT_WORKSPACE_HOST_SERVER_URL`: the server's address, such as
//!   `http://server:3100` (required).
//! - `AGENT_WORKSPACE_HOST_CODE_PATH`: the one-time code the server wrote
//!   (`agent-workspace-server enrol-builtin-host --code-file <path>`), read
//!   on the first start; default `<data root>/builtin-host/code`. (Not
//!   `..._FILE`: that suffix reads a setting's value from a file, and this
//!   file is gone once used.)
//! - The token it is exchanged for is kept in
//!   `<data root>/builtin-host/token` (mode 0600) and used from then on.

use std::{path::Path, sync::Arc};

use anyhow::{Context, bail};
use tokio::net::TcpListener;

use crate::{
    config::{AppConfig, BuiltinHostRemote, SecretString},
    server::Server,
};

/// The program's own settings, read beside the server's.
pub struct ProgramSettings {
    pub server_url: String,
    pub code_file: std::path::PathBuf,
}

impl ProgramSettings {
    pub fn from_env(config: &AppConfig) -> anyhow::Result<Self> {
        let server_url = std::env::var("AGENT_WORKSPACE_HOST_SERVER_URL")
            .context("set AGENT_WORKSPACE_HOST_SERVER_URL to the server's address")?;
        let parsed = url::Url::parse(server_url.trim())
            .context("AGENT_WORKSPACE_HOST_SERVER_URL is not an address")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("AGENT_WORKSPACE_HOST_SERVER_URL must be http or https");
        }
        let code_file = super::install::code_path(config);
        Ok(Self {
            server_url: server_url.trim().trim_end_matches('/').to_owned(),
            code_file,
        })
    }
}

/// The program's own folder under the data root.
fn folder(config: &AppConfig) -> std::path::PathBuf {
    config.data_root.join("builtin-host")
}

/// The install's token: kept from an earlier start, or exchanged for the
/// one-time code now (and the code file removed).
pub async fn token(config: &AppConfig, settings: &ProgramSettings) -> anyhow::Result<String> {
    let kept = folder(config).join("token");
    if let Ok(token) = std::fs::read_to_string(&kept) {
        let token = token.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let code = std::fs::read_to_string(&settings.code_file).with_context(|| {
        format!(
            "no token yet and no code at {}: run `agent-workspace-server enrol-builtin-host \
             --code-file <path>` on the server",
            settings.code_file.display()
        )
    })?;
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/host/builtin/enrol", settings.server_url))
        .header(
            agent_workspace_host_protocol::PROTOCOL_HEADER,
            agent_workspace_host_protocol::PROTOCOL_VERSION,
        )
        .json(&serde_json::json!({ "code": code.trim() }))
        .send()
        .await
        .context("reach the server to enrol")?;
    if !response.status().is_success() {
        bail!(
            "the server refused the code ({}): write a new one with enrol-builtin-host",
            response.status()
        );
    }
    let answer = response
        .json::<serde_json::Value>()
        .await
        .context("read the enrolment's answer")?;
    let token = answer["token"]
        .as_str()
        .context("the enrolment's answer has no token")?
        .to_owned();
    write_private(&kept, &token)?;
    let _ = std::fs::remove_file(&settings.code_file);
    Ok(token)
}

/// Writes `value` to `path`, readable by its owner only.
pub fn write_private(path: &Path, value: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("make {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("write {}", path.display()))?;
    file.write_all(value.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// The configuration the program runs with: a `work` instance that holds
/// the built-in host and nothing else, calling `server_url` over HTTP.
pub fn host_config(mut config: AppConfig, server_url: String, token: String) -> AppConfig {
    config.roles = crate::cluster::Roles {
        api: false,
        live: false,
        work: true,
    };
    config.builtin_host = true;
    // The queue's work, the log and its consumers stay with the server.
    config.work_loop = false;
    config.work_tick_secret_path = None;
    config.builtin_host_remote = Some(BuiltinHostRemote {
        server_url,
        token: SecretString::new(token),
    });
    config
}

/// Run the program until `stop` resolves: health checks on `listener`,
/// the built-in host behind its lease.
pub async fn run(
    config: AppConfig,
    pool: sqlx::PgPool,
    listener: TcpListener,
    stop: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let remote = config
        .builtin_host_remote
        .as_ref()
        .context("the host program runs with the server's address and its token")?;
    // Made once here, so the client made for each call can't fail later.
    if let Err(error) =
        agent_workspace_host_protocol::HostClient::new(&remote.server_url, remote.token.expose())
    {
        bail!("make the protocol client: {error}");
    }
    let server = Server::build(Arc::new(config), pool).await?;
    let running = server.spawn(listener, None);
    let shutdown = running.shutdown();
    let supervisors = running.state().supervisors.clone();
    let wait = running.wait();
    tokio::pin!(wait);
    tokio::select! {
        result = &mut wait => result,
        () = stop => {
            supervisors.request_shutdown();
            let _ = shutdown.send(true);
            wait.await
        }
    }
}

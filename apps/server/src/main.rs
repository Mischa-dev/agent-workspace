#![forbid(unsafe_code)]

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    bootstrap, config::AppConfig, mcp, migration_preflight, retention, seed, server::Server,
    shutdown_signal, telemetry::init_tracing, work_items,
};
use anyhow::{Context, bail};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Runs without the service configuration: the restore drill pipes secret
    // rows from a spare database in and gets counts back.
    if std::env::args().nth(1).as_deref() == Some("secrets-verify-export") {
        return verify_secret_export();
    }
    let config = Arc::new(AppConfig::load().context("load application configuration")?);
    init_tracing(&config.log_filter)?;

    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(config.database_url.expose())
        .await
        .context("connect to PostgreSQL")?;

    // Before anything that changes the database: the release script runs this
    // with a new build to see whether it can take over.
    if std::env::args().nth(1).as_deref() == Some("preflight") {
        let report = agent_workspace_server::preflight::run(&config, &pool).await?;
        report.print();
        if report.failed() {
            bail!("preflight found problems; the release was not switched");
        }
        return Ok(());
    }

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
    let repository_preflight = migration_preflight::prepare_m3_repository_upgrade(&pool, &config)
        .await
        .context("prepare canonical M3 repository mirrors")?;
    if repository_preflight.required {
        tracing::warn!(
            mirrors_adopted = repository_preflight.mirrors_adopted,
            "prepared retained repository mirrors for the canonical M3 layout"
        );
    }

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("apply database migrations")?;
    let provisioned_workflows = work_items::provision_missing_default_workflows(&pool)
        .await
        .context("provision default Work Item workflows")?;
    if provisioned_workflows > 0 {
        tracing::info!(
            provisioned_workflows,
            "provisioned default Work Item workflows"
        );
    }

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
        Some("user-password-rotate") => {
            let command = bootstrap::parse_password_rotation_command(arguments)
                .context("parse host-local password rotation command")?;
            let password =
                bootstrap::read_password_interactively().context("read replacement password")?;
            let rotated = bootstrap::rotate_password(&pool, command, password)
                .await
                .context("rotate User password and sessions")?;
            println!("{}", serde_json::to_string_pretty(&rotated)?);
            return Ok(());
        }
        Some("secret-key-init") => {
            // Creates a new keyring file; refuses to replace one.
            let options = parse_options(arguments)?;
            reject_unknown_options(&options, &["path"])?;
            let path = std::path::PathBuf::from(required_option(&options, "path")?);
            // Checked before the key exists: a later config check can't take
            // back a key already written somewhere it can be served or committed.
            if let Err(reason) =
                agent_workspace_server::config::validate_new_secret_location(&path, &config)
            {
                bail!("--path {}: {reason}", path.display());
            }
            let keyring = agent_workspace_server::secrets::FileKeyring::generate();
            agent_workspace_server::secrets::write_new_keyring(&path, &keyring)
                .with_context(|| format!("create {}", path.display()))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "path": path,
                    "activeKeyId": keyring.key_ids().first(),
                    "next": "Set AGENT_WORKSPACE_SECRETS_KEY_PATH to this path, and back the file up apart from database dumps.",
                }))?
            );
            return Ok(());
        }
        Some("secret-key-rotate") => {
            let options = parse_options(arguments)?;
            reject_unknown_options(&options, &[])?;
            let result = agent_workspace_server::secrets::rotate_master_key(&pool, &config)
                .await
                .context("rotate the secrets master key")?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        Some("retention-worktrees") => {
            let options = parse_options(arguments)?;
            reject_unknown_options(&options, &["apply"])?;
            let result = retention::clean_worktrees(
                &pool,
                &config.data_root,
                optional_bool(&options, "apply", false)?,
            )
            .await
            .context("clean eligible retained worktrees")?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        Some("retention-external-events") => {
            let options = parse_options(arguments)?;
            reject_unknown_options(&options, &["apply"])?;
            let result =
                retention::clean_external_events(&pool, optional_bool(&options, "apply", false)?)
                    .await
                    .context("clean expired external-agent events")?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        Some("mcp-credential-create") => {
            let options = parse_options(arguments)?;
            reject_unknown_options(
                &options,
                &[
                    "project-id",
                    "actor-id",
                    "client-name",
                    "scopes",
                    "expires-days",
                ],
            )?;
            let issued = mcp::issue_credential(
                &pool,
                mcp::IssueCredentialCommand {
                    project_id: required_uuid(&options, "project-id")?,
                    actor_id: required_uuid(&options, "actor-id")?,
                    client_name: required_option(&options, "client-name")?.to_owned(),
                    scopes: parse_scopes(required_option(&options, "scopes")?)?,
                    expires_in_days: optional_u16(&options, "expires-days", 30)?,
                },
            )
            .await
            .context("issue MCP credential")?;
            println!("{}", serde_json::to_string_pretty(&issued)?);
            return Ok(());
        }
        Some("direct-agent-create") => {
            // Creates an agent in the owner's Messages space and writes its
            // token to a new owner-only file, so the token is never printed.
            let options = parse_options(arguments)?;
            reject_unknown_options(
                &options,
                &[
                    "owner-login",
                    "name",
                    "handle",
                    "token-file",
                    "webhook-url",
                    "wakeup-format",
                ],
            )?;
            let token_file = std::path::PathBuf::from(required_option(&options, "token-file")?);
            let (project_id, owner_actor_id) = agent_workspace_server::direct::space_for_login(
                &pool,
                required_option(&options, "owner-login")?,
            )
            .await
            .context("find the owner's Messages space")?;
            let agent = agent_workspace_server::external_agents::provision(
                &pool,
                project_id,
                owner_actor_id,
                required_option(&options, "name")?,
                required_option(&options, "handle")?,
            )
            .await
            .context("create agent")?;
            write_private_file(&token_file, &agent.token)?;
            // With --webhook-url, the signing secret goes to "<token-file>.webhook".
            // Wakeups are typed (docs/agent-runner.md) unless
            // --wakeup-format inbox asks for the old inbox.available body.
            if let Some(url) = options.get("webhook-url") {
                let typed = match options.get("wakeup-format").map(String::as_str) {
                    None | Some("typed") => true,
                    Some("inbox") => false,
                    Some(_) => anyhow::bail!("--wakeup-format is typed or inbox"),
                };
                let secret = agent_workspace_server::external_agents::set_webhook_for_host(
                    &pool,
                    &config,
                    agent.connection_id,
                    url,
                    &config.webhook_trusted_origins,
                    typed,
                )
                .await
                .context("set wakeup webhook")?;
                let mut secret_file = token_file.clone().into_os_string();
                secret_file.push(".webhook");
                write_private_file(std::path::Path::new(&secret_file), &secret)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "projectId": project_id,
                    "actorId": agent.actor_id,
                    "connectionId": agent.connection_id,
                    "tokenFile": token_file,
                    "expiresAt": agent.expires_at,
                }))?
            );
            return Ok(());
        }
        Some("mcp-credential-list") => {
            let options = parse_options(arguments)?;
            reject_unknown_options(&options, &["project-id"])?;
            let credentials = mcp::list_credentials(&pool, required_uuid(&options, "project-id")?)
                .await
                .context("list MCP credentials")?;
            println!("{}", serde_json::to_string_pretty(&credentials)?);
            return Ok(());
        }
        Some("mcp-credential-revoke") => {
            let options = parse_options(arguments)?;
            reject_unknown_options(&options, &["credential-id", "expected-version"])?;
            let credential = mcp::revoke_credential(
                &pool,
                required_uuid(&options, "credential-id")?,
                required_i64(&options, "expected-version")?,
            )
            .await
            .context("revoke MCP credential")?;
            println!("{}", serde_json::to_string_pretty(&credential)?);
            return Ok(());
        }
        Some("mcp-credential-replace") => {
            let options = parse_options(arguments)?;
            reject_unknown_options(
                &options,
                &["credential-id", "expected-version", "expires-days"],
            )?;
            let issued = mcp::replace_credential(
                &pool,
                required_uuid(&options, "credential-id")?,
                required_i64(&options, "expected-version")?,
                optional_u16(&options, "expires-days", 30)?,
            )
            .await
            .context("replace MCP credential")?;
            println!("{}", serde_json::to_string_pretty(&issued)?);
            return Ok(());
        }
        Some(_) => bail!("unknown command"),
        None => {}
    }

    let server = Server::build(Arc::clone(&config), pool).await?;
    let listener = TcpListener::bind(config.server_addr)
        .await
        .with_context(|| format!("bind application server to {}", config.server_addr))?;
    let webhook_listener = TcpListener::bind(config.webhook_addr)
        .await
        .with_context(|| format!("bind GitHub webhook server to {}", config.webhook_addr))?;

    tracing::info!(
        address = %config.server_addr,
        environment = %config.environment,
        "Agent Workspace server listening"
    );
    tracing::info!(
        address = %config.webhook_addr,
        github_configured = config.github.is_some(),
        "GitHub webhook server listening"
    );

    let running = server.spawn(listener, webhook_listener);
    let shutdown = running.shutdown();
    let supervisors = running.state().supervisors.clone();
    let wait = running.wait();
    tokio::pin!(wait);
    tokio::select! {
        result = &mut wait => result,
        () = shutdown_signal() => {
            supervisors.request_shutdown();
            let _ = shutdown.send(true);
            wait.await
        }
    }
}

fn parse_options(
    arguments: impl Iterator<Item = String>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut options = BTreeMap::new();
    let mut arguments = arguments;
    while let Some(flag) = arguments.next() {
        let Some(name) = flag.strip_prefix("--") else {
            bail!("expected --name value arguments");
        };
        if name.is_empty() || options.contains_key(name) {
            bail!("invalid or duplicate option --{name}");
        }
        let value = arguments
            .next()
            .with_context(|| format!("missing value for --{name}"))?;
        options.insert(name.to_owned(), value);
    }
    Ok(options)
}

fn required_option<'a>(
    options: &'a BTreeMap<String, String>,
    name: &str,
) -> anyhow::Result<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .with_context(|| format!("missing --{name}"))
}

fn reject_unknown_options(
    options: &BTreeMap<String, String>,
    allowed: &[&str],
) -> anyhow::Result<()> {
    if let Some(name) = options
        .keys()
        .find(|name| !allowed.contains(&name.as_str()))
    {
        bail!("unknown option --{name}");
    }
    Ok(())
}

fn required_uuid(options: &BTreeMap<String, String>, name: &str) -> anyhow::Result<Uuid> {
    required_option(options, name)?
        .parse()
        .with_context(|| format!("invalid --{name}"))
}

fn required_i64(options: &BTreeMap<String, String>, name: &str) -> anyhow::Result<i64> {
    let value = required_option(options, name)?
        .parse::<i64>()
        .with_context(|| format!("invalid --{name}"))?;
    if value < 1 {
        bail!("--{name} must be positive");
    }
    Ok(value)
}

fn optional_u16(
    options: &BTreeMap<String, String>,
    name: &str,
    default: u16,
) -> anyhow::Result<u16> {
    options.get(name).map_or(Ok(default), |value| {
        value
            .parse::<u16>()
            .with_context(|| format!("invalid --{name}"))
    })
}

fn optional_bool(
    options: &BTreeMap<String, String>,
    name: &str,
    default: bool,
) -> anyhow::Result<bool> {
    options
        .get(name)
        .map_or(Ok(default), |value| match value.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => bail!("invalid --{name}; expected true or false"),
        })
}

fn parse_scopes(value: &str) -> anyhow::Result<Vec<String>> {
    let mut scopes = value
        .split(',')
        .map(str::trim)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty() || scopes.iter().any(String::is_empty) {
        bail!("--scopes must be a comma-separated list");
    }
    Ok(scopes)
}

/// Write a secret to a new owner-only file; refuses to overwrite.
fn write_private_file(path: &std::path::Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// `secrets-verify-export --rows PATH`: read a keyring from stdin (so the
/// drill never writes it to disk) and report how many rows, exported with
/// the query `secrets-verify-export --print-query` prints, open with it.
/// Exits nonzero unless every row opens. Prints no values.
fn verify_secret_export() -> anyhow::Result<()> {
    use std::io::Read as _;
    const USAGE: &str = "usage: secrets-verify-export --print-query | --rows PATH";
    let arguments = std::env::args().skip(2).collect::<Vec<_>>();
    let rows_path = match arguments.as_slice() {
        [flag] if flag == "--print-query" => {
            println!("{}", agent_workspace_server::secrets::EXPORT_ROWS_SQL);
            return Ok(());
        }
        [flag, path] if flag == "--rows" => path,
        _ => bail!(USAGE),
    };
    let mut keyring = zeroize::Zeroizing::new(String::new());
    std::io::stdin()
        .read_to_string(&mut keyring)
        .context("read the keyring from stdin")?;
    let keyring = agent_workspace_server::secrets::FileKeyring::parse(&keyring)
        .map_err(|_| anyhow::anyhow!("the keyring on stdin is unreadable"))?;
    let rows = std::fs::read_to_string(rows_path)
        .with_context(|| format!("read exported rows from {rows_path}"))?;
    let report = agent_workspace_server::secrets::verify_exported_rows(&keyring, &rows)
        .map_err(|_| anyhow::anyhow!("exported rows are malformed"))?;
    println!("{}", serde_json::to_string(&report)?);
    if report.failed > 0 {
        bail!(
            "{} of {} secrets did not open",
            report.failed,
            report.checked
        );
    }
    Ok(())
}

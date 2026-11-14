use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use thiserror::Error;

const PREFIX: &str = "AGENT_WORKSPACE_";
pub const GITHUB_API_VERSION: &str = "2026-03-10";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Environment {
    Development,
    Test,
    Production,
}

impl Environment {
    pub fn is_development(self) -> bool {
        matches!(self, Self::Development)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Production => "production",
        })
    }
}

impl FromStr for Environment {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "development" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            "production" => Ok(Self::Production),
            _ => Err(ConfigError::Invalid {
                key: format!("{PREFIX}ENV"),
                reason: "expected development, test, or production".into(),
            }),
        }
    }
}

#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub environment: Environment,
    pub server_addr: SocketAddr,
    pub webhook_addr: SocketAddr,
    pub allowed_origins: Vec<String>,
    /// Operator-set webhook origins (`scheme://host[:port]`) that may use
    /// http and private or tailnet addresses, such as an agent host's wakeup
    /// listener on a Tailscale IP. Every other webhook must be public https.
    pub webhook_trusted_origins: Vec<String>,
    /// The address people and cloud agents use, e.g. `https://workspace.example.com`.
    /// OAuth metadata and MCP resource URLs are built from it. Defaults to the
    /// loopback server address.
    pub public_origin: Option<String>,
    pub database_url: SecretString,
    pub database_max_connections: u32,
    pub data_root: PathBuf,
    pub data_quota_bytes: u64,
    pub minimum_free_bytes: u64,
    pub web_root: PathBuf,
    pub codex_binary: PathBuf,
    pub codex_home_root: PathBuf,
    pub codex_host_concurrency_limit: usize,
    pub codex_max_line_bytes: usize,
    pub codex_max_stderr_bytes: usize,
    pub codex_max_buffered_events: usize,
    pub codex_request_timeout: Duration,
    pub codex_shutdown_timeout: Duration,
    /// How often a Codex Cloud task's status is read while it runs.
    pub codex_cloud_poll_interval: Duration,
    /// The Claude Code command line (H6).
    pub claude_binary: PathBuf,
    /// The Claude API, for Claude Code cloud runs (C3). Tests point it at a
    /// fake server.
    pub anthropic_api_url: String,
    /// Directories a registered command-line harness's program must sit in
    /// (H7). Empty, command-line harnesses can't run on this server.
    pub command_harness_dirs: Vec<PathBuf>,
    pub demo_executor: bool,
    pub github: Option<GitHubConfig>,
    pub workos: Option<WorkOsConfig>,
    pub log_filter: String,
    /// The secrets master keyring (S1): an owner-only file outside the data
    /// and release roots. Unset in development, a keyring under
    /// `.agent-workspace-secrets/` in the working directory is created on
    /// first use; unset elsewhere, encrypted secrets are unavailable.
    pub secrets_key_path: Option<PathBuf>,
    /// Who push services contact about this server's Web Push (N3): a
    /// `mailto:` or `https:` URL. Unset, the public origin when it's https,
    /// else a mailto at the server's host.
    pub push_contact: Option<String>,
    /// The bearer token `/metrics` requires (O2), as an owner-only file.
    pub metrics_token_path: Option<PathBuf>,
    /// Background jobs run at once in this process (F9).
    pub job_concurrency: usize,
    /// Lets workspaces point their model provider (`ai.base_url`) and mail
    /// server at private, loopback and tailnet addresses, and mail at any
    /// port. Off, both must be public addresses, and mail must use a mail
    /// port: those requests leave from inside this server's network, and a
    /// workspace's owner isn't necessarily the person who runs the server.
    /// An operator running a local model or mail relay turns it on.
    pub allow_private_addresses: bool,
    /// People who run this server, by login name, besides whoever set it up
    /// from its terminal (`bootstrap-admin`). Only they see Settings ›
    /// Workspace › Server.
    pub operator_logins: Vec<String>,
    /// Which set of defaults this server started from
    /// (`AGENT_WORKSPACE_PROFILE`); any variable set still wins.
    pub profile: Profile,
    /// Whether this server fetches previews of links people post
    /// (`AGENT_WORKSPACE_LINK_PREVIEWS`). Off, no workspace gets them,
    /// whatever its own setting says.
    pub link_previews: bool,
    /// Set when `AGENT_WORKSPACE_SECRETS_KEY_PATH` names a file that doesn't
    /// exist yet in a safe place: the server creates the keyring there when
    /// it starts (`secrets::ensure_keyring`).
    pub secrets_key_missing: bool,
}

/// A set of defaults sized for one kind of server (H1.12). A profile only
/// sets defaults; a variable that is set still wins, except that `hosted`
/// refuses the switches a shared server must never have.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    /// One small machine (about 1 vCPU and 1 GB): a small pool, one job at
    /// a time, no link previews. The numbers are a guess until measured.
    Small,
    /// Today's defaults.
    Standard,
    /// A server many workspaces share: no private addresses, no trusted
    /// webhook origins, no development helpers.
    Hosted,
}

impl Profile {
    fn defaults(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Small => &[
                ("DATABASE_MAX_CONNECTIONS", "4"),
                ("JOB_CONCURRENCY", "1"),
                ("LINK_PREVIEWS", "false"),
            ],
            Self::Standard => &[],
            Self::Hosted => &[("DATABASE_MAX_CONNECTIONS", "20"), ("JOB_CONCURRENCY", "8")],
        }
    }
}

impl FromStr for Profile {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "small" => Ok(Self::Small),
            "standard" => Ok(Self::Standard),
            "hosted" => Ok(Self::Hosted),
            _ => Err(ConfigError::Invalid {
                key: format!("{PREFIX}PROFILE"),
                reason: "expected small, standard, or hosted".into(),
            }),
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Small => "small",
            Self::Standard => "standard",
            Self::Hosted => "hosted",
        })
    }
}

/// Reads every `AGENT_WORKSPACE_<NAME>_FILE` into `AGENT_WORKSPACE_<NAME>`,
/// so a container can pass secrets as files (`DATABASE_URL_FILE`). Setting
/// both is refused; the file's trailing newline is dropped.
fn with_file_values(
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ConfigError> {
    const MAX_FILE_VALUE_BYTES: u64 = 64 * 1024;
    let mut resolved = values.clone();
    for (key, path) in values {
        let Some(name) = key
            .strip_suffix("_FILE")
            .filter(|name| name.len() > PREFIX.len())
        else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            resolved.remove(key);
            continue;
        }
        let invalid = |reason: &str| ConfigError::Invalid {
            key: key.clone(),
            reason: reason.to_owned(),
        };
        if values
            .get(name)
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(invalid(&format!("set {name} or {key}, not both")));
        }
        let metadata = std::fs::metadata(path).map_err(|_| invalid("the file can't be read"))?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_VALUE_BYTES {
            return Err(invalid("must name a file of at most 64 KiB"));
        }
        let contents =
            std::fs::read_to_string(path).map_err(|_| invalid("the file can't be read"))?;
        let value = contents.trim_end_matches(['\n', '\r']).to_owned();
        if value.is_empty() {
            return Err(invalid("the file is empty"));
        }
        resolved.remove(key);
        resolved.insert(name.to_owned(), value);
    }
    Ok(resolved)
}

#[derive(Clone, Debug)]
pub struct WorkOsConfig {
    pub client_id: String,
    pub api_key: SecretString,
    pub allow_signup: bool,
    pub authkit_domain: Option<String>,
    pub google_enabled: bool,
    pub agent_registration_enabled: bool,
    /// Where the WorkOS API lives: always `https://api.workos.com` outside
    /// development, where the browser tests point it at a stand-in.
    pub api_base: String,
}

#[derive(Clone)]
pub struct GitHubConfig {
    pub app_id: u64,
    pub private_key_path: PathBuf,
    pub webhook_secret_path: PathBuf,
    pub webhook_previous_secret_path: Option<PathBuf>,
    pub api_base_url: String,
    pub api_version: String,
    /// For connecting through the App's install page (S5); unset, people
    /// can't connect GitHub from settings.
    pub connect: Option<GitHubConnectConfig>,
}

/// The App's public name and its OAuth client, which let a person install
/// the App and prove which installations they can reach.
#[derive(Clone)]
pub struct GitHubConnectConfig {
    /// The App's URL name: `github.com/apps/<slug>`.
    pub app_slug: String,
    pub client_id: String,
    pub client_secret_path: PathBuf,
}

impl fmt::Debug for GitHubConnectConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHubConnectConfig")
            .field("app_slug", &self.app_slug)
            .field("client_id", &self.client_id)
            .field("client_secret_path", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Debug for GitHubConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHubConfig")
            .field("app_id", &self.app_id)
            .field("private_key_path", &"[REDACTED]")
            .field("webhook_secret_path", &"[REDACTED]")
            .field(
                "webhook_previous_secret_path",
                &self
                    .webhook_previous_secret_path
                    .as_ref()
                    .map(|_| "[REDACTED]"),
            )
            .field("api_base_url", &self.api_base_url)
            .field("api_version", &self.api_version)
            .field("connect", &self.connect)
            .finish()
    }
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let initial_environment = std::env::var(format!("{PREFIX}ENV"))
            .unwrap_or_else(|_| "development".to_owned())
            .parse::<Environment>()?;

        if initial_environment.is_development() {
            let _ = dotenvy::dotenv();
        }

        let values = std::env::vars()
            .filter(|(key, _)| key.starts_with(PREFIX))
            .collect::<BTreeMap<_, _>>();
        Self::from_values(&values)
    }

    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let mut values = with_file_values(values)?;
        let profile = optional(&values, "PROFILE", "standard").parse::<Profile>()?;
        for (suffix, default) in profile.defaults() {
            let key = format!("{PREFIX}{suffix}");
            if values.get(&key).is_none_or(|value| value.trim().is_empty()) {
                values.insert(key, (*default).to_owned());
            }
        }
        let values = &values;
        let environment = optional(values, "ENV", "development").parse::<Environment>()?;
        let server_addr = parse(values, "SERVER_ADDR", "127.0.0.1:3100")?;
        let webhook_addr = parse(values, "WEBHOOK_ADDR", "127.0.0.1:3101")?;
        let database_url = required(values, "DATABASE_URL")?;
        let database_max_connections = parse(values, "DATABASE_MAX_CONNECTIONS", "5")?;
        let data_root = PathBuf::from(optional(values, "DATA_ROOT", ".agent-workspace"));
        let data_quota_bytes = parse(values, "DATA_QUOTA_BYTES", "21474836480")?;
        let minimum_free_bytes = parse(values, "MINIMUM_FREE_BYTES", "5368709120")?;
        let web_root = PathBuf::from(optional(values, "WEB_ROOT", "apps/web/dist"));
        let codex_binary = PathBuf::from(optional(values, "CODEX_BINARY", "codex"));
        // Harness account homes live in the data root unless placed elsewhere
        // inside it.
        let codex_home_root = PathBuf::from(optional(
            values,
            "CODEX_HOME_ROOT",
            &data_root.join("codex/profiles").to_string_lossy(),
        ));
        let codex_host_concurrency_limit = parse(values, "CODEX_HOST_CONCURRENCY_LIMIT", "1")?;
        let codex_max_line_bytes = parse(values, "CODEX_MAX_LINE_BYTES", "1048576")?;
        let codex_max_stderr_bytes = parse(values, "CODEX_MAX_STDERR_BYTES", "262144")?;
        let codex_max_buffered_events = parse(values, "CODEX_MAX_BUFFERED_EVENTS", "64")?;
        let codex_request_timeout =
            Duration::from_secs(parse(values, "CODEX_REQUEST_TIMEOUT_SECONDS", "120")?);
        let codex_shutdown_timeout =
            Duration::from_secs(parse(values, "CODEX_SHUTDOWN_TIMEOUT_SECONDS", "5")?);
        let codex_cloud_poll_interval =
            Duration::from_millis(parse(values, "CODEX_CLOUD_POLL_MILLISECONDS", "15000")?);
        let claude_binary = PathBuf::from(optional(values, "CLAUDE_BINARY", "claude"));
        let anthropic_api_url = optional(values, "ANTHROPIC_API_URL", "https://api.anthropic.com")
            .trim_end_matches('/')
            .to_owned();
        if !(anthropic_api_url.starts_with("https://")
            || anthropic_api_url.starts_with("http://127.0.0.1")
            || anthropic_api_url.starts_with("http://localhost"))
        {
            return Err(ConfigError::Invalid {
                key: "AGENT_WORKSPACE_ANTHROPIC_API_URL".into(),
                reason: "use an https URL (or a loopback http one for tests)".into(),
            });
        }
        let command_harness_dirs = optional(values, "COMMAND_HARNESS_DIRS", "")
            .split(':')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if command_harness_dirs.iter().any(|path| !path.is_absolute()) {
            return Err(ConfigError::Invalid {
                key: "AGENT_WORKSPACE_COMMAND_HARNESS_DIRS".into(),
                reason: "list absolute directories separated by colons".into(),
            });
        }
        let demo_executor = parse(values, "DEMO_EXECUTOR", "false")?;
        let github = github_config(values, &data_root, &web_root)?;
        let log_filter = optional(values, "LOG_FILTER", "agent_workspace=info,tower_http=info");
        let webhook_trusted_origins = webhook_trusted_origins(values)?;
        let public_origin = public_origin(values)?;
        let allowed_origins = allowed_origins(values, environment, public_origin.as_deref())?;

        validate_loopback("SERVER_ADDR", server_addr, environment)?;
        validate_loopback("WEBHOOK_ADDR", webhook_addr, environment)?;

        if database_max_connections == 0 {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}DATABASE_MAX_CONNECTIONS"),
                reason: "must be greater than zero".into(),
            });
        }

        if data_root.as_os_str().is_empty()
            || web_root.as_os_str().is_empty()
            || codex_home_root.as_os_str().is_empty()
        {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}DATA_ROOT"),
                reason: "runtime paths cannot be empty".into(),
            });
        }
        if data_quota_bytes < 1024 * 1024 * 1024 {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}DATA_QUOTA_BYTES"),
                reason: "must be at least 1073741824".into(),
            });
        }
        if minimum_free_bytes < 1024 * 1024 * 1024 {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}MINIMUM_FREE_BYTES"),
                reason: "must be at least 1073741824".into(),
            });
        }
        if !(1..=16).contains(&codex_host_concurrency_limit) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_HOST_CONCURRENCY_LIMIT"),
                reason: "must be between 1 and 16".into(),
            });
        }
        if !(1024..=16 * 1024 * 1024).contains(&codex_max_line_bytes) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_MAX_LINE_BYTES"),
                reason: "must be between 1024 and 16777216".into(),
            });
        }
        if !(1024..=4 * 1024 * 1024).contains(&codex_max_stderr_bytes) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_MAX_STDERR_BYTES"),
                reason: "must be between 1024 and 4194304".into(),
            });
        }
        if !(1..=1_024).contains(&codex_max_buffered_events) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_MAX_BUFFERED_EVENTS"),
                reason: "must be between 1 and 1024".into(),
            });
        }
        if !(Duration::from_secs(1)..=Duration::from_secs(600)).contains(&codex_request_timeout) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_REQUEST_TIMEOUT_SECONDS"),
                reason: "must be between 1 and 600".into(),
            });
        }
        if !(Duration::from_secs(1)..=Duration::from_secs(30)).contains(&codex_shutdown_timeout) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_SHUTDOWN_TIMEOUT_SECONDS"),
                reason: "must be between 1 and 30".into(),
            });
        }
        if !(Duration::from_millis(10)..=Duration::from_secs(600))
            .contains(&codex_cloud_poll_interval)
        {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_CLOUD_POLL_MILLISECONDS"),
                reason: "must be between 10 and 600000".into(),
            });
        }
        // Each harness account gets a private home beneath this root, which
        // must sit inside the data root; checked here so a misplaced root
        // stops the server at start instead of refusing every new account.
        if crate::profile_runtime::resolve_path_within_data_root(&data_root, &codex_home_root)
            .is_err()
        {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}CODEX_HOME_ROOT"),
                reason: format!("must be a folder inside {PREFIX}DATA_ROOT"),
            });
        }
        if demo_executor && !environment.is_development() {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}DEMO_EXECUTOR"),
                reason: "the demo executor is development-only".into(),
            });
        }

        let workos = workos_config(values, environment, &data_root, &web_root)?;
        let (secrets_key_path, secrets_key_missing) =
            secrets_key_path(values, &data_root, &web_root)?;
        // The master key opens every stored credential; a checkout (or an
        // etckeeper `/etc`) could commit it.
        if let Some(work_tree) = secrets_key_path
            .as_deref()
            .and_then(git_work_tree_containing)
        {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}SECRETS_KEY_PATH"),
                reason: format!(
                    "{} is a Git work tree; keep the keyring outside it",
                    work_tree.display()
                ),
            });
        }
        let push_contact =
            Some(optional(values, "PUSH_CONTACT", "")).filter(|contact| !contact.is_empty());
        if push_contact.as_deref().is_some_and(|contact| {
            !(contact.starts_with("mailto:") || contact.starts_with("https://"))
                || contact.len() > 200
                || contact.chars().any(|c| c.is_control() || c.is_whitespace())
        }) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}PUSH_CONTACT"),
                reason: "use a mailto: or https: URL".into(),
            });
        }
        let metrics_token_path =
            optional_secret_path(values, "METRICS_TOKEN_PATH", &data_root, &web_root, 4096)?;
        let job_concurrency = parse(values, "JOB_CONCURRENCY", "4")?;
        if !(1..=64).contains(&job_concurrency) {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}JOB_CONCURRENCY"),
                reason: "must be between 1 and 64".into(),
            });
        }
        let allow_private_addresses = parse(values, "ALLOW_PRIVATE_ADDRESSES", "false")?;
        let operator_logins = operator_logins(values)?;
        let link_previews = parse(values, "LINK_PREVIEWS", "true")?;
        if profile == Profile::Hosted {
            // A shared server's network is not any one workspace's to reach.
            for (refused, set) in [
                ("ALLOW_PRIVATE_ADDRESSES", allow_private_addresses),
                (
                    "WEBHOOK_TRUSTED_ORIGINS",
                    !webhook_trusted_origins.is_empty(),
                ),
                ("DEMO_EXECUTOR", demo_executor),
            ] {
                if set {
                    return Err(ConfigError::Invalid {
                        key: format!("{PREFIX}{refused}"),
                        reason: "a hosted server can't turn this on".into(),
                    });
                }
            }
        }
        Ok(Self {
            environment,
            server_addr,
            webhook_addr,
            allowed_origins,
            webhook_trusted_origins,
            public_origin,
            database_url: SecretString::new(database_url),
            database_max_connections,
            data_root,
            data_quota_bytes,
            minimum_free_bytes,
            web_root,
            codex_binary,
            codex_home_root,
            codex_host_concurrency_limit,
            codex_max_line_bytes,
            codex_max_stderr_bytes,
            codex_max_buffered_events,
            codex_request_timeout,
            codex_shutdown_timeout,
            codex_cloud_poll_interval,
            claude_binary,
            anthropic_api_url,
            command_harness_dirs,
            demo_executor,
            github,
            workos,
            log_filter,
            secrets_key_path,
            push_contact,
            metrics_token_path,
            job_concurrency,
            allow_private_addresses,
            operator_logins,
            profile,
            link_previews,
            secrets_key_missing,
        })
    }
}

/// The keyring's path, and whether it still has to be created: a path to a
/// file that doesn't exist yet is accepted when it is somewhere a new
/// keyring may go (outside the data and web roots and any Git work tree, in
/// an existing directory), and the server creates it when it starts.
fn secrets_key_path(
    values: &BTreeMap<String, String>,
    data_root: &Path,
    web_root: &Path,
) -> Result<(Option<PathBuf>, bool), ConfigError> {
    let key = "SECRETS_KEY_PATH";
    let Some(value) = values
        .get(&format!("{PREFIX}{key}"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok((None, false));
    };
    let path = PathBuf::from(value);
    let missing = matches!(
        std::fs::symlink_metadata(&path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    );
    if missing {
        new_secret_location(&path, data_root, web_root).map_err(|reason| ConfigError::Invalid {
            key: format!("{PREFIX}{key}"),
            reason: format!("the keyring doesn't exist and can't be created there: {reason}"),
        })?;
        return Ok((Some(path), true));
    }
    validate_secret_file(key, &path, data_root, web_root, 64 * 1024)?;
    Ok((Some(path), false))
}

fn github_config(
    values: &BTreeMap<String, String>,
    data_root: &Path,
    web_root: &Path,
) -> Result<Option<GitHubConfig>, ConfigError> {
    let app_id = values.get(&format!("{PREFIX}GITHUB_APP_ID"));
    let private_key = values.get(&format!("{PREFIX}GITHUB_PRIVATE_KEY_PATH"));
    let webhook_secret = values.get(&format!("{PREFIX}GITHUB_WEBHOOK_SECRET_PATH"));
    let webhook_previous_secret = values
        .get(&format!("{PREFIX}GITHUB_WEBHOOK_PREVIOUS_SECRET_PATH"))
        .filter(|value| !value.trim().is_empty());
    let configured = [app_id, private_key, webhook_secret]
        .iter()
        .filter(|value| value.is_some_and(|value| !value.trim().is_empty()))
        .count();
    if configured == 0 && webhook_previous_secret.is_none() {
        return Ok(None);
    }
    if configured != 3 {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_APP_ID"),
            reason: "GitHub App ID, private-key path, and webhook-secret path must be configured together".into(),
        });
    }

    let app_id = app_id
        .expect("counted GitHub App ID")
        .parse::<u64>()
        .map_err(|error| ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_APP_ID"),
            reason: error.to_string(),
        })?;
    if app_id == 0 {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_APP_ID"),
            reason: "must be greater than zero".into(),
        });
    }
    let private_key_path = PathBuf::from(private_key.expect("counted private-key path"));
    let webhook_secret_path = PathBuf::from(webhook_secret.expect("counted webhook-secret path"));
    validate_secret_file(
        "GITHUB_PRIVATE_KEY_PATH",
        &private_key_path,
        data_root,
        web_root,
        64 * 1024,
    )?;
    validate_secret_file(
        "GITHUB_WEBHOOK_SECRET_PATH",
        &webhook_secret_path,
        data_root,
        web_root,
        4096,
    )?;
    let webhook_previous_secret_path = webhook_previous_secret.map(PathBuf::from);
    if let Some(previous_path) = &webhook_previous_secret_path {
        validate_secret_file(
            "GITHUB_WEBHOOK_PREVIOUS_SECRET_PATH",
            previous_path,
            data_root,
            web_root,
            4096,
        )?;
        if previous_path == &webhook_secret_path || previous_path == &private_key_path {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}GITHUB_WEBHOOK_PREVIOUS_SECRET_PATH"),
                reason: "must be distinct from every current GitHub secret path".into(),
            });
        }
    }

    let api_base_url = optional(values, "GITHUB_API_BASE_URL", "https://api.github.com");
    if api_base_url != "https://api.github.com" {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_API_BASE_URL"),
            reason: "version one supports only https://api.github.com".into(),
        });
    }
    let api_version = optional(values, "GITHUB_API_VERSION", GITHUB_API_VERSION);
    if api_version != GITHUB_API_VERSION {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_API_VERSION"),
            reason: format!("must equal the pinned version {GITHUB_API_VERSION}"),
        });
    }

    let connect = github_connect_config(values, data_root, web_root)?;
    if let Some(connect) = &connect
        && [&private_key_path, &webhook_secret_path].contains(&&connect.client_secret_path)
    {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_CLIENT_SECRET_PATH"),
            reason: "must be distinct from every other GitHub secret path".into(),
        });
    }

    Ok(Some(GitHubConfig {
        app_id,
        private_key_path,
        webhook_secret_path,
        webhook_previous_secret_path,
        api_base_url,
        api_version,
        connect,
    }))
}

/// The App's slug, OAuth client id and client-secret file, all or none.
fn github_connect_config(
    values: &BTreeMap<String, String>,
    data_root: &Path,
    web_root: &Path,
) -> Result<Option<GitHubConnectConfig>, ConfigError> {
    let value = |suffix: &str| {
        values
            .get(&format!("{PREFIX}{suffix}"))
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let slug = value("GITHUB_APP_SLUG");
    let client_id = value("GITHUB_CLIENT_ID");
    let secret = value("GITHUB_CLIENT_SECRET_PATH");
    let (slug, client_id, secret) = match (slug, client_id, secret) {
        (None, None, None) => return Ok(None),
        (Some(slug), Some(client_id), Some(secret)) => (slug, client_id, secret),
        _ => {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}GITHUB_APP_SLUG"),
                reason:
                    "GitHub App slug, client ID, and client-secret path must be configured together"
                        .into(),
            });
        }
    };
    if !(1..=100).contains(&slug.len())
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_APP_SLUG"),
            reason: "must be the App's URL name: lower-case letters, digits and hyphens".into(),
        });
    }
    if !(1..=100).contains(&client_id.len())
        || !client_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
    {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}GITHUB_CLIENT_ID"),
            reason: "must be the App's client ID".into(),
        });
    }
    let client_secret_path = PathBuf::from(secret);
    validate_secret_file(
        "GITHUB_CLIENT_SECRET_PATH",
        &client_secret_path,
        data_root,
        web_root,
        4096,
    )?;
    Ok(Some(GitHubConnectConfig {
        app_slug: slug,
        client_id,
        client_secret_path,
    }))
}

fn optional_secret_path(
    values: &BTreeMap<String, String>,
    key: &str,
    data_root: &Path,
    web_root: &Path,
    maximum_bytes: u64,
) -> Result<Option<PathBuf>, ConfigError> {
    let Some(value) = values
        .get(&format!("{PREFIX}{key}"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    validate_secret_file(key, &path, data_root, web_root, maximum_bytes)?;
    Ok(Some(path))
}

/// Whether `path` is absolute, has no `.` or `..` parts, and neither its
/// text nor its resolved directory falls under a served or runtime root.
fn secret_path_outside_roots(path: &Path, data_root: &Path, web_root: &Path) -> bool {
    let plain = path
        .components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)));
    // Compare resolved paths too: a symlinked directory could otherwise point
    // into a served root while the text looks outside it.
    let resolved = path
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .zip(path.file_name())
        .map(|(parent, name)| parent.join(name));
    let Some(resolved) = resolved else {
        return false;
    };
    path.is_absolute()
        && plain
        && [data_root, web_root].into_iter().all(|root| {
            let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            !path.starts_with(root) && !resolved.starts_with(&canonical)
        })
}

/// Where `secret-key-init` may create a keyring: everywhere a configured
/// secret file may live, and not inside a Git work tree (a checkout, or an
/// `/etc` kept by etckeeper), where the key could be committed.
pub fn validate_new_secret_location(path: &Path, config: &AppConfig) -> Result<(), String> {
    new_secret_location(path, &config.data_root, &config.web_root)
}

fn new_secret_location(path: &Path, data_root: &Path, web_root: &Path) -> Result<(), String> {
    if !secret_path_outside_roots(path, data_root, web_root) {
        return Err(
            "the path must be absolute, in an existing directory, and outside the data and web roots"
                .into(),
        );
    }
    path.parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .ok_or("the directory must exist")?;
    if let Some(work_tree) = git_work_tree_containing(path) {
        return Err(format!(
            "{} is a Git work tree; keep the keyring outside it",
            work_tree.display()
        ));
    }
    Ok(())
}

/// The nearest Git work tree holding `path`'s resolved directory, if any.
fn git_work_tree_containing(path: &Path) -> Option<PathBuf> {
    let directory = std::fs::canonicalize(path.parent()?).ok()?;
    directory
        .ancestors()
        .find(|ancestor| std::fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

fn validate_secret_file(
    key: &str,
    path: &Path,
    data_root: &Path,
    web_root: &Path,
    maximum_bytes: u64,
) -> Result<(), ConfigError> {
    if !secret_path_outside_roots(path, data_root, web_root) {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}{key}"),
            reason: "must be an absolute path outside runtime data and release roots".into(),
        });
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ConfigError::Invalid {
        key: format!("{PREFIX}{key}"),
        reason: "secret file is unavailable".into(),
    })?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}{key}"),
            reason: "secret must be a regular owner-only file".into(),
        });
    }
    if metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}{key}"),
            reason: format!("secret file must contain between 1 and {maximum_bytes} bytes"),
        });
    }
    Ok(())
}

fn public_origin(values: &BTreeMap<String, String>) -> Result<Option<String>, ConfigError> {
    let Some(value) = values
        .get(&format!("{PREFIX}PUBLIC_ORIGIN"))
        .map(|value| value.trim().trim_end_matches('/'))
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let parsed = url::Url::parse(value).map_err(|error| ConfigError::Invalid {
        key: format!("{PREFIX}PUBLIC_ORIGIN"),
        reason: error.to_string(),
    })?;
    // Browsers treat localhost as a secure origin, so `__Host-` cookies work
    // there over http: a trial of a production build on a laptop needs no
    // certificate. Any other host must be https.
    let loopback = parsed.scheme() == "http"
        && parsed.host().is_some_and(|host| match host {
            url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(ip) => ip.is_loopback(),
            url::Host::Ipv6(ip) => ip.is_loopback(),
        });
    if (parsed.scheme() != "https" && !loopback)
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}PUBLIC_ORIGIN"),
            reason: "must be an HTTPS origin without credentials, query, or fragment (or http on localhost or a loopback address)".into(),
        });
    }
    Ok(Some(parsed.origin().ascii_serialization()))
}

fn operator_logins(values: &BTreeMap<String, String>) -> Result<Vec<String>, ConfigError> {
    let key = format!("{PREFIX}OPERATOR_LOGINS");
    let configured = values.get(&key).map(String::as_str).unwrap_or_default();
    let mut logins = Vec::new();
    for candidate in configured
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let login = candidate.to_ascii_lowercase();
        if login.len() > 64
            || !login
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(ConfigError::Invalid {
                key,
                reason: format!("{candidate}: list login names separated by commas"),
            });
        }
        logins.push(login);
    }
    Ok(logins)
}

fn webhook_trusted_origins(values: &BTreeMap<String, String>) -> Result<Vec<String>, ConfigError> {
    let key = format!("{PREFIX}WEBHOOK_TRUSTED_ORIGINS");
    let configured = values.get(&key).map(String::as_str).unwrap_or_default();
    let mut origins = Vec::new();
    for candidate in configured
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let invalid = |reason: &str| ConfigError::Invalid {
            key: key.clone(),
            reason: format!("{candidate}: {reason}"),
        };
        let url = reqwest::Url::parse(candidate).map_err(|error| invalid(&error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid("use an origin such as http://100.64.0.1:8732"));
        }
        origins.push(url.origin().ascii_serialization());
    }
    Ok(origins)
}

fn allowed_origins(
    values: &BTreeMap<String, String>,
    environment: Environment,
    public_origin: Option<&str>,
) -> Result<Vec<String>, ConfigError> {
    let configured = values
        .get(&format!("{PREFIX}ALLOWED_ORIGINS"))
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    let candidates = if configured.is_empty() && environment.is_development() {
        vec![
            "http://127.0.0.1:3100",
            "http://localhost:3100",
            "http://127.0.0.1:5173",
            "http://localhost:5173",
        ]
    } else if configured.is_empty() {
        // Outside development the browser app is served from the public
        // origin unless the operator lists others.
        match public_origin {
            Some(origin) => vec![origin],
            None => return Err(ConfigError::Missing(format!("{PREFIX}ALLOWED_ORIGINS"))),
        }
    } else {
        configured.split(',').map(str::trim).collect()
    };

    let mut origins = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let parsed = candidate
            .parse::<http::Uri>()
            .map_err(|error| ConfigError::Invalid {
                key: format!("{PREFIX}ALLOWED_ORIGINS"),
                reason: error.to_string(),
            })?;
        let valid_scheme = matches!(parsed.scheme_str(), Some("http" | "https"));
        let valid_authority = parsed
            .authority()
            .is_some_and(|authority| !authority.as_str().contains('@'));
        let valid_path = parsed
            .path_and_query()
            .is_none_or(|path| path.as_str() == "/");
        if !valid_scheme
            || !valid_authority
            || !valid_path
            || candidate.contains('*')
            || candidate == "null"
        {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}ALLOWED_ORIGINS"),
                reason: "origins must be exact http(s) origins without paths or wildcards".into(),
            });
        }
        let normalized = candidate.trim_end_matches('/').to_owned();
        if !origins.contains(&normalized) {
            origins.push(normalized);
        }
    }
    Ok(origins)
}

fn validate_loopback(
    key: &str,
    address: SocketAddr,
    environment: Environment,
) -> Result<(), ConfigError> {
    if environment.is_development() && !address.ip().is_loopback() {
        return Err(ConfigError::Invalid {
            key: format!("{PREFIX}{key}"),
            reason: "development listeners must bind to loopback".into(),
        });
    }
    Ok(())
}

fn required(values: &BTreeMap<String, String>, suffix: &str) -> Result<String, ConfigError> {
    values
        .get(&format!("{PREFIX}{suffix}"))
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| ConfigError::Missing(format!("{PREFIX}{suffix}")))
}

fn optional(values: &BTreeMap<String, String>, suffix: &str, default: &str) -> String {
    values
        .get(&format!("{PREFIX}{suffix}"))
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| default.to_owned())
}

fn parse<T>(
    values: &BTreeMap<String, String>,
    suffix: &str,
    default: &str,
) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    optional(values, suffix, default)
        .parse::<T>()
        .map_err(|error| ConfigError::Invalid {
            key: format!("{PREFIX}{suffix}"),
            reason: error.to_string(),
        })
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(String),
    #[error("invalid configuration for {key}: {reason}")]
    Invalid { key: String, reason: String },
}

pub const WORKOS_API_BASE: &str = "https://api.workos.com";

fn workos_config(
    values: &BTreeMap<String, String>,
    environment: Environment,
    data_root: &Path,
    web_root: &Path,
) -> Result<Option<WorkOsConfig>, ConfigError> {
    let client_id = values.get("AGENT_WORKSPACE_WORKOS_CLIENT_ID");
    let key_path = values.get("AGENT_WORKSPACE_WORKOS_API_KEY_PATH");
    if client_id.is_none() && key_path.is_none() {
        return Ok(None);
    }
    let error = || ConfigError::Invalid {
        key: "AGENT_WORKSPACE_WORKOS_CLIENT_ID".into(),
        reason: "set a client ID and protected API key file together".into(),
    };
    let client_id = client_id
        .filter(|id| id.starts_with("client_") && id.len() < 100)
        .ok_or_else(error)?
        .clone();
    let path = PathBuf::from(key_path.ok_or_else(error)?);
    validate_secret_file("WORKOS_API_KEY_PATH", &path, data_root, web_root, 4096)?;
    let key = std::fs::read_to_string(path).map_err(|_| error())?;
    if !key.trim().starts_with("sk_") {
        return Err(error());
    }
    let allow_signup = match values
        .get("AGENT_WORKSPACE_WORKOS_ALLOW_SIGNUP")
        .map(String::as_str)
    {
        None | Some("false") => false,
        Some("true") => true,
        _ => return Err(error()),
    };
    let authkit_domain = values
        .get("AGENT_WORKSPACE_WORKOS_AUTHKIT_DOMAIN")
        .map(|value| {
            let url = url::Url::parse(value).map_err(|_| error())?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
            {
                return Err(error());
            }
            Ok(url.as_str().trim_end_matches('/').to_owned())
        })
        .transpose()?;
    let flag = |key: &str| -> Result<bool, ConfigError> {
        match values.get(key).map(String::as_str) {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            _ => Err(error()),
        }
    };
    let google_enabled = flag("AGENT_WORKSPACE_WORKOS_GOOGLE_ENABLED")?;
    let agent_registration_enabled = flag("AGENT_WORKSPACE_WORKOS_AGENT_REGISTRATION_ENABLED")?;
    if agent_registration_enabled && authkit_domain.is_none() {
        return Err(error());
    }
    let api_base = match values.get("AGENT_WORKSPACE_WORKOS_API_BASE") {
        None => WORKOS_API_BASE.to_owned(),
        Some(value) => {
            let url = url::Url::parse(value).map_err(|_| ConfigError::Invalid {
                key: "AGENT_WORKSPACE_WORKOS_API_BASE".into(),
                reason: "must be a URL".into(),
            })?;
            let loopback = url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
                && url.path() == "/"
                && url.query().is_none();
            if !environment.is_development() || !loopback {
                return Err(ConfigError::Invalid {
                    key: "AGENT_WORKSPACE_WORKOS_API_BASE".into(),
                    reason: "only a loopback stand-in in development".into(),
                });
            }
            url.as_str().trim_end_matches('/').to_owned()
        }
    };
    Ok(Some(WorkOsConfig {
        api_base,
        authkit_domain,
        google_enabled,
        agent_registration_enabled,
        client_id,
        api_key: SecretString::new(key.trim()),
        allow_signup,
    }))
}

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

    use super::*;

    fn base_values() -> BTreeMap<String, String> {
        BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            "postgres://example.invalid/database".to_owned(),
        )])
    }

    #[test]
    fn a_secret_file_may_not_reach_a_served_root_through_dots_or_symlinks() {
        use std::os::unix::fs::{OpenOptionsExt as _, symlink};
        let base = std::env::temp_dir().join(format!("aw-secret-path-{}", uuid::Uuid::new_v4()));
        let web_root = base.join("release/web");
        let data_root = base.join("data");
        let outside = base.join("etc");
        for directory in [&web_root, &data_root, &outside] {
            std::fs::create_dir_all(directory).expect("make directory");
        }
        for file in [web_root.join("keyring"), outside.join("keyring")] {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&file)
                .and_then(|mut file| std::io::Write::write_all(&mut file, b"k"))
                .expect("write keyring");
        }
        symlink(&web_root, base.join("etc/linked-web")).expect("link");
        let check = |path: PathBuf| {
            validate_secret_file("SECRETS_KEY_PATH", &path, &data_root, &web_root, 64)
        };
        check(outside.join("keyring")).expect("a plain outside path is fine");
        check(base.join("release/../release/web/keyring")).expect_err("dots into the web root");
        check(base.join("etc/../release/web/keyring")).expect_err("dots anywhere");
        check(base.join("etc/linked-web/keyring")).expect_err("a symlinked directory into it");
        check(web_root.join("keyring")).expect_err("the web root itself");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_new_keyring_stays_out_of_served_roots_and_git_work_trees() {
        let base = std::env::temp_dir().join(format!("aw-new-secret-{}", uuid::Uuid::new_v4()));
        let checkout = base.join("checkout");
        for directory in [
            base.join("etc"),
            checkout.join(".git"),
            checkout.join("deep"),
        ] {
            std::fs::create_dir_all(directory).expect("make directory");
        }
        let mut config = AppConfig::from_values(&base_values()).expect("valid config");
        config.web_root = base.join("release/web");
        config.data_root = base.join("data");
        std::fs::create_dir_all(&config.web_root).expect("make web root");
        validate_new_secret_location(&base.join("etc/keyring"), &config)
            .expect("a plain outside path is fine");
        validate_new_secret_location(&config.web_root.join("keyring"), &config)
            .expect_err("the web root");
        validate_new_secret_location(&base.join("etc/../release/web/keyring"), &config)
            .expect_err("dots into the web root");
        validate_new_secret_location(&checkout.join("deep/keyring"), &config)
            .expect_err("inside a Git work tree");
        validate_new_secret_location(&base.join("missing/keyring"), &config)
            .expect_err("a directory that doesn't exist");
        // An existing keyring inside a checkout is refused at startup too.
        let committed = checkout.join("deep/keyring");
        std::fs::write(&committed, "k").expect("write keyring");
        std::fs::set_permissions(
            &committed,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .expect("owner only");
        let mut values = base_values();
        values.insert(
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            committed.to_string_lossy().into_owned(),
        );
        let error = AppConfig::from_values(&values).expect_err("keyring in a checkout");
        assert!(error.to_string().contains("Git work tree"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn harness_account_homes_must_sit_inside_the_data_root() {
        let mut values = base_values();
        values.insert(format!("{PREFIX}DATA_ROOT"), "/srv/aw/data".to_owned());
        values.insert(
            format!("{PREFIX}CODEX_HOME_ROOT"),
            "/srv/aw/codex".to_owned(),
        );
        let error = AppConfig::from_values(&values).expect_err("home root outside the data root");
        assert!(error.to_string().contains("CODEX_HOME_ROOT"), "{error}");
        values.insert(
            format!("{PREFIX}CODEX_HOME_ROOT"),
            "/srv/aw/data/codex/profiles".to_owned(),
        );
        AppConfig::from_values(&values).expect("home root inside the data root");
        values.remove(&format!("{PREFIX}CODEX_HOME_ROOT"));
        let config = AppConfig::from_values(&values).expect("default home root");
        assert_eq!(
            config.codex_home_root,
            std::path::Path::new("/srv/aw/data/codex/profiles")
        );
    }

    #[test]
    fn secrets_are_redacted_from_debug_output() {
        let config = AppConfig::from_values(&base_values()).expect("valid config");
        let debug = format!("{config:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("example.invalid"));
    }

    #[test]
    fn development_listener_must_be_loopback() {
        let mut values = base_values();
        values.insert(
            "AGENT_WORKSPACE_SERVER_ADDR".to_owned(),
            "0.0.0.0:3100".to_owned(),
        );

        let error = AppConfig::from_values(&values).expect_err("must reject public bind");
        assert!(error.to_string().contains("must bind to loopback"));
    }

    #[test]
    fn production_may_bind_to_a_non_loopback_address() {
        let mut values = base_values();
        values.insert("AGENT_WORKSPACE_ENV".to_owned(), "production".to_owned());
        values.insert(
            "AGENT_WORKSPACE_ALLOWED_ORIGINS".to_owned(),
            "https://workspace.example.com".to_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_SERVER_ADDR".to_owned(),
            "0.0.0.0:3100".to_owned(),
        );

        let config = AppConfig::from_values(&values).expect("valid production config");
        assert_eq!(
            config.server_addr.ip(),
            std::net::IpAddr::from([0, 0, 0, 0])
        );
    }

    #[test]
    fn production_requires_exact_allowed_origins() {
        let mut values = base_values();
        values.insert("AGENT_WORKSPACE_ENV".to_owned(), "production".to_owned());
        let error = AppConfig::from_values(&values).expect_err("origin must be configured");
        assert!(error.to_string().contains("ALLOWED_ORIGINS"));

        values.insert(
            "AGENT_WORKSPACE_ALLOWED_ORIGINS".to_owned(),
            "https://*.example.com".to_owned(),
        );
        let error = AppConfig::from_values(&values).expect_err("wildcard must be rejected");
        assert!(error.to_string().contains("without paths or wildcards"));
    }

    #[test]
    fn public_origin_rejects_credentials_and_insecure_production_urls() {
        for origin in [
            "https://user@example.com",
            "https://example.com?next=evil",
            "https://example.com/#fragment",
            "http://example.com",
        ] {
            let mut values = base_values();
            values.insert("AGENT_WORKSPACE_ENV".to_owned(), "production".to_owned());
            values.insert(
                "AGENT_WORKSPACE_ALLOWED_ORIGINS".to_owned(),
                "https://example.com".to_owned(),
            );
            values.insert(
                "AGENT_WORKSPACE_PUBLIC_ORIGIN".to_owned(),
                origin.to_owned(),
            );
            assert!(
                AppConfig::from_values(&values).is_err(),
                "accepted {origin}"
            );
        }

        let mut development = base_values();
        development.insert(
            "AGENT_WORKSPACE_PUBLIC_ORIGIN".to_owned(),
            "http://127.0.0.1:3100".to_owned(),
        );
        assert!(AppConfig::from_values(&development).is_ok());
    }

    #[test]
    fn production_on_loopback_may_use_http_and_takes_its_origins_from_the_public_one() {
        let mut values = base_values();
        values.insert("AGENT_WORKSPACE_ENV".to_owned(), "production".to_owned());
        values.insert(
            "AGENT_WORKSPACE_PUBLIC_ORIGIN".to_owned(),
            "http://localhost:3100".to_owned(),
        );
        let config = AppConfig::from_values(&values).expect("laptop trial");
        assert_eq!(config.allowed_origins, ["http://localhost:3100"]);
        values.insert(
            "AGENT_WORKSPACE_PUBLIC_ORIGIN".to_owned(),
            "http://127.0.0.1:3100".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_ok());
        // Any other host still needs https.
        values.insert(
            "AGENT_WORKSPACE_PUBLIC_ORIGIN".to_owned(),
            "http://workspace.example.com".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());
        // An https public origin is the allowed origin unless others are listed.
        values.insert(
            "AGENT_WORKSPACE_PUBLIC_ORIGIN".to_owned(),
            "https://workspace.example.com".to_owned(),
        );
        let config = AppConfig::from_values(&values).expect("public origin");
        assert_eq!(config.allowed_origins, ["https://workspace.example.com"]);
        values.insert(
            "AGENT_WORKSPACE_ALLOWED_ORIGINS".to_owned(),
            "https://app.example.com".to_owned(),
        );
        let config = AppConfig::from_values(&values).expect("listed origins");
        assert_eq!(config.allowed_origins, ["https://app.example.com"]);
    }

    #[test]
    fn any_variable_may_come_from_a_file_but_not_from_both() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let file = directory.path().join("database_url");
        std::fs::write(&file, "postgres://from-a-file.invalid/database\n").expect("write");
        let values = BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL_FILE".to_owned(),
            file.to_string_lossy().into_owned(),
        )]);
        let config = AppConfig::from_values(&values).expect("file value");
        assert_eq!(
            config.database_url.expose(),
            "postgres://from-a-file.invalid/database"
        );

        let mut both = values.clone();
        both.extend(base_values());
        let error = AppConfig::from_values(&both).expect_err("both set");
        assert!(error.to_string().contains("not both"));

        let missing = BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL_FILE".to_owned(),
            directory
                .path()
                .join("absent")
                .to_string_lossy()
                .into_owned(),
        )]);
        assert!(AppConfig::from_values(&missing).is_err());
        std::fs::write(&file, "\n").expect("write");
        assert!(AppConfig::from_values(&values).is_err(), "an empty file");
    }

    #[test]
    fn a_profile_sets_defaults_that_variables_still_override() {
        let mut values = base_values();
        let standard = AppConfig::from_values(&values).expect("standard");
        assert_eq!(standard.profile, Profile::Standard);
        assert_eq!(standard.database_max_connections, 5);
        assert_eq!(standard.job_concurrency, 4);
        assert!(standard.link_previews);

        values.insert("AGENT_WORKSPACE_PROFILE".to_owned(), "small".to_owned());
        let small = AppConfig::from_values(&values).expect("small");
        assert_eq!(small.database_max_connections, 4);
        assert_eq!(small.job_concurrency, 1);
        assert!(!small.link_previews);
        values.insert("AGENT_WORKSPACE_JOB_CONCURRENCY".to_owned(), "3".to_owned());
        assert_eq!(
            AppConfig::from_values(&values)
                .expect("override")
                .job_concurrency,
            3
        );

        values.insert("AGENT_WORKSPACE_PROFILE".to_owned(), "roomy".to_owned());
        assert!(AppConfig::from_values(&values).is_err());
    }

    #[test]
    fn a_hosted_server_refuses_switches_that_reach_its_own_network() {
        let mut values = base_values();
        values.insert("AGENT_WORKSPACE_PROFILE".to_owned(), "hosted".to_owned());
        let hosted = AppConfig::from_values(&values).expect("hosted");
        assert_eq!(hosted.database_max_connections, 20);
        for (key, value) in [
            ("AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES", "true"),
            (
                "AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS",
                "http://100.64.0.1:8732",
            ),
            ("AGENT_WORKSPACE_DEMO_EXECUTOR", "true"),
        ] {
            let mut refused = values.clone();
            refused.insert(key.to_owned(), value.to_owned());
            let error = AppConfig::from_values(&refused).expect_err(key);
            assert!(error.to_string().contains("hosted"), "{key}: {error}");
        }
    }

    #[test]
    fn a_missing_keyring_in_a_safe_place_is_created_on_first_start() {
        let keys = tempfile::tempdir().expect("keys directory");
        let path = keys.path().join("keyring");
        let mut values = base_values();
        values.insert(
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            path.to_string_lossy().into_owned(),
        );
        let config = AppConfig::from_values(&values).expect("missing keyring");
        assert!(config.secrets_key_missing);
        let created = crate::secrets::ensure_keyring(&config).expect("create");
        assert_eq!(created.as_deref(), Some(path.as_path()));
        assert_eq!(
            std::fs::metadata(&path)
                .expect("keyring")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        // Once it exists it is read, never replaced.
        let before = std::fs::read(&path).expect("read");
        assert_eq!(
            crate::secrets::ensure_keyring(&config).expect("again"),
            None
        );
        assert_eq!(std::fs::read(&path).expect("read"), before);
        let config = AppConfig::from_values(&values).expect("existing keyring");
        assert!(!config.secrets_key_missing);

        // A missing file in a directory that doesn't exist is refused.
        values.insert(
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            keys.path()
                .join("absent/keyring")
                .to_string_lossy()
                .into_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());
    }

    /// `docs/self-host/configuration.md` names every variable the server
    /// reads.
    #[test]
    fn every_variable_is_documented() {
        let source = include_str!("config.rs");
        let source = &source[..source
            .find("\n#[cfg(test)]\nmod tests")
            .unwrap_or(source.len())];
        let documented = include_str!("../../../docs/self-host/configuration.md");
        let mut names = std::collections::BTreeSet::new();
        for (index, _) in source.match_indices('"') {
            let rest = &source[index + 1..];
            let Some(end) = rest.find('"') else { continue };
            let literal = &rest[..end];
            let name = literal.strip_prefix(PREFIX).unwrap_or(literal);
            if name.len() > 2
                && name.starts_with(|c: char| c.is_ascii_uppercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                names.insert(name.to_owned());
            }
        }
        for (index, _) in source.match_indices("{PREFIX}") {
            let name = source[index + "{PREFIX}".len()..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect::<String>();
            if !name.is_empty() {
                names.insert(name);
            }
        }
        assert!(names.contains("DATABASE_URL") && names.contains("METRICS_TOKEN_PATH"));
        let missing = names
            .iter()
            .filter(|name| !documented.contains(&format!("`{PREFIX}{name}`")))
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "document these in docs/self-host/configuration.md: {missing:?}"
        );
    }

    #[test]
    fn public_origin_keeps_the_validated_canonical_origin() {
        for input in [
            "https:EXAMPLE.com",
            "https:/example.com",
            "https://EXAMPLE.com:443/",
        ] {
            let mut values = base_values();
            values.insert("AGENT_WORKSPACE_PUBLIC_ORIGIN".to_owned(), input.to_owned());
            let config = AppConfig::from_values(&values).expect("valid origin");
            assert_eq!(config.public_origin.as_deref(), Some("https://example.com"));
        }
    }

    #[test]
    fn codex_supervisor_limits_are_bounded() {
        let mut values = base_values();
        values.insert(
            "AGENT_WORKSPACE_CODEX_HOST_CONCURRENCY_LIMIT".to_owned(),
            "0".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());

        values.insert(
            "AGENT_WORKSPACE_CODEX_HOST_CONCURRENCY_LIMIT".to_owned(),
            "1".to_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_CODEX_MAX_LINE_BYTES".to_owned(),
            "16777217".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());

        values.insert(
            "AGENT_WORKSPACE_CODEX_MAX_LINE_BYTES".to_owned(),
            "1048576".to_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_CODEX_MAX_BUFFERED_EVENTS".to_owned(),
            "1025".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());

        values.insert(
            "AGENT_WORKSPACE_CODEX_MAX_BUFFERED_EVENTS".to_owned(),
            "64".to_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS".to_owned(),
            "601".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());
    }

    #[test]
    fn storage_watermarks_are_bounded_and_have_private_host_defaults() {
        let config = AppConfig::from_values(&base_values()).expect("valid config");
        assert_eq!(config.data_quota_bytes, 20 * 1024 * 1024 * 1024);
        assert_eq!(config.minimum_free_bytes, 5 * 1024 * 1024 * 1024);

        let mut values = base_values();
        values.insert(
            "AGENT_WORKSPACE_DATA_QUOTA_BYTES".to_owned(),
            "1073741823".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());
        values.insert(
            "AGENT_WORKSPACE_DATA_QUOTA_BYTES".to_owned(),
            "1073741824".to_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_MINIMUM_FREE_BYTES".to_owned(),
            "0".to_owned(),
        );
        assert!(AppConfig::from_values(&values).is_err());
    }

    #[test]
    fn github_configuration_is_all_or_none_and_redacts_paths() {
        let mut values = base_values();
        values.insert(
            "AGENT_WORKSPACE_GITHUB_APP_ID".to_owned(),
            "4442593".to_owned(),
        );
        let error = AppConfig::from_values(&values).expect_err("partial config must fail");
        assert!(error.to_string().contains("configured together"));

        let directory = tempfile::tempdir().expect("secret directory");
        let private_key = directory.path().join("github-app.pem");
        let webhook_secret = directory.path().join("github-webhook-secret");
        let previous_webhook_secret = directory.path().join("github-webhook-secret.previous");
        for (path, bytes) in [
            (&private_key, b"private-key-placeholder".as_slice()),
            (
                &webhook_secret,
                b"high-entropy-webhook-secret-placeholder".as_slice(),
            ),
            (
                &previous_webhook_secret,
                b"previous-high-entropy-webhook-secret".as_slice(),
            ),
        ] {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .expect("owner-only secret file");
            file.write_all(bytes).expect("write secret fixture");
        }
        values.insert(
            "AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH".to_owned(),
            private_key.to_string_lossy().into_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH".to_owned(),
            webhook_secret.to_string_lossy().into_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_GITHUB_WEBHOOK_PREVIOUS_SECRET_PATH".to_owned(),
            previous_webhook_secret.to_string_lossy().into_owned(),
        );
        let config = AppConfig::from_values(&values).expect("complete GitHub config");
        let debug = format!("{config:?}");
        assert!(
            config
                .github
                .as_ref()
                .and_then(|github| github.webhook_previous_secret_path.as_ref())
                .is_some()
        );
        assert!(!debug.contains(private_key.to_string_lossy().as_ref()));
        assert!(!debug.contains(webhook_secret.to_string_lossy().as_ref()));
        assert!(!debug.contains(previous_webhook_secret.to_string_lossy().as_ref()));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn github_secret_files_reject_symlinks_and_group_or_world_access() {
        let directory = tempfile::tempdir().expect("secret directory");
        let private_key = directory.path().join("github-app.pem");
        let webhook_secret = directory.path().join("github-webhook-secret");
        std::fs::write(&private_key, "private-key-placeholder").expect("private key");
        std::fs::write(&webhook_secret, "webhook-secret-placeholder").expect("webhook secret");
        std::fs::set_permissions(&private_key, std::fs::Permissions::from_mode(0o640))
            .expect("set unsafe mode");
        std::fs::set_permissions(&webhook_secret, std::fs::Permissions::from_mode(0o600))
            .expect("set safe mode");

        let mut values = base_values();
        values.extend([
            (
                "AGENT_WORKSPACE_GITHUB_APP_ID".to_owned(),
                "4442593".to_owned(),
            ),
            (
                "AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH".to_owned(),
                private_key.to_string_lossy().into_owned(),
            ),
            (
                "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH".to_owned(),
                webhook_secret.to_string_lossy().into_owned(),
            ),
        ]);
        let error = AppConfig::from_values(&values).expect_err("unsafe mode must fail");
        assert!(error.to_string().contains("owner-only"));

        std::fs::set_permissions(&private_key, std::fs::Permissions::from_mode(0o600))
            .expect("set safe mode");
        let symlink = directory.path().join("webhook-secret-link");
        std::os::unix::fs::symlink(&webhook_secret, &symlink).expect("secret symlink");
        values.insert(
            "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH".to_owned(),
            symlink.to_string_lossy().into_owned(),
        );
        let error = AppConfig::from_values(&values).expect_err("symlink must fail");
        assert!(error.to_string().contains("regular owner-only file"));

        values.insert(
            "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH".to_owned(),
            webhook_secret.to_string_lossy().into_owned(),
        );
        values.insert(
            "AGENT_WORKSPACE_GITHUB_WEBHOOK_PREVIOUS_SECRET_PATH".to_owned(),
            webhook_secret.to_string_lossy().into_owned(),
        );
        let error = AppConfig::from_values(&values).expect_err("overlap paths must differ");
        assert!(error.to_string().contains("must be distinct"));

        let oversized = directory.path().join("oversized-webhook-secret");
        std::fs::write(&oversized, vec![b'x'; 4097]).expect("oversized secret");
        std::fs::set_permissions(&oversized, std::fs::Permissions::from_mode(0o600))
            .expect("owner-only oversized secret");
        values.insert(
            "AGENT_WORKSPACE_GITHUB_WEBHOOK_PREVIOUS_SECRET_PATH".to_owned(),
            oversized.to_string_lossy().into_owned(),
        );
        let error = AppConfig::from_values(&values).expect_err("oversized secret must fail");
        assert!(error.to_string().contains("between 1 and 4096 bytes"));
    }

    #[test]
    fn webhook_trusted_origins_are_exact_origins() {
        let mut values = base_values();
        values.insert(
            "AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS".to_owned(),
            " http://100.119.56.3:8732 , https://hooks.internal:443/ ".to_owned(),
        );
        let config = AppConfig::from_values(&values).expect("config");
        assert_eq!(
            config.webhook_trusted_origins,
            ["http://100.119.56.3:8732", "https://hooks.internal"]
        );
        for invalid in [
            "ftp://100.119.56.3",
            "http://user:secret@100.119.56.3:8732",
            "http://100.119.56.3:8732/wake",
            "100.119.56.3:8732",
        ] {
            values.insert(
                "AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS".to_owned(),
                invalid.to_owned(),
            );
            assert!(
                AppConfig::from_values(&values).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn workos_api_stand_in_is_development_loopback_only() {
        let base = std::env::temp_dir().join(format!("aw-workos-config-{}", std::process::id()));
        let (data_root, web_root) = (base.join("data"), base.join("web"));
        std::fs::create_dir_all(&base).expect("make directory");
        let key = base.join("workos-api-key");
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&key)
            .and_then(|mut file| file.write_all(b"sk_test"))
            .expect("write key");
        let values = |api_base: Option<&str>| {
            let mut values = BTreeMap::from([
                (
                    "AGENT_WORKSPACE_WORKOS_CLIENT_ID".to_owned(),
                    "client_test".to_owned(),
                ),
                (
                    "AGENT_WORKSPACE_WORKOS_API_KEY_PATH".to_owned(),
                    key.to_string_lossy().into_owned(),
                ),
            ]);
            if let Some(api_base) = api_base {
                values.insert(
                    "AGENT_WORKSPACE_WORKOS_API_BASE".to_owned(),
                    api_base.to_owned(),
                );
            }
            values
        };
        let api_base = |environment, api_base| {
            workos_config(&values(api_base), environment, &data_root, &web_root)
                .map(|config| config.expect("configured").api_base)
        };
        assert_eq!(
            api_base(Environment::Production, None).expect("default"),
            WORKOS_API_BASE
        );
        assert_eq!(
            api_base(Environment::Development, Some("http://localhost:3192")).expect("stand-in"),
            "http://localhost:3192"
        );
        for (environment, value) in [
            (Environment::Production, "http://localhost:3192"),
            (Environment::Development, "https://elsewhere.example"),
            (Environment::Development, "http://10.0.0.5:3192"),
            (Environment::Development, "http://localhost:3192/path"),
        ] {
            assert!(
                api_base(environment, Some(value)).is_err(),
                "accepted {value}"
            );
        }
        std::fs::remove_dir_all(&base).expect("clean up");
    }
}

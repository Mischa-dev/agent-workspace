use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
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
    pub database_url: SecretString,
    pub database_max_connections: u32,
    pub data_root: PathBuf,
    pub web_root: PathBuf,
    pub codex_binary: PathBuf,
    pub codex_home_root: PathBuf,
    pub codex_host_concurrency_limit: usize,
    pub codex_max_line_bytes: usize,
    pub codex_max_stderr_bytes: usize,
    pub codex_max_buffered_events: usize,
    pub codex_request_timeout: Duration,
    pub codex_shutdown_timeout: Duration,
    pub demo_executor: bool,
    pub github: Option<GitHubConfig>,
    pub log_filter: String,
}

#[derive(Clone)]
pub struct GitHubConfig {
    pub app_id: u64,
    pub private_key_path: PathBuf,
    pub webhook_secret_path: PathBuf,
    pub webhook_previous_secret_path: Option<PathBuf>,
    pub api_base_url: String,
    pub api_version: String,
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
        let environment = optional(values, "ENV", "development").parse::<Environment>()?;
        let server_addr = parse(values, "SERVER_ADDR", "127.0.0.1:3100")?;
        let webhook_addr = parse(values, "WEBHOOK_ADDR", "127.0.0.1:3101")?;
        let database_url = required(values, "DATABASE_URL")?;
        let database_max_connections = parse(values, "DATABASE_MAX_CONNECTIONS", "5")?;
        let data_root = PathBuf::from(optional(values, "DATA_ROOT", ".agent-workspace"));
        let web_root = PathBuf::from(optional(values, "WEB_ROOT", "apps/web/dist"));
        let codex_binary = PathBuf::from(optional(values, "CODEX_BINARY", "codex"));
        let codex_home_root = PathBuf::from(optional(
            values,
            "CODEX_HOME_ROOT",
            ".agent-workspace/codex/profiles",
        ));
        let codex_host_concurrency_limit = parse(values, "CODEX_HOST_CONCURRENCY_LIMIT", "1")?;
        let codex_max_line_bytes = parse(values, "CODEX_MAX_LINE_BYTES", "1048576")?;
        let codex_max_stderr_bytes = parse(values, "CODEX_MAX_STDERR_BYTES", "262144")?;
        let codex_max_buffered_events = parse(values, "CODEX_MAX_BUFFERED_EVENTS", "64")?;
        let codex_request_timeout =
            Duration::from_secs(parse(values, "CODEX_REQUEST_TIMEOUT_SECONDS", "120")?);
        let codex_shutdown_timeout =
            Duration::from_secs(parse(values, "CODEX_SHUTDOWN_TIMEOUT_SECONDS", "5")?);
        let demo_executor = parse(values, "DEMO_EXECUTOR", "false")?;
        let github = github_config(values, &data_root, &web_root)?;
        let log_filter = optional(values, "LOG_FILTER", "agent_workspace=info,tower_http=info");
        let allowed_origins = allowed_origins(values, environment)?;

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
        if demo_executor && !environment.is_development() {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}DEMO_EXECUTOR"),
                reason: "the demo executor is development-only".into(),
            });
        }

        Ok(Self {
            environment,
            server_addr,
            webhook_addr,
            allowed_origins,
            database_url: SecretString::new(database_url),
            database_max_connections,
            data_root,
            web_root,
            codex_binary,
            codex_home_root,
            codex_host_concurrency_limit,
            codex_max_line_bytes,
            codex_max_stderr_bytes,
            codex_max_buffered_events,
            codex_request_timeout,
            codex_shutdown_timeout,
            demo_executor,
            github,
            log_filter,
        })
    }
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

    Ok(Some(GitHubConfig {
        app_id,
        private_key_path,
        webhook_secret_path,
        webhook_previous_secret_path,
        api_base_url,
        api_version,
    }))
}

fn validate_secret_file(
    key: &str,
    path: &Path,
    data_root: &Path,
    web_root: &Path,
    maximum_bytes: u64,
) -> Result<(), ConfigError> {
    if !path.is_absolute() || path.starts_with(data_root) || path.starts_with(web_root) {
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

fn allowed_origins(
    values: &BTreeMap<String, String>,
    environment: Environment,
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
        return Err(ConfigError::Missing(format!("{PREFIX}ALLOWED_ORIGINS")));
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
}

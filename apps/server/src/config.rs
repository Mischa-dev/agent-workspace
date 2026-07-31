use std::{collections::BTreeMap, fmt, net::SocketAddr, path::PathBuf, str::FromStr};

use thiserror::Error;

const PREFIX: &str = "AGENT_WORKSPACE_";

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
    pub database_url: SecretString,
    pub database_max_connections: u32,
    pub data_root: PathBuf,
    pub codex_binary: PathBuf,
    pub codex_home_root: PathBuf,
    pub log_filter: String,
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
        let codex_binary = PathBuf::from(optional(values, "CODEX_BINARY", "codex"));
        let codex_home_root = PathBuf::from(optional(
            values,
            "CODEX_HOME_ROOT",
            ".agent-workspace/codex/profiles",
        ));
        let log_filter = optional(values, "LOG_FILTER", "agent_workspace=info,tower_http=info");

        validate_loopback("SERVER_ADDR", server_addr, environment)?;
        validate_loopback("WEBHOOK_ADDR", webhook_addr, environment)?;

        if database_max_connections == 0 {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}DATABASE_MAX_CONNECTIONS"),
                reason: "must be greater than zero".into(),
            });
        }

        if data_root.as_os_str().is_empty() || codex_home_root.as_os_str().is_empty() {
            return Err(ConfigError::Invalid {
                key: format!("{PREFIX}DATA_ROOT"),
                reason: "runtime paths cannot be empty".into(),
            });
        }

        Ok(Self {
            environment,
            server_addr,
            webhook_addr,
            database_url: SecretString::new(database_url),
            database_max_connections,
            data_root,
            codex_binary,
            codex_home_root,
            log_filter,
        })
    }
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
            "AGENT_WORKSPACE_SERVER_ADDR".to_owned(),
            "0.0.0.0:3100".to_owned(),
        );

        let config = AppConfig::from_values(&values).expect("valid production config");
        assert_eq!(
            config.server_addr.ip(),
            std::net::IpAddr::from([0, 0, 0, 0])
        );
    }
}

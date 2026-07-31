use std::{
    collections::{HashSet, VecDeque},
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tempfile::TempDir;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const API_VERSION: &str = "2026-03-10";
pub const WEBHOOK_PORT: u16 = 3101;
const MAX_ERROR_BYTES: usize = 8 * 1024;

#[derive(Clone)]
pub struct GitHubAppClient {
    http: reqwest::Client,
    api_root: String,
    app_id: u64,
    installation_id: u64,
    private_key_pem: Arc<Zeroizing<Vec<u8>>>,
}

#[derive(Debug, Serialize)]
struct AppClaims {
    iat: u64,
    exp: u64,
    iss: String,
}

#[derive(Debug, Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: String,
}

pub struct InstallationToken {
    secret: SecretString,
    pub expires_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub html_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebhookConfig {
    pub url: String,
    pub content_type: String,
    pub insecure_ssl: String,
}

#[derive(Debug, Deserialize)]
struct CheckRunsResponse {
    check_runs: Vec<CheckRun>,
}

impl InstallationToken {
    pub fn expose(&self) -> &str {
        self.secret.expose_secret()
    }
}

impl std::fmt::Debug for InstallationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstallationToken")
            .field("secret", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl GitHubAppClient {
    pub fn new_app(app_id: u64, private_key_pem: Vec<u8>) -> Result<Self, GitHubSpikeError> {
        Self::build(app_id, 0, private_key_pem)
    }

    pub fn new(
        app_id: u64,
        installation_id: u64,
        private_key_pem: Vec<u8>,
    ) -> Result<Self, GitHubSpikeError> {
        if installation_id == 0 {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        Self::build(app_id, installation_id, private_key_pem)
    }

    fn build(
        app_id: u64,
        installation_id: u64,
        private_key_pem: Vec<u8>,
    ) -> Result<Self, GitHubSpikeError> {
        if app_id == 0 || private_key_pem.is_empty() {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .https_only(true)
                .build()
                .map_err(GitHubSpikeError::Http)?,
            api_root: "https://api.github.com".to_owned(),
            app_id,
            installation_id,
            private_key_pem: Arc::new(Zeroizing::new(private_key_pem)),
        })
    }

    pub fn app_jwt(&self) -> Result<SecretString, GitHubSpikeError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| GitHubSpikeError::Clock)?
            .as_secs();
        let claims = AppClaims {
            iat: now.saturating_sub(60),
            exp: now + 9 * 60,
            iss: self.app_id.to_string(),
        };
        let key = EncodingKey::from_rsa_pem(self.private_key_pem.as_slice())
            .map_err(GitHubSpikeError::Jwt)?;
        let jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map_err(GitHubSpikeError::Jwt)?;
        Ok(SecretString::from(jwt))
    }

    pub async fn installation_token(
        &self,
        repository: &str,
    ) -> Result<InstallationToken, GitHubSpikeError> {
        if self.installation_id == 0 {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        validate_repository_name(repository)?;
        let jwt = self.app_jwt()?;
        let url = format!(
            "{}/app/installations/{}/access_tokens",
            self.api_root, self.installation_id
        );
        let response = self
            .http
            .post(url)
            .headers(github_headers(jwt.expose_secret())?)
            .json(&json!({
                "repositories": [repository],
                "permissions": {
                    "contents": "write",
                    "pull_requests": "write",
                    "checks": "read",
                    "metadata": "read"
                }
            }))
            .send()
            .await
            .map_err(GitHubSpikeError::Http)?;
        let status = response.status();
        if !status.is_success() {
            return Err(GitHubSpikeError::Api {
                status: status.as_u16(),
            });
        }
        let response: InstallationTokenResponse =
            response.json().await.map_err(GitHubSpikeError::Http)?;
        Ok(InstallationToken {
            secret: SecretString::from(response.token),
            expires_at: response.expires_at,
        })
    }

    pub async fn update_webhook_config(
        &self,
        url: &str,
        secret: &str,
    ) -> Result<WebhookConfig, GitHubSpikeError> {
        if !url.starts_with("https://") || url.len() > 2_048 || secret.len() < 32 {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        let jwt = self.app_jwt()?;
        let response = self
            .http
            .patch(format!("{}/app/hook/config", self.api_root))
            .headers(github_headers(jwt.expose_secret())?)
            .json(&json!({
                "url": url,
                "content_type": "json",
                "insecure_ssl": "0",
                "secret": secret,
            }))
            .send()
            .await
            .map_err(GitHubSpikeError::Http)?;
        decode_api_response(response).await
    }

    pub async fn create_pull_request(
        &self,
        owner: &str,
        repository: &str,
        title: &str,
        head: &str,
        base: &str,
        token: &InstallationToken,
    ) -> Result<PullRequest, GitHubSpikeError> {
        validate_owner(owner)?;
        validate_repository_name(repository)?;
        validate_branch(head)?;
        validate_name(base)?;
        let response = self
            .http
            .post(format!(
                "{}/repos/{owner}/{repository}/pulls",
                self.api_root
            ))
            .headers(github_headers(token.expose())?)
            .json(&json!({"title": title, "head": head, "base": base}))
            .send()
            .await
            .map_err(GitHubSpikeError::Http)?;
        decode_api_response(response).await
    }

    pub async fn check_runs(
        &self,
        owner: &str,
        repository: &str,
        git_ref: &str,
        token: &InstallationToken,
    ) -> Result<Vec<CheckRun>, GitHubSpikeError> {
        validate_owner(owner)?;
        validate_repository_name(repository)?;
        validate_ref(git_ref)?;
        let response = self
            .http
            .get(format!(
                "{}/repos/{owner}/{repository}/commits/{}/check-runs",
                self.api_root,
                urlencoding::encode(git_ref)
            ))
            .headers(github_headers(token.expose())?)
            .send()
            .await
            .map_err(GitHubSpikeError::Http)?;
        let response: CheckRunsResponse = decode_api_response(response).await?;
        Ok(response.check_runs)
    }

    pub async fn close_pull_request(
        &self,
        owner: &str,
        repository: &str,
        number: u64,
        token: &InstallationToken,
    ) -> Result<PullRequest, GitHubSpikeError> {
        validate_owner(owner)?;
        validate_repository_name(repository)?;
        if number == 0 {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        let response = self
            .http
            .patch(format!(
                "{}/repos/{owner}/{repository}/pulls/{number}",
                self.api_root
            ))
            .headers(github_headers(token.expose())?)
            .json(&json!({"state": "closed"}))
            .send()
            .await
            .map_err(GitHubSpikeError::Http)?;
        decode_api_response(response).await
    }

    pub async fn delete_branch(
        &self,
        owner: &str,
        repository: &str,
        branch: &str,
        token: &InstallationToken,
    ) -> Result<(), GitHubSpikeError> {
        validate_owner(owner)?;
        validate_repository_name(repository)?;
        validate_branch(branch)?;
        let response = self
            .http
            .delete(format!(
                "{}/repos/{owner}/{repository}/git/refs/heads/{}",
                self.api_root,
                urlencoding::encode(branch)
            ))
            .headers(github_headers(token.expose())?)
            .send()
            .await
            .map_err(GitHubSpikeError::Http)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(GitHubSpikeError::Api {
                status: response.status().as_u16(),
            })
        }
    }
}

async fn decode_api_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, GitHubSpikeError> {
    let status = response.status();
    if !status.is_success() {
        return Err(GitHubSpikeError::Api {
            status: status.as_u16(),
        });
    }
    response.json().await.map_err(GitHubSpikeError::Http)
}

fn github_headers(bearer: &str) -> Result<HeaderMap, GitHubSpikeError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(USER_AGENT, HeaderValue::from_static("agent-workspace-m0"));
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static(API_VERSION),
    );
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {bearer}"))
            .map_err(|_| GitHubSpikeError::InvalidConfig)?,
    );
    Ok(headers)
}

pub struct CredentialedGit {
    git_binary: PathBuf,
    helper_directory: TempDir,
    helper_path: PathBuf,
}

impl CredentialedGit {
    pub fn new(git_binary: PathBuf) -> Result<Self, GitHubSpikeError> {
        if !git_binary.is_absolute() {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        let helper_directory = tempfile::Builder::new()
            .prefix("agent-workspace-askpass-")
            .tempdir()
            .map_err(GitHubSpikeError::Io)?;
        let helper_path = helper_directory.path().join("askpass");
        let mut helper = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o700)
            .open(&helper_path)
            .map_err(GitHubSpikeError::Io)?;
        helper
            .write_all(
                br#"#!/bin/sh
case "$1" in
  *Username*) printf '%s\n' 'x-access-token' ;;
  *Password*) printf '%s\n' "$AGENT_WORKSPACE_GIT_TOKEN" ;;
  *) exit 1 ;;
esac
"#,
            )
            .map_err(GitHubSpikeError::Io)?;
        Ok(Self {
            git_binary,
            helper_directory,
            helper_path,
        })
    }

    pub fn clone_repository(
        &self,
        owner: &str,
        repository: &str,
        destination: &Path,
        token: &InstallationToken,
    ) -> Result<(), GitHubSpikeError> {
        validate_owner(owner)?;
        validate_repository_name(repository)?;
        if !destination.is_absolute() || destination.exists() {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        self.run(
            [
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "credential.helper=",
                "-c",
                "credential.useHttpPath=true",
                "clone",
                "--origin",
                "origin",
                &format!("https://github.com/{owner}/{repository}.git"),
                &destination.to_string_lossy(),
            ],
            token,
        )?;
        Ok(())
    }

    pub fn push_branch(
        &self,
        repository_path: &Path,
        branch: &str,
        token: &InstallationToken,
    ) -> Result<(), GitHubSpikeError> {
        validate_branch(branch)?;
        self.run(
            [
                "-C",
                &repository_path.to_string_lossy(),
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "credential.helper=",
                "-c",
                "credential.useHttpPath=true",
                "push",
                "origin",
                &format!("HEAD:refs/heads/{branch}"),
            ],
            token,
        )?;
        Ok(())
    }

    pub fn helper_directory(&self) -> &Path {
        self.helper_directory.path()
    }

    fn run<const N: usize>(
        &self,
        arguments: [&str; N],
        token: &InstallationToken,
    ) -> Result<Output, GitHubSpikeError> {
        let output = Command::new(&self.git_binary)
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("HOME", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", &self.helper_path)
            .env("AGENT_WORKSPACE_GIT_TOKEN", token.expose())
            .output()
            .map_err(GitHubSpikeError::Io)?;
        if output.status.success() {
            return Ok(output);
        }
        Err(GitHubSpikeError::Git {
            stderr: redact_and_bound(&output.stderr, token.expose()),
        })
    }
}

#[derive(Clone)]
pub struct WebhookVerifier {
    secret_digest_key: Arc<Zeroizing<Vec<u8>>>,
    expected_repository: Arc<str>,
    deliveries: Arc<Mutex<DeliveryCache>>,
}

struct DeliveryCache {
    seen: HashSet<String>,
    order: VecDeque<String>,
    capacity: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedWebhook {
    pub delivery_id: String,
    pub event: String,
    pub duplicate: bool,
}

impl WebhookVerifier {
    pub fn new(
        secret: Vec<u8>,
        expected_repository: String,
        capacity: usize,
    ) -> Result<Self, GitHubSpikeError> {
        if secret.len() < 32 || capacity == 0 || !expected_repository.contains('/') {
            return Err(GitHubSpikeError::InvalidConfig);
        }
        Ok(Self {
            secret_digest_key: Arc::new(Zeroizing::new(secret)),
            expected_repository: Arc::from(expected_repository),
            deliveries: Arc::new(Mutex::new(DeliveryCache {
                seen: HashSet::new(),
                order: VecDeque::new(),
                capacity,
            })),
        })
    }

    pub fn verify(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<VerifiedWebhook, GitHubSpikeError> {
        let delivery_id = required_header(headers, "x-github-delivery")?;
        let event = required_header(headers, "x-github-event")?;
        if !matches!(
            event.as_str(),
            "installation"
                | "installation_repositories"
                | "push"
                | "pull_request"
                | "pull_request_review"
                | "check_suite"
                | "check_run"
        ) {
            return Err(GitHubSpikeError::UnexpectedEvent(event));
        }
        let signature = required_header(headers, "x-hub-signature-256")?;
        verify_signature(self.secret_digest_key.as_slice(), body, &signature)?;

        let payload: Value = serde_json::from_slice(body).map_err(GitHubSpikeError::Json)?;
        let repository = payload
            .pointer("/repository/full_name")
            .and_then(Value::as_str)
            .ok_or(GitHubSpikeError::MissingRepository)?;
        if repository != self.expected_repository.as_ref() {
            return Err(GitHubSpikeError::WrongRepository(repository.to_owned()));
        }

        let mut deliveries = self
            .deliveries
            .lock()
            .map_err(|_| GitHubSpikeError::LockPoisoned)?;
        let duplicate = deliveries.seen.contains(&delivery_id);
        if !duplicate {
            deliveries.seen.insert(delivery_id.clone());
            deliveries.order.push_back(delivery_id.clone());
            if deliveries.order.len() > deliveries.capacity
                && let Some(expired) = deliveries.order.pop_front()
            {
                deliveries.seen.remove(&expired);
            }
        }
        Ok(VerifiedWebhook {
            delivery_id,
            event,
            duplicate,
        })
    }
}

pub fn unique_test_branch() -> String {
    format!("codex/m0-github-spike-{}", Uuid::now_v7())
}

pub fn verify_signature(
    secret: &[u8],
    body: &[u8],
    signature_header: &str,
) -> Result<(), GitHubSpikeError> {
    let supplied = signature_header
        .strip_prefix("sha256=")
        .ok_or(GitHubSpikeError::BadSignature)?;
    let supplied = hex::decode(supplied).map_err(|_| GitHubSpikeError::BadSignature)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret).map_err(|_| GitHubSpikeError::BadSignature)?;
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    if supplied.len() != expected.len() || !bool::from(supplied.ct_eq(expected.as_slice())) {
        return Err(GitHubSpikeError::BadSignature);
    }
    Ok(())
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Result<String, GitHubSpikeError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(GitHubSpikeError::MissingHeader(name))
}

fn validate_owner(value: &str) -> Result<(), GitHubSpikeError> {
    validate_name(value)
}

fn validate_repository_name(value: &str) -> Result<(), GitHubSpikeError> {
    validate_name(value)
}

fn validate_name(value: &str) -> Result<(), GitHubSpikeError> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || value.starts_with('.')
    {
        return Err(GitHubSpikeError::InvalidConfig);
    }
    Ok(())
}

fn validate_branch(value: &str) -> Result<(), GitHubSpikeError> {
    if !value.starts_with("codex/m0-github-spike-")
        || value.starts_with('-')
        || value.contains("..")
        || value.contains("@{")
        || value.contains('\\')
        || value.chars().any(char::is_whitespace)
    {
        return Err(GitHubSpikeError::InvalidConfig);
    }
    Ok(())
}

fn validate_ref(value: &str) -> Result<(), GitHubSpikeError> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.contains("..")
        || value.contains("@{")
        || value.contains('\\')
        || value.chars().any(char::is_whitespace)
    {
        return Err(GitHubSpikeError::InvalidConfig);
    }
    Ok(())
}

fn redact_and_bound(bytes: &[u8], token: &str) -> String {
    let bounded = &bytes[..bytes.len().min(MAX_ERROR_BYTES)];
    String::from_utf8_lossy(bounded).replace(token, "[REDACTED]")
}

#[derive(Debug, Error)]
pub enum GitHubSpikeError {
    #[error("invalid GitHub spike configuration")]
    InvalidConfig,
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("JWT generation failed")]
    Jwt(#[source] jsonwebtoken::errors::Error),
    #[error("GitHub HTTP request failed")]
    Http(#[source] reqwest::Error),
    #[error("GitHub API returned HTTP {status}")]
    Api { status: u16 },
    #[error("credentialed Git operation failed: {stderr}")]
    Git { stderr: String },
    #[error("filesystem operation failed")]
    Io(#[source] std::io::Error),
    #[error("missing webhook header {0}")]
    MissingHeader(&'static str),
    #[error("webhook signature is invalid")]
    BadSignature,
    #[error("webhook event is not subscribed: {0}")]
    UnexpectedEvent(String),
    #[error("webhook payload omitted repository.full_name")]
    MissingRepository,
    #[error("webhook repository identity is wrong: {0}")]
    WrongRepository(String),
    #[error("webhook payload is not valid JSON")]
    Json(#[source] serde_json::Error),
    #[error("webhook delivery cache lock was poisoned")]
    LockPoisoned,
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    use super::{GitHubSpikeError, InstallationToken, WebhookVerifier, redact_and_bound};

    fn signed_headers(secret: &[u8], body: &[u8]) -> HeaderMap {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("valid HMAC key");
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_str(&signature).expect("signature header"),
        );
        headers.insert("x-github-delivery", HeaderValue::from_static("delivery-1"));
        headers.insert("x-github-event", HeaderValue::from_static("push"));
        headers
    }

    #[test]
    fn verifies_signature_repository_and_duplicate_delivery() {
        let secret = b"a-high-entropy-development-webhook-secret";
        let body = br#"{"repository":{"full_name":"Mischa-dev/agent-workspace-m0-spike"}}"#;
        let verifier = WebhookVerifier::new(
            secret.to_vec(),
            "Mischa-dev/agent-workspace-m0-spike".to_owned(),
            10,
        )
        .expect("valid verifier");
        let headers = signed_headers(secret, body);

        let first = verifier.verify(&headers, body).expect("first delivery");
        let second = verifier.verify(&headers, body).expect("duplicate delivery");
        assert!(!first.duplicate);
        assert!(second.duplicate);
    }

    #[test]
    fn rejects_bad_signature_and_wrong_repository() {
        let secret = b"a-high-entropy-development-webhook-secret";
        let verifier = WebhookVerifier::new(
            secret.to_vec(),
            "Mischa-dev/agent-workspace-m0-spike".to_owned(),
            10,
        )
        .expect("valid verifier");
        let wrong_body = br#"{"repository":{"full_name":"other/repository"}}"#;
        let headers = signed_headers(secret, wrong_body);
        assert!(matches!(
            verifier.verify(&headers, wrong_body),
            Err(GitHubSpikeError::WrongRepository(_))
        ));

        let mut bad_headers = headers;
        bad_headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_static(
                "sha256=0000000000000000000000000000000000000000000000000000000000000000",
            ),
        );
        assert!(matches!(
            verifier.verify(&bad_headers, wrong_body),
            Err(GitHubSpikeError::BadSignature)
        ));
    }

    #[test]
    fn token_debug_and_git_errors_are_redacted() {
        let token = InstallationToken {
            secret: secrecy::SecretString::from("opaque-token-value"),
            expires_at: "future".to_owned(),
        };
        assert!(!format!("{token:?}").contains(token.expose()));
        assert_eq!(
            redact_and_bound(b"failure opaque-token-value", token.expose()),
            "failure [REDACTED]"
        );
    }

    #[test]
    fn live_evidence_fixture_is_redacted_and_bounded() {
        let fixture = include_str!("../fixtures/live-evidence.json");
        let parsed: serde_json::Value =
            serde_json::from_str(fixture).expect("valid live evidence fixture");
        assert_eq!(parsed["credentials_redacted"], true);
        assert_eq!(parsed["webhook_proof"]["temporary_url_redacted"], true);
        assert!(!fixture.contains("trycloudflare.com"));
        assert!(!fixture.contains("BEGIN RSA PRIVATE KEY"));
        assert!(!fixture.contains("BEGIN PRIVATE KEY"));
        assert!(fixture.len() < 8 * 1024);
    }
}

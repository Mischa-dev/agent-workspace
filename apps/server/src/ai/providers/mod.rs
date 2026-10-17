//! Model providers the gateway can call. Each adapter declares a manifest
//! (its name, whether it needs a key or an address, its default models and
//! their prices) and turns one structured-output request into its own HTTP
//! call. Provider names and JSON live only in these files; the settings
//! screen is drawn from the manifests.
//!
//! To add a provider, add a module with a `manifest()` and a `complete()`,
//! and a line to [`ProviderId`].

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::{provider::manifest::ModelEntry, secrets::SecretValue};

mod anthropic;
pub mod fake;
mod google;
mod openai;

/// What a provider offers, for the settings screen and the gateway.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderManifest {
    /// Stored in the `ai.provider` setting.
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Whether calls need an API key (stored as the secret [`secret_name`]).
    pub needs_key: bool,
    /// Whether the workspace sets the provider's address (`ai.base_url`).
    pub needs_base_url: bool,
    /// The address used when `ai.base_url` is unset.
    pub default_base_url: Option<&'static str>,
    /// Used for quick jobs (dependency checks, triage) unless `ai.model` says.
    pub default_quick_model: &'static str,
    /// Used for deep jobs (plan to tasks) unless `ai.deep_model` says.
    pub default_deep_model: &'static str,
    /// Models to offer, with prices where known. Any other model id works too.
    pub models: Vec<ModelEntry>,
    /// Only offered outside production: answers from rules, for tests.
    pub testing_only: bool,
}

/// The secret that holds a provider's key, at the project (an override) or
/// the workspace.
pub fn secret_name(provider: &str) -> String {
    format!("ai_key.{provider}")
}

/// The providers this server knows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderId {
    Anthropic,
    OpenAi,
    OpenAiCompatible,
    Google,
    Fake,
}

impl ProviderId {
    pub const ALL: [ProviderId; 5] = [
        ProviderId::Anthropic,
        ProviderId::OpenAi,
        ProviderId::OpenAiCompatible,
        ProviderId::Google,
        ProviderId::Fake,
    ];

    pub fn manifest(self) -> ProviderManifest {
        match self {
            Self::Anthropic => anthropic::manifest(),
            Self::OpenAi => openai::manifest(),
            Self::OpenAiCompatible => openai::compatible_manifest(),
            Self::Google => google::manifest(),
            Self::Fake => fake::manifest(),
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.manifest().id == id)
    }

    pub async fn complete(
        self,
        client: &reqwest::Client,
        request: &CompletionRequest<'_>,
    ) -> Result<Completion, ProviderError> {
        match self {
            Self::Anthropic => anthropic::complete(client, request).await,
            Self::OpenAi | Self::OpenAiCompatible => {
                openai::complete(client, request, self == Self::OpenAi).await
            }
            Self::Google => google::complete(client, request).await,
            Self::Fake => fake::complete(request),
        }
    }
}

/// One call: instructions, the input as JSON text, and the reply's schema.
pub struct CompletionRequest<'a> {
    pub model: &'a str,
    pub system: &'a str,
    /// The job's input, serialized. Never logged.
    pub input: &'a str,
    pub prompt_id: &'a str,
    pub schema: &'a Value,
    pub max_output_tokens: u32,
    pub api_key: Option<&'a SecretValue>,
    pub base_url: Option<&'a str>,
    pub timeout: Duration,
}

/// The reply: its JSON text and the tokens it used.
#[derive(Debug)]
pub struct Completion {
    pub text: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Why a call failed, as a short category safe to log and store. Never a
/// response body: it can echo the prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    /// Rejected the key or the account.
    Unauthorized,
    /// Too many requests or out of quota; try again later.
    RateLimited,
    /// The model declined to answer.
    Refused,
    /// A timeout, a network error or a server error; try again later.
    Unavailable(String),
    /// The request was refused as malformed (a model name it doesn't know,
    /// a schema it can't follow).
    BadRequest(String),
}

impl ProviderError {
    pub fn category(&self) -> String {
        match self {
            Self::Unauthorized => "the provider refused the key".to_owned(),
            Self::RateLimited => "the provider is rate limiting or out of quota".to_owned(),
            Self::Refused => "the model declined to answer".to_owned(),
            Self::Unavailable(detail) => format!("the provider is unavailable ({detail})"),
            Self::BadRequest(detail) => format!("the provider refused the request ({detail})"),
        }
    }

    /// Whether trying the same call later can help.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Unavailable(_))
    }
}

/// Maps an HTTP status and transport error to a [`ProviderError`]. Only the
/// status and the provider's error type (a short code) are kept.
fn status_error(status: reqwest::StatusCode, error_type: Option<&str>) -> ProviderError {
    let code = error_type
        .filter(|code| {
            code.len() <= 60
                && code
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        })
        .map(|code| format!(", {code}"))
        .unwrap_or_default();
    match status.as_u16() {
        401 | 403 => ProviderError::Unauthorized,
        429 => ProviderError::RateLimited,
        400 | 404 | 413 | 422 => {
            ProviderError::BadRequest(format!("HTTP {}{code}", status.as_u16()))
        }
        _ => ProviderError::Unavailable(format!("HTTP {}{code}", status.as_u16())),
    }
}

fn transport_error(error: &reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Unavailable("timed out".to_owned())
    } else if error.is_connect() {
        ProviderError::Unavailable("couldn't connect".to_owned())
    } else {
        ProviderError::Unavailable("network error".to_owned())
    }
}

/// The base URL to call: the configured one when the provider takes one,
/// checked to be http(s) with no credentials, else the default.
fn base_url(request: &CompletionRequest<'_>, default: &str) -> Result<String, ProviderError> {
    let chosen = request.base_url.unwrap_or(default).trim_end_matches('/');
    let parsed = url::Url::parse(chosen)
        .map_err(|_| ProviderError::BadRequest("the provider address isn't a URL".to_owned()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none()
    {
        return Err(ProviderError::BadRequest(
            "the provider address must be http or https, without a password".to_owned(),
        ));
    }
    Ok(chosen.to_owned())
}

/// Reads a JSON reply body, at most 4 MiB.
async fn read_json(response: reqwest::Response) -> Result<Value, ProviderError> {
    const MAX_BODY: usize = 4 * 1024 * 1024;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| transport_error(&error))?;
    if bytes.len() > MAX_BODY {
        return Err(ProviderError::Unavailable("reply too large".to_owned()));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ProviderError::Unavailable("reply wasn't JSON".to_owned()))
}

/// Sends a request and returns the JSON body of a successful reply, or the
/// error with the provider's error type.
async fn send(request: reqwest::RequestBuilder) -> Result<Value, ProviderError> {
    let response = request
        .send()
        .await
        .map_err(|error| transport_error(&error))?;
    let status = response.status();
    let body = read_json(response).await;
    if status.is_success() {
        return body;
    }
    let body = body.unwrap_or(Value::Null);
    let error_type = body
        .pointer("/error/type")
        .or_else(|| body.pointer("/error/code"))
        .or_else(|| body.pointer("/error/status"))
        .and_then(Value::as_str);
    Err(status_error(status, error_type))
}

fn tokens(value: Option<&Value>) -> i64 {
    value.and_then(Value::as_i64).unwrap_or(0).max(0)
}

fn model(id: &str, name: &str, input: f64, output: f64, is_default: bool) -> ModelEntry {
    ModelEntry {
        id: id.to_owned(),
        display_name: name.to_owned(),
        prices: Some(crate::provider::manifest::ModelPrices {
            currency: "USD".to_owned(),
            input_per_million: input,
            output_per_million: output,
            cached_input_per_million: None,
        }),
        is_default,
        ..ModelEntry::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_are_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for provider in ProviderId::ALL {
            let manifest = provider.manifest();
            assert!(ids.insert(manifest.id), "{} twice", manifest.id);
            assert_eq!(ProviderId::parse(manifest.id), Some(provider));
            assert!(
                crate::settings::definition("ai.provider")
                    .expect("ai.provider")
                    .validate(&serde_json::json!(manifest.id))
                    .is_ok()
            );
            crate::secrets::validate_name(&secret_name(manifest.id)).expect("secret name");
            assert!(!manifest.default_quick_model.is_empty());
            assert!(
                manifest.needs_base_url
                    || manifest.default_base_url.is_some()
                    || manifest.testing_only
            );
        }
    }

    #[test]
    fn provider_addresses_are_checked() {
        let schema = Value::Null;
        let request = |base_url| CompletionRequest {
            model: "m",
            system: "",
            input: "{}",
            prompt_id: "ping",
            schema: &schema,
            max_output_tokens: 1,
            api_key: None,
            base_url,
            timeout: Duration::from_secs(1),
        };
        assert_eq!(
            base_url(&request(None), "https://example.test/v1/").expect("default"),
            "https://example.test/v1"
        );
        assert!(base_url(&request(Some("http://localhost:11434/v1")), "x").is_ok());
        for bad in [
            "file:///etc/passwd",
            "https://user:pw@example.test",
            "not a url",
        ] {
            assert!(base_url(&request(Some(bad)), "x").is_err(), "{bad}");
        }
    }

    #[test]
    fn errors_keep_only_a_short_code() {
        let error = status_error(
            reqwest::StatusCode::BAD_REQUEST,
            Some("the prompt was: secret plans"),
        );
        assert_eq!(error, ProviderError::BadRequest("HTTP 400".to_owned()));
        assert_eq!(
            status_error(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                Some("overloaded_error")
            ),
            ProviderError::Unavailable("HTTP 503, overloaded_error".to_owned())
        );
        assert!(status_error(reqwest::StatusCode::TOO_MANY_REQUESTS, None).retryable());
        assert!(!status_error(reqwest::StatusCode::UNAUTHORIZED, None).retryable());
    }
}

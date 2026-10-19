//! AI1: the model gateway, how the server makes short AI calls with any
//! provider.
//!
//! [`run`] resolves which provider and model to use from settings
//! (`ai.provider`, `ai.model` for quick jobs, `ai.deep_model` for deep ones,
//! `ai.base_url`), finds the key (a project's own `ai_key.<provider>` secret
//! first, then the workspace's), checks the workspace's monthly budget, asks
//! for a reply that fits the prompt's JSON Schema, checks it, and retries
//! once with the problem spelled out if it doesn't fit. Every attempt is
//! logged in `ai_usage` with tokens and cost, never the prompt or the reply.

use std::{collections::BTreeMap, sync::OnceLock, time::Duration};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    prompts::Prompt,
    providers::{self, CompletionRequest, ProviderError, ProviderId, ProviderManifest},
    schema,
};
use crate::{
    AppState,
    config::Environment,
    error::AppError,
    provider::manifest::ModelPrices,
    secrets::{self, SecretOwner},
    settings::{self, SettingsContext},
};

/// The most input one call may carry, in characters of JSON. Callers split
/// bigger work into batches.
pub const MAX_INPUT_CHARS: usize = 400_000;

/// Which of the two configured models a job uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Short judgments: dependencies, triage, criteria, summaries.
    Quick,
    /// Long structured work: plan to tasks.
    Deep,
}

impl Tier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Deep => "deep",
        }
    }

    fn timeout(self) -> Duration {
        match self {
            Self::Quick => Duration::from_secs(120),
            Self::Deep => Duration::from_secs(300),
        }
    }
}

/// Whose call it is: the workspace pays and its settings apply; the project
/// may override them; the person, if any, is recorded.
#[derive(Clone, Copy, Debug)]
pub struct Scope {
    pub organization_id: Uuid,
    pub project_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
}

/// Why a call couldn't be made or didn't give a usable answer. The messages
/// are for people; none carries a provider's response body.
#[derive(Debug)]
pub enum AiError {
    /// Settings are missing something; the message says what.
    NotConfigured(String),
    /// This month's spend has reached the workspace's budget.
    OverBudget {
        budget_usd: i64,
    },
    Provider(ProviderError),
    /// The model's reply didn't fit the schema, even after one retry.
    InvalidOutput,
    App(AppError),
}

impl From<AppError> for AiError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for AiError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

impl AiError {
    /// A sentence for the person, safe to store.
    pub fn message(&self) -> String {
        match self {
            Self::NotConfigured(message) => message.clone(),
            Self::OverBudget { budget_usd } => format!(
                "This workspace has spent this month's budget for suggestions and summaries (${budget_usd}). An admin can raise it in Runs and models settings."
            ),
            Self::Provider(error) => format!("The AI call failed: {}.", error.category()),
            Self::InvalidOutput => {
                "The model's answer didn't have the expected shape, twice. Try again, or pick another model.".to_owned()
            }
            Self::App(_) => "Something went wrong on the server. Try again.".to_owned(),
        }
    }

    /// Whether a background job should try again later.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Provider(error) => error.retryable(),
            Self::App(_) => true,
            _ => false,
        }
    }

    pub fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::NotConfigured(_) => (StatusCode::CONFLICT, "ai_not_configured"),
            Self::OverBudget { .. } => (StatusCode::CONFLICT, "ai_budget_reached"),
            Self::Provider(ProviderError::RateLimited) => {
                (StatusCode::TOO_MANY_REQUESTS, "ai_rate_limited")
            }
            Self::Provider(_) => (StatusCode::BAD_GATEWAY, "ai_provider_failed"),
            Self::InvalidOutput => (StatusCode::BAD_GATEWAY, "ai_invalid_output"),
            Self::App(_) => {
                let Self::App(error) = self else {
                    unreachable!()
                };
                return error.into_response();
            }
        };
        (
            status,
            Json(json!({"error": {"code": code, "message": self.message(), "details": null}})),
        )
            .into_response()
    }
}

/// What a call resolved to.
#[derive(Clone, Debug)]
pub struct ResolvedModel {
    pub provider: ProviderId,
    pub manifest: ProviderManifest,
    pub model: String,
    pub base_url: Option<String>,
    /// The secret that holds the key, and whether it's the project's own.
    pub key: Option<(Uuid, KeySource)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    Project,
    Workspace,
}

/// The providers people may choose on this server.
pub fn offered_providers(environment: Environment) -> Vec<ProviderManifest> {
    ProviderId::ALL
        .into_iter()
        .map(ProviderId::manifest)
        .filter(|manifest| !manifest.testing_only || environment != Environment::Production)
        .collect()
}

/// Works out the provider, model, address and key for `scope`, or says
/// what's missing.
pub async fn resolve(state: &AppState, scope: Scope, tier: Tier) -> Result<ResolvedModel, AiError> {
    let mut connection = state.pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        SettingsContext {
            organization_id: scope.organization_id,
            project_id: scope.project_id,
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    drop(connection);
    let Some(provider_id) = effective.string("ai.provider") else {
        return Err(AiError::NotConfigured(
            "Choose a provider for suggestions and summaries in Runs and models settings first."
                .to_owned(),
        ));
    };
    let provider = ProviderId::parse(&provider_id).ok_or_else(|| {
        AiError::NotConfigured(format!(
            "The provider \"{provider_id}\" isn't one this server knows. Choose another in Runs and models settings."
        ))
    })?;
    let manifest = provider.manifest();
    if manifest.testing_only && state.config.environment == Environment::Production {
        return Err(AiError::NotConfigured(
            "The test provider isn't available on this server. Choose a real provider in Runs and models settings.".to_owned(),
        ));
    }
    let model = match tier {
        Tier::Quick => effective.string("ai.model"),
        Tier::Deep => effective
            .string("ai.deep_model")
            .or_else(|| effective.string("ai.model")),
    }
    .unwrap_or_else(|| {
        match tier {
            Tier::Quick => manifest.default_quick_model,
            Tier::Deep => manifest.default_deep_model,
        }
        .to_owned()
    });
    let base_url = effective
        .string("ai.base_url")
        .filter(|address| !address.trim().is_empty());
    if manifest.needs_base_url && base_url.is_none() {
        return Err(AiError::NotConfigured(format!(
            "Set the address of your {} in Runs and models settings.",
            manifest.display_name
        )));
    }
    let name = providers::secret_name(manifest.id);
    let mut key = None;
    if let Some(project_id) = scope.project_id
        && let Some(id) = secrets::find(
            &state.pool,
            SecretOwner::Project {
                organization_id: scope.organization_id,
                project_id,
            },
            &name,
        )
        .await?
    {
        key = Some((id, KeySource::Project));
    }
    if key.is_none()
        && let Some(id) = secrets::find(
            &state.pool,
            SecretOwner::Workspace {
                organization_id: scope.organization_id,
            },
            &name,
        )
        .await?
    {
        key = Some((id, KeySource::Workspace));
    }
    if manifest.needs_key && key.is_none() {
        return Err(AiError::NotConfigured(format!(
            "Add your {} API key in Runs and models settings.",
            manifest.display_name
        )));
    }
    Ok(ResolvedModel {
        provider,
        manifest,
        model,
        base_url,
        key,
    })
}

/// The workspace's monthly budget in whole dollars; 0 means no limit.
pub async fn budget_usd(pool: &PgPool, organization_id: Uuid) -> Result<i64, AppError> {
    let mut connection = pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        SettingsContext {
            organization_id,
            project_id: None,
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    Ok(effective.value::<i64>("ai.monthly_budget_usd").unwrap_or(0))
}

/// What the workspace has spent this calendar month (UTC), in millionths of
/// a dollar.
pub async fn spent_this_month_micros(
    pool: &PgPool,
    organization_id: Uuid,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(sum(cost_micros), 0)::bigint FROM ai_usage
        WHERE organization_id = $1
          AND created_at >= (date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
        "#,
    )
    .bind(organization_id)
    .fetch_one(pool)
    .await?)
}

/// A model's prices: a person's edit in the workspace's model catalog (any
/// harness listing the same model) first, then the provider's own list.
async fn prices(
    pool: &PgPool,
    organization_id: Uuid,
    resolved: &ResolvedModel,
) -> Result<Option<ModelPrices>, AppError> {
    let rows = sqlx::query_as::<_, (Option<Value>, Option<Value>)>(
        r#"
        SELECT overrides->'prices', provider_data->'prices'
        FROM harness_models
        WHERE organization_id = $1 AND model_id = $2
        ORDER BY edited_at DESC NULLS LAST
        LIMIT 5
        "#,
    )
    .bind(organization_id)
    .bind(&resolved.model)
    .fetch_all(pool)
    .await?;
    for (edited, reported) in rows {
        for candidate in [edited, reported].into_iter().flatten() {
            if let Ok(prices) = serde_json::from_value::<ModelPrices>(candidate) {
                return Ok(Some(prices));
            }
        }
    }
    Ok(resolved
        .manifest
        .models
        .iter()
        .find(|model| model.id == resolved.model)
        .and_then(|model| model.prices.clone()))
}

pub fn cost_micros(prices: Option<&ModelPrices>, input_tokens: i64, output_tokens: i64) -> i64 {
    let Some(prices) = prices else {
        return 0;
    };
    // Per million tokens to millionths of a unit: the factors cancel.
    let cost = prices.input_per_million * input_tokens as f64
        + prices.output_per_million * output_tokens as f64;
    if cost.is_finite() && cost > 0.0 {
        cost.round() as i64
    } else {
        0
    }
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}

/// A checked answer and what made it.
#[derive(Clone, Debug)]
pub struct Answer<T> {
    pub value: T,
    pub provider: &'static str,
    pub model: String,
    pub prompt_id: &'static str,
    pub prompt_version: i32,
}

struct UsageRow<'a> {
    scope: Scope,
    prompt: &'a Prompt,
    resolved: &'a ResolvedModel,
    tier: Tier,
    input_tokens: i64,
    output_tokens: i64,
    cost: i64,
    outcome: &'static str,
    error: Option<String>,
    latency_ms: i64,
}

async fn record_usage(pool: &PgPool, row: UsageRow<'_>) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO ai_usage (
            id, organization_id, project_id, actor_id, prompt_id, prompt_version,
            provider, model, tier, input_tokens, output_tokens, cost_micros,
            outcome, error, latency_ms
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(row.scope.organization_id)
    .bind(row.scope.project_id)
    .bind(row.scope.actor_id)
    .bind(row.prompt.id)
    .bind(row.prompt.version)
    .bind(row.resolved.manifest.id)
    .bind(row.resolved.model.chars().take(200).collect::<String>())
    .bind(row.tier.as_str())
    .bind(row.input_tokens)
    .bind(row.output_tokens)
    .bind(row.cost)
    .bind(row.outcome)
    .bind(
        row.error
            .map(|error| error.chars().take(300).collect::<String>()),
    )
    .bind(i32::try_from(row.latency_ms).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;
    Ok(())
}

/// Runs `prompt` on `input` with the model `scope`'s settings choose, and
/// returns the checked reply as `T`.
pub async fn run<T: DeserializeOwned>(
    state: &AppState,
    scope: Scope,
    prompt: &'static Prompt,
    input: &impl Serialize,
    tier: Tier,
) -> Result<Answer<T>, AiError> {
    let resolved = resolve(state, scope, tier).await?;
    run_with(state, scope, prompt, input, tier, &resolved).await
}

/// [`run`] with the model already resolved.
pub async fn run_with<T: DeserializeOwned>(
    state: &AppState,
    scope: Scope,
    prompt: &'static Prompt,
    input: &impl Serialize,
    tier: Tier,
    resolved: &ResolvedModel,
) -> Result<Answer<T>, AiError> {
    let budget = budget_usd(&state.pool, scope.organization_id).await?;
    if budget > 0
        && spent_this_month_micros(&state.pool, scope.organization_id).await?
            >= budget.saturating_mul(1_000_000)
    {
        return Err(AiError::OverBudget { budget_usd: budget });
    }
    let input = serde_json::to_string(input).map_err(|_| AppError::Internal("AI input"))?;
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(AiError::App(AppError::Internal("AI input over the limit")));
    }
    let key = match resolved.key {
        Some((secret_id, _)) => {
            let keys = secrets::keyring(&state.config).map_err(|_| {
                AiError::NotConfigured(
                    "This server has no secrets keyring, so it can't read the provider's key."
                        .to_owned(),
                )
            })?;
            Some(secrets::reveal(&state.pool, keys.as_ref(), secret_id, "ai.gateway").await?)
        }
        None => None,
    };
    let prices = prices(&state.pool, scope.organization_id, resolved).await?;
    let mut system = prompt.system.to_owned();
    for attempt in 0..2 {
        let started = std::time::Instant::now();
        let request = CompletionRequest {
            model: &resolved.model,
            system: &system,
            input: &input,
            prompt_id: prompt.id,
            schema: prompt.schema(),
            max_output_tokens: prompt.max_output_tokens,
            api_key: key.as_ref(),
            base_url: resolved.base_url.as_deref(),
            timeout: tier.timeout(),
        };
        let result = resolved.provider.complete(client(), &request).await;
        let latency_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        let completion = match result {
            Ok(completion) => completion,
            Err(error) => {
                record_usage(
                    &state.pool,
                    UsageRow {
                        scope,
                        prompt,
                        resolved,
                        tier,
                        input_tokens: 0,
                        output_tokens: 0,
                        cost: 0,
                        outcome: if error == ProviderError::Refused {
                            "refused"
                        } else {
                            "provider_error"
                        },
                        error: Some(error.category()),
                        latency_ms,
                    },
                )
                .await?;
                return Err(AiError::Provider(error));
            }
        };
        let cost = cost_micros(
            prices.as_ref(),
            completion.input_tokens,
            completion.output_tokens,
        );
        let parsed = serde_json::from_str::<Value>(completion.text.trim())
            .map_err(|_| "the reply wasn't JSON".to_owned())
            .and_then(|value| match schema::validate(prompt.schema(), &value) {
                None => serde_json::from_value::<T>(value)
                    .map_err(|_| "the reply didn't match the expected shape".to_owned()),
                Some(problem) => Err(problem),
            });
        record_usage(
            &state.pool,
            UsageRow {
                scope,
                prompt,
                resolved,
                tier,
                input_tokens: completion.input_tokens,
                output_tokens: completion.output_tokens,
                cost,
                outcome: if parsed.is_ok() {
                    "succeeded"
                } else {
                    "invalid_output"
                },
                error: parsed.as_ref().err().map(|_| "schema mismatch".to_owned()),
                latency_ms,
            },
        )
        .await?;
        match parsed {
            Ok(value) => {
                return Ok(Answer {
                    value,
                    provider: resolved.manifest.id,
                    model: resolved.model.clone(),
                    prompt_id: prompt.id,
                    prompt_version: prompt.version,
                });
            }
            Err(problem) if attempt == 0 => {
                // The problem names a JSON path and a rule, never reply text.
                system = format!(
                    "{}\n\n{}: {problem}. Reply again with only JSON that matches it.",
                    prompt.system,
                    providers::fake::RETRY_NOTE
                );
            }
            Err(_) => {}
        }
    }
    Err(AiError::InvalidOutput)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_come_from_per_million_prices() {
        let prices = ModelPrices {
            currency: "USD".to_owned(),
            input_per_million: 2.0,
            output_per_million: 10.0,
            cached_input_per_million: None,
        };
        // 1,000 input tokens at $2/M is $0.002 = 2,000 micros; 100 output
        // tokens at $10/M is $0.001 = 1,000 micros.
        assert_eq!(cost_micros(Some(&prices), 1_000, 100), 3_000);
        assert_eq!(cost_micros(None, 1_000, 100), 0);
    }

    #[test]
    fn the_test_provider_is_not_offered_in_production() {
        assert!(
            offered_providers(Environment::Production)
                .iter()
                .all(|manifest| !manifest.testing_only)
        );
        assert!(
            offered_providers(Environment::Test)
                .iter()
                .any(|manifest| manifest.testing_only)
        );
    }
}

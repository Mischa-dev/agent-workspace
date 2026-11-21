//! H5 and U2: what a run did, and what it cost.
//!
//! Harness adapters report steps (commands, file changes, other tools), the
//! agent's own notes and token usage as neutral events. This module turns
//! them into run event payloads and usage rows. Payloads are redacted and
//! capped before they are stored: command output can carry anything a
//! command printed, keys included, so every string goes through [`redact`]
//! first, and no payload passes [`MAX_PAYLOAD_BYTES`].
//!
//! Tokens are priced from the workspace's model catalog (the same prices
//! the catalog page edits), and a harness that reports its own cost keeps
//! that beside the estimate, for models the catalog has no price for.

use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    harness::manifest::ModelPrices,
    provider::{adapter::TokenUsage, catalog},
};

pub use agent_workspace_redaction::{
    MAX_NOTE_CHARS, MAX_OUTPUT_CHARS, MAX_PAYLOAD_BYTES, cap, redact, redacted, secret_field,
};

pub use agent_workspace_builtin_host::steps::{
    detail_payload, detect_tests, item_payload, note_payload, step_summary,
};

// ---------------------------------------------------------------------------
// Tokens and cost

/// The estimated cost of some tokens at a model's catalog prices. Cached
/// input is billed at its own price when the catalog has one, else as input.
pub fn price(usage: &TokenUsage, prices: &ModelPrices) -> f64 {
    let per = |tokens: u64, per_million: f64| tokens as f64 * per_million / 1_000_000.0;
    per(usage.input_tokens, prices.input_per_million)
        + per(usage.output_tokens, prices.output_per_million)
        + per(
            usage.cached_input_tokens,
            prices
                .cached_input_per_million
                .unwrap_or(prices.input_per_million),
        )
}

/// Tokens a run reported for one model.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cached_input: i64,
}

/// What a built-in run's tokens cost at the workspace's catalog prices for
/// its harness, with the currency; `None` for a run on another host or a
/// model the catalog has no price for.
pub async fn builtin_estimate(
    state: &AppState,
    run_id: Uuid,
    model: &str,
    tokens: Tokens,
) -> Result<Option<(f64, String)>, AppError> {
    let Some(scope) =
        crate::runs::usage::builtin_scope_of(crate::db::scoped(&state.pool), run_id).await?
    else {
        return Ok(None);
    };
    let harnesses = crate::harness_catalog::Harnesses::of(state, scope.workspace_id).await?;
    // A registered harness switched off mid-run still prices that run.
    let Some(manifest) = harnesses.known(&scope.adapter_kind) else {
        return Ok(None);
    };
    let prices = catalog::run_catalog(&state.pool, scope.workspace_id, manifest)
        .await?
        .into_iter()
        .find(|entry| entry.id == model)
        .and_then(|entry| entry.prices);
    let count = |tokens: i64| u64::try_from(tokens).unwrap_or(0);
    Ok(prices.map(|prices| {
        let usage = TokenUsage {
            input_tokens: count(tokens.input),
            output_tokens: count(tokens.output),
            cached_input_tokens: count(tokens.cached_input),
            ..TokenUsage::default()
        };
        (price(&usage, &prices), prices.currency.clone())
    }))
}

/// A run's (or a set of runs') tokens and cost, for views.
#[derive(Clone, Debug, Default, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    /// The catalog estimate where there is one, else what the harness
    /// reported; `None` when neither knows.
    pub cost: Option<f64>,
    pub currency: Option<String>,
}

/// Sums usage rows; cost prefers each row's catalog estimate over the
/// harness's own figure, and reports USD when currencies are mixed or none.
pub const TOTALS: &str = r#"
    COALESCE(sum(input_tokens), 0)::bigint AS input_tokens,
    COALESCE(sum(output_tokens), 0)::bigint AS output_tokens,
    COALESCE(sum(cached_input_tokens), 0)::bigint AS cached_input_tokens,
    sum(COALESCE(estimated_cost, reported_cost)) AS cost,
    CASE WHEN count(*) FILTER (WHERE COALESCE(estimated_cost, reported_cost) IS NOT NULL) = 0
         THEN NULL
         WHEN count(DISTINCT COALESCE(CASE WHEN estimated_cost IS NOT NULL THEN currency
                                           ELSE reported_currency END, 'USD')) = 1
         THEN min(COALESCE(CASE WHEN estimated_cost IS NOT NULL THEN currency
                                ELSE reported_currency END, 'USD'))
         ELSE 'USD' END AS currency
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::adapter::{HarnessItem, ItemDetail, ItemKind, TestCounts};

    #[test]
    fn credentials_are_removed_from_reported_text() {
        let text = "export OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz0123\n\
                    curl -H 'Authorization: Bearer abcdef0123456789xyz' https://x\n\
                    ghp_abcdefghijklmnopqrstuvwxyz0123456789 and AKIAABCDEFGHIJKLMNOP\n\
                    {\"password\": \"hunter2hunter2\", \"user\": \"mischa\"}\n\
                    token=abcd1234efgh jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.c2lnbmF0dXJlMTIz";
        let redacted = redact(text);
        for secret in [
            "sk-proj-abcdefghijklmnopqrstuvwxyz0123",
            "abcdef0123456789xyz",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "AKIAABCDEFGHIJKLMNOP",
            "hunter2hunter2",
            "abcd1234efgh",
            "c2lnbmF0dXJlMTIz",
        ] {
            assert!(!redacted.contains(secret), "{secret} leaked: {redacted}");
        }
        assert!(redacted.contains("\"user\": \"mischa\""));
        assert!(redacted.contains("https://x"));
        assert_eq!(redact("cargo test --workspace"), "cargo test --workspace");
        // Command lines: a flag's value after a space, and quoted values
        // with spaces in them.
        let command = "./deploy --token abcdefghijklmnop --api-key=zyxwvutsrq --verbose";
        let redacted = redact(command);
        assert!(!redacted.contains("abcdefghijklmnop"), "{redacted}");
        assert!(!redacted.contains("zyxwvutsrq"), "{redacted}");
        assert!(redacted.contains("--verbose"), "{redacted}");
        let quoted = redact("PASSWORD='alpha beta gamma' ./deploy && TOKEN=\"one two three\" run");
        for secret in ["alpha", "beta", "gamma", "one", "two", "three"] {
            assert!(!quoted.contains(secret), "{secret} leaked: {quoted}");
        }
        assert!(
            quoted.contains("./deploy") && quoted.ends_with(" run"),
            "{quoted}"
        );
        assert_eq!(redact("git log --format=%H"), "git log --format=%H");
        assert_eq!(
            redact("the task-key AW-12 passes"),
            "the task-key AW-12 passes"
        );
    }

    #[test]
    fn private_key_blocks_are_removed() {
        let text = "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\nafter";
        let redacted = redact(text);
        assert!(!redacted.contains("b3BlbnNzaC1rZXk"));
        assert!(redacted.contains("before") && redacted.contains("after"));
        let public = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
        assert_eq!(redact(public), public);
    }

    #[test]
    fn long_output_keeps_its_start_and_end() {
        let text = (0..10_000).map(|n| format!("{n}\n")).collect::<String>();
        let capped = cap(&text, 1_000);
        assert!(capped.chars().count() < 1_100);
        assert!(capped.starts_with("0\n1\n"));
        assert!(capped.trim_end().ends_with("9999"));
        assert!(capped.contains("characters left out"));
    }

    #[test]
    fn test_summaries_are_counted() {
        let cargo = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 1 ignored\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored";
        assert_eq!(
            detect_tests(cargo),
            Some(TestCounts {
                passed: 5,
                failed: 1,
                skipped: 1
            })
        );
        assert_eq!(
            detect_tests("Tests:       2 failed, 40 passed, 42 total"),
            Some(TestCounts {
                passed: 40,
                failed: 2,
                skipped: 0
            })
        );
        assert_eq!(
            detect_tests("==== 12 passed, 1 skipped in 0.31s ===="),
            Some(TestCounts {
                passed: 12,
                failed: 0,
                skipped: 1
            })
        );
        assert_eq!(detect_tests("Compiling foo v0.1.0\nFinished"), None);
    }

    #[test]
    fn payloads_are_redacted_and_bounded() {
        let item = HarnessItem {
            key: "k".to_owned(),
            kind: ItemKind::CommandExecution,
            detail: ItemDetail {
                command: Some("deploy --token=supersecretvalue".to_owned()),
                exit_code: Some(0),
                output: Some("x".repeat(200_000)),
                ..ItemDetail::default()
            },
        };
        let payload = item_payload(&item);
        let text = payload.to_string();
        assert!(!text.contains("supersecretvalue"));
        assert!(text.len() <= MAX_PAYLOAD_BYTES);
        assert_eq!(payload["exitCode"], 0);
    }

    #[test]
    fn tokens_are_priced_per_million_with_cached_input_at_its_own_price() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cached_input_tokens: 2_000_000,
            ..TokenUsage::default()
        };
        let prices = ModelPrices {
            currency: "USD".to_owned(),
            input_per_million: 4.0,
            output_per_million: 20.0,
            cached_input_per_million: Some(0.2),
        };
        assert!((price(&usage, &prices) - (4.0 + 10.0 + 0.4)).abs() < 1e-9);
        let no_cached_price = ModelPrices {
            cached_input_per_million: None,
            ..prices
        };
        assert!((price(&usage, &no_cached_price) - (4.0 + 10.0 + 8.0)).abs() < 1e-9);
    }
}

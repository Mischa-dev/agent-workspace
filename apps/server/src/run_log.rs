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

use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    provider::{
        adapter::{HarnessItem, ItemDetail, ItemKind, TestCounts, TokenUsage},
        catalog,
        manifest::ModelPrices,
    },
};

use agent_workspace_redaction::{MAX_COMMAND_CHARS, MAX_FILES_PER_STEP, clean};
pub use agent_workspace_redaction::{
    MAX_NOTE_CHARS, MAX_OUTPUT_CHARS, MAX_PAYLOAD_BYTES, cap, redact, redacted, secret_field,
};

// ---------------------------------------------------------------------------
// Tests a command ran

/// Counts tests from the summary line common runners print: cargo, Jest
/// and Vitest, pytest, Go and Mocha. `None` when nothing looks like one.
pub fn detect_tests(output: &str) -> Option<TestCounts> {
    let mut found = None::<TestCounts>;
    for line in output.lines().rev().take(400) {
        let lower = line.trim().to_ascii_lowercase();
        // The first place `word` follows a number ("FAILED. 2 passed; 1
        // failed" names "failed" twice).
        let number_before = |word: &str| -> Option<u32> {
            lower.match_indices(word).find_map(|(at, _)| {
                lower[..at]
                    .trim_end()
                    .rsplit(|c: char| !c.is_ascii_digit())
                    .next()
                    .filter(|digits| !digits.is_empty())?
                    .parse()
                    .ok()
            })
        };
        let looks_like_summary = lower.starts_with("test result:")
            || lower.starts_with("tests:")
            || lower.starts_with("tests ")
            || (lower.starts_with('=') && lower.contains(" in "))
            || lower.contains(" passing");
        if !looks_like_summary {
            continue;
        }
        let passed = number_before(" passed").or_else(|| number_before(" passing"));
        let failed = number_before(" failed").or_else(|| number_before(" failing"));
        let skipped = number_before(" skipped")
            .or_else(|| number_before(" ignored"))
            .or_else(|| number_before(" pending"));
        if passed.is_none() && failed.is_none() {
            continue;
        }
        let counts = found.get_or_insert_with(TestCounts::default);
        // Cargo prints one summary per test binary; add them up.
        counts.passed += passed.unwrap_or(0);
        counts.failed += failed.unwrap_or(0);
        counts.skipped += skipped.unwrap_or(0);
        if !lower.starts_with("test result:") {
            break;
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Payloads

/// The stored payload of one step: redacted, capped, and only the fields
/// the run page shows.
pub fn item_payload(item: &HarnessItem) -> Value {
    detail_payload(item.kind, &item.detail)
}

pub fn detail_payload(kind: ItemKind, detail: &ItemDetail) -> Value {
    let mut payload = Map::new();
    if let Some(tool) = &detail.tool {
        payload.insert("tool".to_owned(), json!(clean(tool, 120)));
    }
    if let Some(command) = &detail.command {
        payload.insert(
            "command".to_owned(),
            json!(clean(command, MAX_COMMAND_CHARS)),
        );
    }
    if let Some(code) = detail.exit_code {
        payload.insert("exitCode".to_owned(), json!(code));
    }
    if let Some(output) = detail.output.as_deref().filter(|output| !output.is_empty()) {
        payload.insert("output".to_owned(), json!(clean(output, MAX_OUTPUT_CHARS)));
    }
    if !detail.files.is_empty() {
        payload.insert(
            "files".to_owned(),
            Value::Array(
                detail
                    .files
                    .iter()
                    .take(MAX_FILES_PER_STEP)
                    .map(|(path, change)| json!({ "path": clean(path, 500), "change": change }))
                    .collect(),
            ),
        );
    }
    let tests = detail.tests.or_else(|| {
        (kind == ItemKind::CommandExecution)
            .then(|| detail.output.as_deref().and_then(detect_tests))
            .flatten()
    });
    if let Some(tests) = tests {
        payload.insert(
            "tests".to_owned(),
            json!({ "passed": tests.passed, "failed": tests.failed, "skipped": tests.skipped }),
        );
    }
    if detail.failed {
        payload.insert("failed".to_owned(), json!(true));
    }
    fit(Value::Object(payload))
}

pub fn note_payload(text: &str) -> Value {
    fit(json!({ "text": clean(text, MAX_NOTE_CHARS) }))
}

/// Drops the output, then everything, rather than store an oversized row.
fn fit(mut payload: Value) -> Value {
    let size = |value: &Value| serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len());
    if size(&payload) <= MAX_PAYLOAD_BYTES {
        return payload;
    }
    if let Some(object) = payload.as_object_mut() {
        object.remove("output");
        object.insert("truncated".to_owned(), json!(true));
    }
    if size(&payload) <= MAX_PAYLOAD_BYTES {
        return payload;
    }
    json!({ "truncated": true })
}

/// A one-line summary of a step for the thread and the timeline.
pub fn step_summary(kind: &str, detail: &ItemDetail) -> String {
    let line = |text: &str| {
        let first = redact(text.lines().next().unwrap_or_default());
        cap(first.trim(), 160)
    };
    match kind {
        "command_started" => detail.command.as_deref().map_or_else(
            || "The agent started a command in the isolated worktree.".to_owned(),
            |command| format!("Running {}", line(command)),
        ),
        "command_completed" => match (detail.command.as_deref(), detail.exit_code) {
            (Some(command), Some(code)) => format!("Ran {} (exit {code})", line(command)),
            (Some(command), None) => format!("Ran {}", line(command)),
            _ => "The agent completed a command in the isolated worktree.".to_owned(),
        },
        "file_change" => match detail.files.len() {
            0 => "The agent completed a file change in the isolated worktree.".to_owned(),
            1 => format!("Changed {}", line(&detail.files[0].0)),
            count => format!("Changed {count} files"),
        },
        "tool_used" => format!("Used {}", line(detail.tool.as_deref().unwrap_or("a tool"))),
        _ => String::new(),
    }
}

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

/// Adds usage to the run's row for its model and prices it. Usage is a
/// side note to the run: a failure here is logged, never the run's.
pub async fn record_tokens(state: &AppState, run_id: Uuid, usage: TokenUsage) {
    if let Err(error) = store_tokens(state, run_id, usage).await {
        tracing::warn!(error_category = error.log_category(), %run_id, "run usage not recorded");
    }
}

async fn store_tokens(state: &AppState, run_id: Uuid, usage: TokenUsage) -> Result<(), AppError> {
    let scope = crate::runs::usage::builtin_scope(crate::db::scoped(&state.pool), run_id).await?;
    let model = usage
        .model
        .clone()
        .filter(|model| (1..=200).contains(&model.len()))
        .or(scope.model.clone())
        .unwrap_or_else(|| "default".to_owned());
    let prices = match state.supervisors.manifest(&scope.adapter_kind) {
        Some(manifest) => catalog::run_catalog(&state.pool, scope.workspace_id, &manifest)
            .await?
            .into_iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.prices),
        None => None,
    };
    let clamp = |tokens: u64| i64::try_from(tokens).unwrap_or(i64::MAX);
    crate::runs::usage::add_builtin(
        crate::db::scoped(&state.pool),
        run_id,
        &crate::runs::usage::BuiltinTokens {
            model,
            backend: scope.adapter_kind.clone(),
            date: scope.created_at.date_naive(),
            input_tokens: clamp(usage.input_tokens),
            output_tokens: clamp(usage.output_tokens),
            cached_input_tokens: clamp(usage.cached_input_tokens),
            estimated_cost: prices.as_ref().map(|prices| price(&usage, prices)),
            currency: prices.as_ref().map(|prices| prices.currency.clone()),
            reported_cost_usd: usage
                .reported_cost_usd
                .filter(|cost| cost.is_finite() && *cost >= 0.0),
        },
    )
    .await
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

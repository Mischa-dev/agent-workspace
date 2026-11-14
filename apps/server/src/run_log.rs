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

use chrono::Utc;
use serde_json::{Map, Value, json};
use sqlx::PgPool;
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

/// The most of one step's output a run keeps: its start and its end.
pub const MAX_OUTPUT_CHARS: usize = 8_000;
/// The most one note keeps.
pub const MAX_NOTE_CHARS: usize = 4_000;
/// The most one run event payload may hold (the database checks it too).
pub const MAX_PAYLOAD_BYTES: usize = 48 * 1024;
const MAX_FILES_PER_STEP: usize = 200;
const MAX_COMMAND_CHARS: usize = 2_000;
const REDACTED: &str = "[redacted]";

// ---------------------------------------------------------------------------
// Redaction

/// Words that, followed by `=` or `:`, introduce a secret value.
const SECRET_KEYS: [&str; 9] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "private_key",
];

/// Object keys whose value is a secret, compared lower-cased with `_`,
/// `-` and other separators removed, by how the key ends: `password`,
/// `client_secret`, `X-Api-Key`, `accessToken`, `Set-Cookie`. Counts such
/// as `inputTokens` don't end in one and are kept.
const SECRET_FIELD_ENDINGS: [&str; 16] = [
    "password",
    "passwords",
    "passwd",
    "secret",
    "secrets",
    "token",
    "apikey",
    "accesskey",
    "privatekey",
    "authorization",
    "cookie",
    "cookies",
    "credential",
    "credentials",
    "sessionkey",
    "signingkey",
];

/// Whether a JSON object key names a secret, so its whole value goes.
pub fn secret_field(key: &str) -> bool {
    let compact = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    SECRET_FIELD_ENDINGS
        .iter()
        .any(|ending| compact.ends_with(ending))
}

/// What a redacted value is stored as.
pub fn redacted() -> Value {
    Value::String(REDACTED.to_owned())
}

/// Prefixes of well-known credential formats; the rest of the word goes.
const SECRET_PREFIXES: [(&str, usize); 12] = [
    ("sk-", 16),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("github_pat_", 20),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("glpat-", 16),
    ("AKIA", 16),
    ("AIza", 30),
];

fn token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/' | '+' | '=')
}

/// Removes credentials from text a harness reported: private key blocks,
/// bearer tokens, JSON web tokens, well-known key formats and `key=value`
/// pairs whose key names a secret. Over-redacting is fine; this text is
/// for people reviewing a run, not for replaying it.
pub fn redact(text: &str) -> String {
    let text = redact_private_keys(text);
    let mut out = String::with_capacity(text.len());
    let characters = text.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        let (offset, character) = characters[index];
        let rest = &text[offset..];
        let at_word_start = index == 0 || !token_char(characters[index - 1].1);
        if at_word_start {
            // Well-known formats.
            if let Some((prefix, minimum)) = SECRET_PREFIXES
                .iter()
                .find(|(prefix, _)| rest.starts_with(prefix))
            {
                let length = rest.chars().take_while(|c| token_char(*c)).count();
                if length >= prefix.len() + minimum {
                    out.push_str(REDACTED);
                    index += length;
                    continue;
                }
            }
            // A JSON web token: three base64url parts, the first `eyJ`.
            if rest.starts_with("eyJ") {
                let word = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                    .collect::<String>();
                if word.split('.').filter(|part| !part.is_empty()).count() >= 3 && word.len() > 30 {
                    out.push_str(REDACTED);
                    index += word.chars().count();
                    continue;
                }
            }
            // `Bearer <token>` and `Basic <token>`.
            for scheme in ["Bearer ", "bearer ", "Basic ", "basic "] {
                if let Some(after) = rest.strip_prefix(scheme) {
                    let length = after.chars().take_while(|c| token_char(*c)).count();
                    if length >= 8 {
                        out.push_str(scheme);
                        out.push_str(REDACTED);
                        index += scheme.chars().count() + length;
                        break;
                    }
                }
            }
            if index < characters.len() && characters[index].0 != offset {
                continue;
            }
            // `token=…`, `"password": "…"`, `API_KEY: …`, and on a command
            // line `--token …` and `PASSWORD='with spaces'`.
            let lower = rest
                .chars()
                .take(32)
                .collect::<String>()
                .to_ascii_lowercase();
            if let Some(key_length) = secret_key_length(&lower) {
                let key = rest.chars().take(key_length).collect::<String>();
                let after = &rest[key.len()..];
                let separator = after
                    .chars()
                    .take_while(|c| matches!(c, '"' | '\'' | ' ' | ':' | '='))
                    .collect::<String>();
                // A flag (`--token abc`) takes its value after a space.
                let flag_value = key.starts_with('-')
                    && separator.starts_with(' ')
                    && !separator.contains(['=', ':']);
                if separator.contains(['=', ':']) || flag_value {
                    // A quoted value runs to its closing quote, spaces and
                    // all; a bare one to the next space or delimiter.
                    let quote = separator.chars().last().filter(|c| matches!(c, '"' | '\''));
                    let value = after[separator.len()..]
                        .chars()
                        .take_while(|c| match quote {
                            Some(quote) => *c != quote,
                            None => !c.is_whitespace() && !matches!(c, '"' | '\'' | ',' | '}'),
                        })
                        .collect::<String>();
                    if value.len() >= 4 {
                        out.push_str(&key);
                        out.push_str(&separator);
                        out.push_str(REDACTED);
                        index +=
                            key.chars().count() + separator.chars().count() + value.chars().count();
                        continue;
                    }
                }
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// The length of a secret-naming key at the start of `lower` (already
/// lowercased), allowing a prefix such as `github_` or `x-api-`.
fn secret_key_length(lower: &str) -> Option<usize> {
    let word = lower
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect::<String>();
    SECRET_KEYS
        .iter()
        .any(|key| word.ends_with(key))
        .then_some(word.chars().count())
}

fn redact_private_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let header_end = rest[start..]
            .find("-----\n")
            .or_else(|| rest[start + 11..].find("-----").map(|end| end + 11));
        let Some(header_end) = header_end else { break };
        let header = &rest[start..start + header_end];
        if !header.contains("PRIVATE KEY") {
            out.push_str(&rest[..start + header_end]);
            rest = &rest[start + header_end..];
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str("[redacted private key]");
        match rest[start..].find("-----END ") {
            Some(end) => {
                let tail = &rest[start + end + 9..];
                let close = tail.find("-----").map_or(tail.len(), |close| close + 5);
                rest = &tail[close..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Keeps the start and end of long text, with a note of what was cut.
pub fn cap(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let head = max_chars / 4;
    let tail = max_chars - head;
    let start = text.chars().take(head).collect::<String>();
    let end = text.chars().skip(count - tail).collect::<String>();
    format!(
        "{start}\n… {} characters left out …\n{end}",
        count - head - tail
    )
}

fn clean(text: &str, max_chars: usize) -> String {
    // Control characters other than newlines and tabs only garble a log.
    let text = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect::<String>();
    cap(&redact(&text), max_chars)
}

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

#[derive(sqlx::FromRow)]
struct RunScope {
    organization_id: Uuid,
    project_id: Uuid,
    provider_account_id: Uuid,
    adapter_kind: String,
    model: Option<String>,
    created_at: chrono::DateTime<Utc>,
}

/// Adds usage to the run's row for its model and prices it. Usage is a
/// side note to the run: a failure here is logged, never the run's.
pub async fn record_tokens(state: &AppState, run_id: Uuid, usage: TokenUsage) {
    if let Err(error) = store_tokens(state, run_id, usage).await {
        tracing::warn!(error_category = error.log_category(), %run_id, "run usage not recorded");
    }
}

async fn store_tokens(state: &AppState, run_id: Uuid, usage: TokenUsage) -> Result<(), AppError> {
    let scope = sqlx::query_as::<_, RunScope>(
        r#"
        SELECT run.project_id, project.organization_id,
               profile.provider_account_id, account.adapter_kind,
               run.config ->> 'model' AS model, run.created_at
        FROM agent_runs run
        JOIN projects project ON project.id = run.project_id
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(&state.pool)
    .await?;
    let model = usage
        .model
        .clone()
        .filter(|model| (1..=200).contains(&model.len()))
        .or(scope.model.clone())
        .unwrap_or_else(|| "default".to_owned());
    let prices = match state.supervisors.manifest(&scope.adapter_kind) {
        Some(manifest) => catalog::run_catalog(&state.pool, scope.organization_id, &manifest)
            .await?
            .into_iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| entry.prices),
        None => None,
    };
    let cost = prices.as_ref().map(|prices| price(&usage, prices));
    let clamp = |tokens: u64| i64::try_from(tokens).unwrap_or(i64::MAX);
    let reported = usage
        .reported_cost_usd
        .filter(|cost| cost.is_finite() && *cost >= 0.0);
    sqlx::query(
        r#"
        INSERT INTO agent_run_usage (
            agent_run_id, model, organization_id, project_id, provider_account_id,
            adapter_kind, usage_date, input_tokens, output_tokens, cached_input_tokens,
            estimated_cost, currency, reported_cost_usd
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7::date, $8, $9, $10, $11, $12, $13)
        ON CONFLICT (agent_run_id, model) DO UPDATE SET
            input_tokens = agent_run_usage.input_tokens + EXCLUDED.input_tokens,
            output_tokens = agent_run_usage.output_tokens + EXCLUDED.output_tokens,
            cached_input_tokens = agent_run_usage.cached_input_tokens + EXCLUDED.cached_input_tokens,
            estimated_cost = CASE
                WHEN EXCLUDED.estimated_cost IS NULL THEN agent_run_usage.estimated_cost
                ELSE COALESCE(agent_run_usage.estimated_cost, 0) + EXCLUDED.estimated_cost END,
            currency = COALESCE(EXCLUDED.currency, agent_run_usage.currency),
            reported_cost_usd = CASE
                WHEN EXCLUDED.reported_cost_usd IS NULL THEN agent_run_usage.reported_cost_usd
                ELSE COALESCE(agent_run_usage.reported_cost_usd, 0) + EXCLUDED.reported_cost_usd END,
            updated_at = now()
        "#,
    )
    .bind(run_id)
    .bind(&model)
    .bind(scope.organization_id)
    .bind(scope.project_id)
    .bind(scope.provider_account_id)
    .bind(&scope.adapter_kind)
    .bind(scope.created_at.date_naive())
    .bind(clamp(usage.input_tokens))
    .bind(clamp(usage.output_tokens))
    .bind(clamp(usage.cached_input_tokens))
    .bind(cost)
    .bind(prices.as_ref().map(|prices| prices.currency.clone()))
    .bind(reported)
    .execute(&state.pool)
    .await?;
    Ok(())
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

/// One run's totals across its models.
pub async fn run_totals(pool: &PgPool, run_id: Uuid) -> Result<UsageTotals, AppError> {
    Ok(sqlx::query_as::<_, UsageTotals>(&format!(
        "SELECT {TOTALS} FROM agent_run_usage WHERE agent_run_id = $1"
    ))
    .bind(run_id)
    .fetch_one(pool)
    .await?)
}

/// Sums usage rows; cost prefers each row's catalog estimate over the
/// harness's own figure, and reports USD when currencies are mixed or none.
pub const TOTALS: &str = r#"
    COALESCE(sum(input_tokens), 0)::bigint AS input_tokens,
    COALESCE(sum(output_tokens), 0)::bigint AS output_tokens,
    COALESCE(sum(cached_input_tokens), 0)::bigint AS cached_input_tokens,
    sum(COALESCE(estimated_cost, reported_cost_usd)) AS cost,
    CASE WHEN count(*) FILTER (WHERE COALESCE(estimated_cost, reported_cost_usd) IS NOT NULL) = 0
         THEN NULL
         WHEN count(DISTINCT COALESCE(currency, 'USD')) = 1 THEN min(COALESCE(currency, 'USD'))
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

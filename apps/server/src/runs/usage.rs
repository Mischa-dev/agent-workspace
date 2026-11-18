//! A1.3: usage on a run. A reporter's usage for one of its runs adds to the
//! run's totals per model as exact (`attribution = 'reported'`), the
//! built-in host's included ([`add_builtin`]). A1.15: window reports are split onto the runs
//! going during the window ([`attribute_window_usage`]), and [`totals`] is
//! what budgets and metering read.

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{FromRow, PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    ops::{RunOpError, UsageInput},
    store::RunRow,
};
use crate::{
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry},
};

/// What the built-in host needs to price a run's usage.
#[derive(Clone, Debug, FromRow)]
pub struct BuiltinScope {
    pub workspace_id: Uuid,
    pub adapter_kind: String,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A built-in run's workspace, harness and chosen model.
pub async fn builtin_scope(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
) -> Result<BuiltinScope, AppError> {
    Ok(sqlx::query_as::<_, BuiltinScope>(
        r#"
        SELECT project.organization_id AS workspace_id, account.adapter_kind,
               run.config ->> 'model' AS model, run.created_at
        FROM agent_runs run
        JOIN projects project ON project.id = run.project_id
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(executor)
    .await?)
}

/// Tokens a built-in run used on one turn, priced by the catalog where it
/// can be.
#[derive(Clone, Debug)]
pub struct BuiltinTokens {
    pub model: String,
    pub backend: String,
    pub date: NaiveDate,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub estimated_cost: Option<f64>,
    pub currency: Option<String>,
    pub reported_cost_usd: Option<f64>,
}

/// Add a built-in run's turn to its usage for the model, as reported.
pub async fn add_builtin(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
    tokens: &BuiltinTokens,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO run_usage (
            run_id, model, attribution, workspace_id, space_id, account_id, backend,
            usage_date, input_tokens, output_tokens, cached_input_tokens,
            estimated_cost, currency, reported_cost, reported_currency, confidence, updated_at
        )
        SELECT run.id, $2, 'reported', run.workspace_id, run.space_id, run.account_id,
               left($3, 80), $4, $5, $6, $7, $8, $9, $10,
               CASE WHEN $10::double precision IS NOT NULL THEN 'USD' END, 'exact', now()
          FROM runs run
         WHERE run.id = $1
        ON CONFLICT (run_id, model, attribution) DO UPDATE SET
            input_tokens = run_usage.input_tokens + EXCLUDED.input_tokens,
            output_tokens = run_usage.output_tokens + EXCLUDED.output_tokens,
            cached_input_tokens = run_usage.cached_input_tokens + EXCLUDED.cached_input_tokens,
            estimated_cost = CASE
                WHEN EXCLUDED.estimated_cost IS NULL THEN run_usage.estimated_cost
                ELSE COALESCE(run_usage.estimated_cost, 0) + EXCLUDED.estimated_cost END,
            currency = COALESCE(EXCLUDED.currency, run_usage.currency),
            reported_cost = CASE
                WHEN EXCLUDED.reported_cost IS NULL THEN run_usage.reported_cost
                ELSE COALESCE(run_usage.reported_cost, 0) + EXCLUDED.reported_cost END,
            reported_currency = COALESCE(EXCLUDED.reported_currency, run_usage.reported_currency),
            updated_at = now()
        "#,
    )
    .bind(run_id)
    .bind(&tokens.model)
    .bind(&tokens.backend)
    .bind(tokens.date)
    .bind(tokens.input_tokens)
    .bind(tokens.output_tokens)
    .bind(tokens.cached_input_tokens)
    .bind(tokens.estimated_cost)
    .bind(&tokens.currency)
    .bind(tokens.reported_cost_usd)
    .execute(executor)
    .await?;
    Ok(())
}

/// Add one reported amount to a run's usage for its model.
pub async fn add_reported(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    model: &str,
    input: &UsageInput,
    currency: Option<&str>,
) -> Result<(), RunOpError> {
    // Costs in two currencies don't add up: keep one per run and model.
    if let Some(currency) = currency
        && input.cost.is_some()
    {
        let held = sqlx::query_scalar::<_, Option<String>>(
            "SELECT reported_currency FROM run_usage WHERE run_id = $1 AND model = $2 AND attribution = 'reported'",
        )
        .bind(run.id)
        .bind(model)
        .fetch_optional(&mut **transaction)
        .await?
        .flatten();
        if held.is_some_and(|held| held != currency) {
            return Err(RunOpError::Refused {
                field: "currency",
                message: "This run's cost for that model is already in another currency; report it in the same one.".to_owned(),
            });
        }
    }
    sqlx::query(
        r#"
        INSERT INTO run_usage (
            run_id, model, attribution, workspace_id, space_id, account_id, host_id,
            backend, usage_date, input_tokens, output_tokens, cached_input_tokens,
            reported_cost, reported_currency, confidence
        )
        VALUES ($1, $2, 'reported', $3, $4, $5, $6, $7, (now() AT TIME ZONE 'UTC')::date,
                $8, $9, $10, $11, $12, 'exact')
        ON CONFLICT (run_id, model, attribution) DO UPDATE SET
            input_tokens = run_usage.input_tokens + EXCLUDED.input_tokens,
            output_tokens = run_usage.output_tokens + EXCLUDED.output_tokens,
            cached_input_tokens = run_usage.cached_input_tokens + EXCLUDED.cached_input_tokens,
            reported_cost = CASE
                WHEN EXCLUDED.reported_cost IS NULL THEN run_usage.reported_cost
                ELSE COALESCE(run_usage.reported_cost, 0) + EXCLUDED.reported_cost END,
            reported_currency = COALESCE(run_usage.reported_currency, EXCLUDED.reported_currency),
            updated_at = now()
        "#,
    )
    .bind(run.id)
    .bind(model)
    .bind(run.workspace_id)
    .bind(run.space_id)
    .bind(run.account_id)
    .bind(run.host_id)
    .bind(&run.backend)
    .bind(input.input_tokens)
    .bind(input.output_tokens)
    .bind(input.cached_input_tokens)
    .bind(input.cost)
    .bind(currency)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ------------------------------------------------------- A1.15: windows

/// The job that splits window reports onto runs.
pub const ATTRIBUTE_KIND: &str = "runs.attribute_window_usage";
/// Window reports older than this are no longer split (one that found no
/// run by then stays "not tied to a run").
const ATTRIBUTE_DAYS: i32 = 30;
/// Reports split per pass.
const ATTRIBUTE_BATCH: i64 = 200;

#[derive(Debug, FromRow)]
struct WindowReport {
    id: Uuid,
    workspace_id: Uuid,
    account_id: Uuid,
    host_id: Option<Uuid>,
    model: String,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    reported_cost: Option<f64>,
    reported_currency: Option<String>,
}

#[derive(Debug, FromRow)]
struct Overlap {
    run_id: Uuid,
    space_id: Option<Uuid>,
    host_id: Option<Uuid>,
    backend: Option<String>,
    /// Seconds of the window the run was going.
    seconds: f64,
}

/// `total` split in proportion to `weights`, in whole units that add up
/// to it: each share rounded down, the rest one each to the largest
/// remainders (the earlier run on a tie).
pub fn split(total: i64, weights: &[f64]) -> Vec<i64> {
    let sum: f64 = weights.iter().sum();
    if total <= 0 || sum <= 0.0 {
        return vec![0; weights.len()];
    }
    let exact = weights
        .iter()
        .map(|weight| total as f64 * weight / sum)
        .collect::<Vec<_>>();
    let mut shares = exact
        .iter()
        .map(|share| share.floor() as i64)
        .collect::<Vec<_>>();
    let mut left = total - shares.iter().sum::<i64>();
    let mut order = (0..weights.len()).collect::<Vec<_>>();
    order.sort_by(|a, b| {
        let remainder = |index: usize| exact[index] - exact[index].floor();
        remainder(*b)
            .partial_cmp(&remainder(*a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(b))
    });
    for index in order {
        if left <= 0 {
            break;
        }
        shares[index] += 1;
        left -= 1;
    }
    shares
}

/// Split the window reports nobody tied to a run across the reporting
/// account's runs (on the report's host, when it names one) that were going
/// during the window, in proportion to how long each overlapped it, as
/// `attribution = 'window'`, `confidence = 'estimated'`. A report is split
/// once; one that overlaps no run stays unattributed and is looked at again
/// on later passes (a run reported late may still claim it). Returns how
/// many reports were split.
pub async fn attribute_window_usage(pool: &PgPool) -> Result<u64, AppError> {
    // Splits reported usage across runs in every workspace.
    crate::tenancy::system("runs.host", attribute_window_usage_here(pool)).await
}

async fn attribute_window_usage_here(pool: &PgPool) -> Result<u64, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let reports = sqlx::query_as::<_, WindowReport>(
        r#"
        SELECT id, workspace_id, account_id, host_id, model, window_start, window_end,
               input_tokens, output_tokens, cached_input_tokens, reported_cost, reported_currency
        FROM usage_reports
        WHERE run_id IS NULL AND attributed_at IS NULL
          AND window_end <= now() AND window_end > now() - make_interval(days => $1)
        ORDER BY created_at, id
        LIMIT $2
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(ATTRIBUTE_DAYS)
    .bind(ATTRIBUTE_BATCH)
    .fetch_all(&mut *transaction)
    .await?;
    let mut split_count = 0;
    for report in &reports {
        let overlaps = sqlx::query_as::<_, Overlap>(
            r#"
            SELECT run.id AS run_id, run.space_id, run.host_id, run.backend,
                   EXTRACT(EPOCH FROM LEAST($5, COALESCE(run.finished_at, now()))
                                      - GREATEST($4, run.started_at))::double precision AS seconds
            FROM runs run
            WHERE run.account_id = $1 AND run.workspace_id = $2
              AND ($3::uuid IS NULL OR run.host_id = $3)
              AND run.started_at IS NOT NULL
              AND run.started_at < $5 AND COALESCE(run.finished_at, now()) > $4
            ORDER BY run.started_at, run.id
            "#,
        )
        .bind(report.account_id)
        .bind(report.workspace_id)
        .bind(report.host_id)
        .bind(report.window_start)
        .bind(report.window_end)
        .fetch_all(&mut *transaction)
        .await?;
        let overlaps = overlaps
            .into_iter()
            .filter(|overlap| overlap.seconds > 0.0)
            .collect::<Vec<_>>();
        if overlaps.is_empty() {
            continue;
        }
        let weights = overlaps
            .iter()
            .map(|overlap| overlap.seconds)
            .collect::<Vec<_>>();
        let sum: f64 = weights.iter().sum();
        let inputs = split(report.input_tokens, &weights);
        let outputs = split(report.output_tokens, &weights);
        let cached = split(report.cached_input_tokens, &weights);
        for (index, overlap) in overlaps.iter().enumerate() {
            let cost = report.reported_cost.map(|cost| cost * weights[index] / sum);
            sqlx::query(
                r#"
                INSERT INTO run_usage (
                    run_id, model, attribution, workspace_id, space_id, account_id, host_id,
                    backend, usage_date, input_tokens, output_tokens, cached_input_tokens,
                    reported_cost, reported_currency, confidence
                )
                VALUES ($1, $2, 'window', $3, $4, $5, $6, $7, ($8 AT TIME ZONE 'UTC')::date,
                        $9, $10, $11, $12, $13, 'estimated')
                ON CONFLICT (run_id, model, attribution) DO UPDATE SET
                    input_tokens = run_usage.input_tokens + EXCLUDED.input_tokens,
                    output_tokens = run_usage.output_tokens + EXCLUDED.output_tokens,
                    cached_input_tokens = run_usage.cached_input_tokens + EXCLUDED.cached_input_tokens,
                    -- Costs in two currencies don't add up: the first one stays.
                    reported_cost = CASE
                        WHEN EXCLUDED.reported_cost IS NULL THEN run_usage.reported_cost
                        WHEN run_usage.reported_currency IS NOT NULL
                             AND run_usage.reported_currency <> EXCLUDED.reported_currency
                            THEN run_usage.reported_cost
                        ELSE COALESCE(run_usage.reported_cost, 0) + EXCLUDED.reported_cost END,
                    reported_currency = COALESCE(run_usage.reported_currency, EXCLUDED.reported_currency),
                    confidence = 'estimated',
                    updated_at = now()
                "#,
            )
            .bind(overlap.run_id)
            .bind(&report.model)
            .bind(report.workspace_id)
            .bind(overlap.space_id)
            .bind(report.account_id)
            .bind(overlap.host_id.or(report.host_id))
            .bind(&overlap.backend)
            .bind(report.window_end)
            .bind(inputs[index])
            .bind(outputs[index])
            .bind(cached[index])
            .bind(cost)
            .bind(cost.and(report.reported_currency.as_deref()))
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query("UPDATE usage_reports SET attributed_at = now() WHERE id = $1")
            .bind(report.id)
            .execute(&mut *transaction)
            .await?;
        split_count += 1;
    }
    transaction.commit().await?;
    Ok(split_count)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        ATTRIBUTE_KIND,
        Duration::from_secs(120),
        |context: JobContext| async move {
            attribute_window_usage(&context.state.pool)
                .await
                .map_err(|error| JobError::retry(error.log_category()))?;
            Ok(())
        },
    );
    registry.every(ATTRIBUTE_KIND, Duration::from_secs(5 * 60));
}

// ------------------------------------------------------- A1.15: totals

/// What usage to add up: a workspace's, narrowed to a space or an account,
/// from a day on.
#[derive(Clone, Copy, Debug)]
pub struct TotalsScope {
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub since: NaiveDate,
}

/// Usage added up from `run_usage`, the one place budgets (A2) and
/// metering (H1) read totals from; they never read raw reports.
#[derive(Clone, Debug, Default, FromRow, PartialEq)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    /// Reported cost where there is one, else the catalog's estimate.
    pub cost: Option<f64>,
    pub currency: Option<String>,
    /// Some of it was split from a window rather than reported per run.
    pub estimated: bool,
}

pub async fn totals(
    executor: impl PgExecutor<'_>,
    scope: TotalsScope,
) -> Result<UsageTotals, AppError> {
    Ok(sqlx::query_as::<_, UsageTotals>(
        r#"
        SELECT COALESCE(sum(input_tokens), 0)::bigint AS input_tokens,
               COALESCE(sum(output_tokens), 0)::bigint AS output_tokens,
               COALESCE(sum(cached_input_tokens), 0)::bigint AS cached_input_tokens,
               sum(COALESCE(reported_cost, estimated_cost)) AS cost,
               min(COALESCE(reported_currency, currency)) AS currency,
               COALESCE(bool_or(confidence = 'estimated'), false) AS estimated
        FROM run_usage
        WHERE workspace_id = $1
          AND ($2::uuid IS NULL OR space_id = $2)
          AND ($3::uuid IS NULL OR account_id = $3)
          AND usage_date >= $4
        "#,
    )
    .bind(scope.workspace_id)
    .bind(scope.space_id)
    .bind(scope.account_id)
    .bind(scope.since)
    .fetch_one(executor)
    .await?)
}

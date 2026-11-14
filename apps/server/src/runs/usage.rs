//! A1.3: usage on a run. A reporter's usage for one of its runs adds to the
//! run's totals per model as exact (`attribution = 'reported'`). The
//! built-in host's usage still lands in `agent_run_usage`, mirrored here by
//! a trigger until A1.22; window reports are split onto runs by A1.15.

use sqlx::{Postgres, Transaction};

use super::{
    ops::{RunOpError, UsageInput},
    store::RunRow,
};

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

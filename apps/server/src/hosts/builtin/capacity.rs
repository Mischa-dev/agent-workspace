//! A1.6: the built-in host's load on provider accounts and its agents'
//! workers: how many runs hold or wait for an account's slots, which runs an
//! agent's worker has, and what an account ran in a window. Only the
//! built-in host runs on provider accounts, so these read its detail rows.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::{error::AppError, runs::ACTIVE_RUN_STATES};

/// The built-in states that hold a slot on an account (the active ones,
/// less those still waiting for a slot).
fn running_states() -> Vec<&'static str> {
    ACTIVE_RUN_STATES
        .into_iter()
        .filter(|state| !matches!(*state, "created" | "queued"))
        .collect()
}

/// How many runs are running on the account `account` (a column of the
/// caller's query): a scalar expression for a list of accounts.
pub fn running_on_account(account: &str) -> String {
    let states = running_states()
        .into_iter()
        .map(|state| format!("'{state}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "(SELECT count(*) FROM agent_runs run \
          JOIN execution_profiles run_profile ON run_profile.id = run.execution_profile_id \
          WHERE run_profile.provider_account_id = {account} AND run.state IN ({states}))"
    )
}

/// Runs holding or waiting for one of the account's slots.
pub async fn account_load(
    executor: impl PgExecutor<'_>,
    account_id: Uuid,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM agent_runs run
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        WHERE profile.provider_account_id = $1 AND run.state = ANY($2)
        "#,
    )
    .bind(account_id)
    .bind(&ACTIVE_RUN_STATES[..])
    .fetch_one(executor)
    .await?)
}

/// Runs the account started in `[from, to)`.
pub async fn account_runs_between(
    executor: impl PgExecutor<'_>,
    account_id: Uuid,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM agent_runs run
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        WHERE profile.provider_account_id = $1 AND run.created_at >= $2 AND run.created_at < $3
        "#,
    )
    .bind(account_id)
    .bind(from)
    .bind(to)
    .fetch_one(executor)
    .await?)
}

/// What the account ran per project over the last `days` days: the project
/// (named only where `viewer` can open it), runs, cost and currency, most
/// runs first.
pub async fn account_runs_by_project(
    executor: impl PgExecutor<'_>,
    account_id: Uuid,
    viewer: Uuid,
    days: i64,
) -> Result<
    Vec<(
        Option<Uuid>,
        Option<String>,
        i64,
        Option<f64>,
        Option<String>,
    )>,
    AppError,
> {
    Ok(sqlx::query_as(
        r#"
        SELECT CASE WHEN actor_can_access_project($2, NULL, project.id) THEN project.id END,
               CASE WHEN actor_can_access_project($2, NULL, project.id) THEN project.name END,
               count(*),
               sum(usage.cost),
               min(usage.currency)
        FROM agent_runs run
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN conversations conversation ON conversation.id = run.conversation_id
        JOIN projects project ON project.id = conversation.project_id
        LEFT JOIN LATERAL (
            SELECT sum(COALESCE(estimated_cost, reported_cost)) AS cost,
                   min(COALESCE(CASE WHEN estimated_cost IS NOT NULL THEN currency
                                     ELSE reported_currency END, 'USD')) AS currency
            FROM run_usage
            WHERE run_id = run.id
        ) usage ON true
        -- visibility: an account's owner sees what it ran; each project is
        -- named only where the viewer can open it, and nothing of a
        -- conversation is returned.
        WHERE profile.provider_account_id = $1
          AND run.created_at >= now() - make_interval(days => $3::int)
        GROUP BY 1, 2
        ORDER BY 3 DESC
        LIMIT 50
        "#,
    )
    .bind(account_id)
    .bind(viewer)
    .bind(days)
    .fetch_all(executor)
    .await?)
}

/// Whether any run, not yet finished, uses the account.
pub async fn account_in_use(
    executor: impl PgExecutor<'_>,
    account_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs run
            JOIN execution_profiles profile ON profile.id = run.execution_profile_id
            WHERE profile.provider_account_id = $1 AND run.state = ANY($2)
        )
        "#,
    )
    .bind(account_id)
    .bind(&ACTIVE_RUN_STATES[..])
    .fetch_one(executor)
    .await?)
}

/// Whether an agent's worker has a run that hasn't finished, and whether it
/// ever had one.
pub async fn worker_runs(
    executor: impl PgExecutor<'_>,
    worker_id: Uuid,
) -> Result<(bool, bool), AppError> {
    Ok(sqlx::query_as::<_, (bool, bool)>(
        r#"
        SELECT
            EXISTS (
                SELECT 1 FROM agent_runs
                WHERE execution_worker_id = $1 AND state = ANY($2)
            ),
            EXISTS (SELECT 1 FROM agent_runs WHERE execution_worker_id = $1)
        "#,
    )
    .bind(worker_id)
    .bind(&ACTIVE_RUN_STATES[..])
    .fetch_one(executor)
    .await?)
}

/// What each agent's worker in project `$1` is running now, blocked ones
/// marked waiting: a `SELECT` of `actor_id`, `conversation_id`, `run_id`,
/// `waiting`, `since`, for the caller's union of what agents are doing.
pub fn working_workers() -> String {
    format!(
        "SELECT worker.actor_id, run.conversation_id, run.id AS run_id, \
                run.state = 'blocked' AS waiting, COALESCE(run.started_at, run.created_at) AS since \
         FROM agent_runs run \
         JOIN execution_workers worker ON worker.id = run.execution_worker_id \
         WHERE worker.project_id = $1 AND run.state IN {}",
        crate::runs::sql::builtin_active_states()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_states_leave_out_waiting_for_a_slot() {
        let running = running_states();
        assert!(!running.contains(&"queued") && !running.contains(&"created"));
        assert!(running.contains(&"working") && running.contains(&"blocked"));
    }
}

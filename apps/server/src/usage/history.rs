//! U5: each account's usage over time, for the usage page.
//!
//! Every reading leaves a sample (at most one every few minutes per window
//! unless the number moves) and keeps one row per window cycle with the most
//! it reached before resetting, so the page can say how much each plan is
//! used and how much went unused. Runs are counted per cycle and per
//! project; cost per project comes from the run usage records (U2), and the
//! view says when an account has none yet.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{UsageAccount, WindowSource, WindowView, window_label};
use crate::error::AppError;

/// A window with no new number gets a sample this often.
const SAMPLE_MINUTES: i64 = 10;
/// How far back the chart goes.
const CHART_DAYS: i64 = 14;
/// The most points one window's chart returns.
const MAX_POINTS: usize = 240;
/// Past cycles listed per window.
const CYCLES_SHOWN: i64 = 8;
/// A drop this big inside a cycle means it was refilled (a banked reset).
const REFILL_DROP: i16 = 10;
/// Kept for the chart, and for the cycle list.
const SAMPLE_RETENTION_DAYS: i64 = 35;
const CYCLE_RETENTION_DAYS: i64 = 400;
/// Runs per project are counted over this many days.
const PROJECT_DAYS: i64 = 30;

/// Record one reading of one window, inside the transaction that stores it.
pub(crate) async fn record(
    transaction: &mut Transaction<'_, Postgres>,
    account: &UsageAccount,
    window_key: &str,
    duration_minutes: Option<i32>,
    used_percent: i16,
    resets_at: Option<DateTime<Utc>>,
) -> Result<(), AppError> {
    sqlx::query(&format!(
        r#"
        INSERT INTO usage_window_samples (account_id, organization_id, window_key, used_percent, resets_at)
        SELECT $1, $2, $3, $4, $5
        WHERE NOT EXISTS (
            SELECT 1 FROM usage_window_samples sample
            WHERE sample.account_id = $1 AND sample.window_key = $3
              AND sample.sampled_at > now() - interval '{SAMPLE_MINUTES} minutes'
              AND sample.used_percent = $4
        )
        ON CONFLICT DO NOTHING
        "#
    ))
    .bind(account.id)
    .bind(account.organization_id)
    .bind(window_key)
    .bind(used_percent)
    .bind(resets_at)
    .execute(&mut **transaction)
    .await?;
    let Some(resets_at) = resets_at else {
        return Ok(());
    };
    // The cycle goes on while its reset hasn't passed and nothing refilled
    // it; a window whose reset time drifts between readings stays one cycle.
    let current = sqlx::query_as::<_, (Uuid, DateTime<Utc>, i16)>(
        r#"
        SELECT id, resets_at, last_used_percent FROM usage_window_cycles
        WHERE account_id = $1 AND window_key = $2
        ORDER BY resets_at DESC LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(account.id)
    .bind(window_key)
    .fetch_optional(&mut **transaction)
    .await?;
    let now = Utc::now();
    match current {
        Some((id, cycle_resets_at, last))
            if now < cycle_resets_at + Duration::minutes(2)
                && used_percent + REFILL_DROP > last =>
        {
            sqlx::query(
                r#"
                UPDATE usage_window_cycles
                SET resets_at = $2, peak_used_percent = GREATEST(peak_used_percent, $3),
                    last_used_percent = $3, last_read_at = now(),
                    duration_minutes = COALESCE($4, duration_minutes)
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(resets_at)
            .bind(used_percent)
            .bind(duration_minutes)
            .execute(&mut **transaction)
            .await?;
        }
        _ => {
            sqlx::query(
                r#"
                INSERT INTO usage_window_cycles (
                    id, account_id, organization_id, window_key, duration_minutes, resets_at,
                    peak_used_percent, last_used_percent
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(account.id)
            .bind(account.organization_id)
            .bind(window_key)
            .bind(duration_minutes)
            .bind(resets_at)
            .bind(used_percent)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(())
}

/// Drop history past its keep-by date.
pub(crate) async fn prune(pool: &PgPool) -> Result<(), AppError> {
    sqlx::query(&format!(
        "DELETE FROM usage_window_samples WHERE sampled_at < now() - interval '{SAMPLE_RETENTION_DAYS} days'"
    ))
    .execute(pool)
    .await?;
    sqlx::query(&format!(
        "DELETE FROM usage_window_cycles WHERE resets_at < now() - interval '{CYCLE_RETENTION_DAYS} days'"
    ))
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The view

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub at: DateTime<Utc>,
    pub used_percent: u16,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cycle {
    pub started_at: Option<DateTime<Utc>>,
    pub resets_at: DateTime<Utc>,
    /// Still running: its numbers can grow.
    pub current: bool,
    /// `None` for a schedule's window, whose use nobody reports.
    pub peak_used_percent: Option<u16>,
    pub unused_percent: Option<u16>,
    /// Runs on this account that started in the cycle.
    pub runs: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowHistory {
    pub key: String,
    pub label: String,
    pub source: WindowSource,
    pub points: Vec<Point>,
    /// Newest first.
    pub cycles: Vec<Cycle>,
    /// What went unused in the last finished cycle.
    pub last_cycle_unused_percent: Option<u16>,
    /// The average unused across the finished cycles listed.
    pub average_unused_percent: Option<u16>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUse {
    /// `None` for runs in projects the reader can no longer open.
    pub project_id: Option<Uuid>,
    pub project_name: String,
    pub runs: i64,
    /// Total cost in the account's currency, once runs record it.
    pub cost: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountHistory {
    pub account_id: Uuid,
    pub windows: Vec<WindowHistory>,
    /// Runs per project over the last `project_days` days.
    pub projects: Vec<ProjectUse>,
    pub project_days: i64,
    /// Whether runs on this account record their cost yet.
    pub cost_tracked: bool,
}

#[derive(FromRow)]
struct CycleRow {
    window_key: String,
    duration_minutes: Option<i32>,
    resets_at: DateTime<Utc>,
    peak_used_percent: i16,
}

async fn runs_between(
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

/// How much of the window one run used, on average over its finished
/// cycles that had runs. `None` until there is such a cycle.
pub(crate) async fn percent_per_run(
    pool: &PgPool,
    account_id: Uuid,
    window_key: &str,
) -> Result<Option<f64>, AppError> {
    let rows = sqlx::query_as::<_, (DateTime<Utc>, i32, i16)>(&format!(
        r#"
        SELECT resets_at, duration_minutes, peak_used_percent FROM usage_window_cycles
        WHERE account_id = $1 AND window_key = $2 AND resets_at <= now()
          AND duration_minutes IS NOT NULL AND peak_used_percent > 0
        ORDER BY resets_at DESC LIMIT {CYCLES_SHOWN}
        "#
    ))
    .bind(account_id)
    .bind(window_key)
    .fetch_all(pool)
    .await?;
    let (mut used, mut runs) = (0_f64, 0_i64);
    for (resets_at, minutes, peak) in rows {
        let count = runs_between(
            pool,
            account_id,
            resets_at - Duration::minutes(i64::from(minutes)),
            resets_at,
        )
        .await?;
        if count > 0 {
            used += f64::from(peak);
            runs += count;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    Ok((runs > 0).then(|| used / runs as f64))
}

/// Keep at most `MAX_POINTS`, evenly spread, always with the last.
fn thin(points: Vec<Point>) -> Vec<Point> {
    if points.len() <= MAX_POINTS {
        return points;
    }
    let step = points.len().div_ceil(MAX_POINTS);
    let last = points.len() - 1;
    points
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % step == 0 || *index == last)
        .map(|(_, point)| point)
        .collect()
}

fn unused(peak: i16) -> u16 {
    100 - u16::try_from(peak.clamp(0, 100)).unwrap_or(100)
}

/// The account's history, as its owner `viewer` sees it. `windows` are its
/// current windows (from `windows_at`), which give the labels and order.
pub(crate) async fn account_history(
    pool: &PgPool,
    viewer: Uuid,
    account_id: Uuid,
    windows: &[WindowView],
    now: DateTime<Utc>,
) -> Result<AccountHistory, AppError> {
    let since = now - Duration::days(CHART_DAYS);
    let samples = sqlx::query_as::<_, (String, DateTime<Utc>, i16)>(
        r#"
        SELECT window_key, sampled_at, used_percent FROM usage_window_samples
        WHERE account_id = $1 AND sampled_at >= $2
        ORDER BY window_key, sampled_at
        "#,
    )
    .bind(account_id)
    .bind(since)
    .fetch_all(pool)
    .await?;
    let mut points: BTreeMap<String, Vec<Point>> = BTreeMap::new();
    for (key, at, used) in samples {
        points.entry(key).or_default().push(Point {
            at,
            used_percent: u16::try_from(used.clamp(0, 100)).unwrap_or(0),
        });
    }
    let cycle_rows = sqlx::query_as::<_, CycleRow>(&format!(
        r#"
        SELECT window_key, duration_minutes, resets_at, peak_used_percent FROM (
            SELECT cycle.*, row_number() OVER (
                       PARTITION BY window_key ORDER BY resets_at DESC) AS position
            FROM usage_window_cycles cycle WHERE account_id = $1
        ) ranked
        WHERE position <= {CYCLES_SHOWN}
        ORDER BY window_key, resets_at DESC
        "#
    ))
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    let mut cycles: BTreeMap<String, Vec<CycleRow>> = BTreeMap::new();
    for row in cycle_rows {
        cycles.entry(row.window_key.clone()).or_default().push(row);
    }

    // Current windows first, in their order, then ones no longer reported.
    let mut keys = windows
        .iter()
        .map(|window| window.key.clone())
        .collect::<Vec<_>>();
    for key in cycles.keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    let mut histories = Vec::with_capacity(keys.len());
    for key in keys {
        let window = windows.iter().find(|window| window.key == key);
        let rows = cycles.remove(&key).unwrap_or_default();
        let mut list = Vec::new();
        for row in &rows {
            let length = row
                .duration_minutes
                .map(|minutes| Duration::minutes(i64::from(minutes)));
            let started_at = length.map(|length| row.resets_at - length);
            let runs = match started_at {
                Some(start) => Some(runs_between(pool, account_id, start, row.resets_at).await?),
                None => None,
            };
            list.push(Cycle {
                started_at,
                resets_at: row.resets_at,
                current: row.resets_at > now,
                peak_used_percent: Some(100 - unused(row.peak_used_percent)),
                unused_percent: Some(unused(row.peak_used_percent)),
                runs,
            });
        }
        // A schedule's window has no reported use, but its runs still count.
        if let Some(window) = window.filter(|window| window.source == WindowSource::Schedule)
            && let (Some(resets_at), Some(minutes)) = (window.resets_at, window.duration_minutes)
        {
            let start = resets_at - Duration::minutes(minutes);
            list.push(Cycle {
                started_at: Some(start),
                resets_at,
                current: true,
                peak_used_percent: None,
                unused_percent: None,
                runs: Some(runs_between(pool, account_id, start, resets_at).await?),
            });
        }
        let finished = list
            .iter()
            .filter(|cycle| !cycle.current)
            .filter_map(|cycle| cycle.unused_percent)
            .collect::<Vec<_>>();
        let average = (!finished.is_empty()).then(|| {
            let total = finished.iter().map(|value| u32::from(*value)).sum::<u32>();
            u16::try_from(total / u32::try_from(finished.len()).unwrap_or(1)).unwrap_or(0)
        });
        histories.push(WindowHistory {
            label: window.map_or_else(
                || {
                    window_label(
                        rows.first()
                            .and_then(|row| row.duration_minutes)
                            .map(i64::from),
                    )
                },
                |window| window.label.clone(),
            ),
            source: window.map_or(WindowSource::Reported, |window| window.source),
            points: thin(points.remove(&key).unwrap_or_default()),
            last_cycle_unused_percent: finished.first().copied(),
            average_unused_percent: average,
            cycles: list,
            key,
        });
    }

    // Cost comes from W12's per-run usage records (U2), in each record's
    // currency; mixed currencies are rare enough to report as USD.
    let projects = sqlx::query_as::<
        _,
        (
            Option<Uuid>,
            Option<String>,
            i64,
            Option<f64>,
            Option<String>,
        ),
    >(&format!(
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
            SELECT sum(COALESCE(estimated_cost, reported_cost_usd)) AS cost,
                   min(COALESCE(currency, 'USD')) AS currency
            FROM agent_run_usage
            WHERE agent_run_id = run.id
        ) usage ON true
        WHERE profile.provider_account_id = $1
          AND run.created_at >= now() - interval '{PROJECT_DAYS} days'
        GROUP BY 1, 2
        ORDER BY 3 DESC
        LIMIT 50
        "#
    ))
    .bind(account_id)
    .bind(viewer)
    .fetch_all(pool)
    .await?;
    let mut uses = Vec::new();
    let mut hidden = 0;
    for (project_id, name, runs, cost, currency) in projects {
        match (project_id, name) {
            (Some(project_id), Some(name)) => uses.push(ProjectUse {
                project_id: Some(project_id),
                project_name: name,
                runs,
                currency: cost.and(currency),
                cost,
            }),
            _ => hidden += runs,
        }
    }
    let cost_tracked = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM agent_run_usage WHERE provider_account_id = $1)",
    )
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    if hidden > 0 {
        uses.push(ProjectUse {
            project_id: None,
            project_name: "Projects you can no longer open".to_owned(),
            runs: hidden,
            cost: None,
            currency: None,
        });
    }
    Ok(AccountHistory {
        account_id,
        windows: histories,
        projects: uses,
        project_days: PROJECT_DAYS,
        cost_tracked,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{MAX_POINTS, Point, thin, unused};

    #[test]
    fn thinning_keeps_the_shape_and_the_last_point() {
        let start = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
        let points = (0..1000)
            .map(|minute| Point {
                at: start + chrono::Duration::minutes(minute),
                used_percent: u16::try_from(minute / 10).expect("small"),
            })
            .collect::<Vec<_>>();
        let thinned = thin(points);
        assert!(thinned.len() <= MAX_POINTS + 1);
        assert_eq!(thinned.first().expect("first").used_percent, 0);
        assert_eq!(thinned.last().expect("last").used_percent, 99);
    }

    #[test]
    fn unused_is_what_the_peak_left() {
        assert_eq!(unused(62), 38);
        assert_eq!(unused(100), 0);
        assert_eq!(unused(-3), 100);
    }
}

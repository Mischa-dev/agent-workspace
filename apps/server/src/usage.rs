//! U1, U6: usage windows and banked resets for provider accounts.
//!
//! Every account has whichever windows its harness reports through
//! [`HarnessSession::read_usage`] (a five-hour session, a week, a month of
//! credits), so a plan with only a weekly limit just has one window. The
//! `usage.poll_due` job finds accounts due for a read and queues one
//! `usage.poll_account` job each; that job reads through a control session,
//! which never takes a run slot (F8). Runs also report usage as they go
//! ([`record_run_report`]). Accounts whose harness reports nothing get
//! windows from reset schedules their owner sets; those are computed when
//! read.
//!
//! After each reading the capacity alert rules run ([`alerts`]). Everything
//! here is the account owner's alone: other people, even in projects the
//! account serves, never see its usage.

pub mod alerts;
pub mod ready;
pub mod schedule;

use std::{collections::BTreeSet, path::PathBuf, time::Duration as StdDuration};

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{delete, get, post, put},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::IdempotencyKey,
    jobs::{self, JobError, JobRegistry, NewJob},
    provider::{
        AccountHandle, HarnessError,
        adapter::{ResetOutcome, UsageReport},
    },
};

pub const POLL_DUE_KIND: &str = "usage.poll_due";
pub const POLL_ACCOUNT_KIND: &str = "usage.poll_account";
pub const EVALUATE_KIND: &str = "usage.evaluate";
/// How often an account is read when nothing is close.
const POLL_MINUTES: i64 = 15;
/// How often when a window resets within six hours or is nearly full.
const CLOSE_POLL_MINUTES: i64 = 5;
const MAX_SCHEDULES_PER_ACCOUNT: i64 = 10;

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        POLL_ACCOUNT_KIND,
        StdDuration::from_secs(120),
        |context| async move {
            let job: AccountJob = context.job.payload_as()?;
            poll_account(&context.state, job.account_id).await
        },
    );
    registry.register(EVALUATE_KIND, |context| async move {
        let job: AccountJob = context.job.payload_as()?;
        alerts::evaluate(&context.state, job.account_id, Utc::now()).await?;
        Ok(())
    });
    registry.register(POLL_DUE_KIND, |context| async move {
        enqueue_due(&context.state.pool).await?;
        Ok(())
    });
    registry.every(POLL_DUE_KIND, StdDuration::from_secs(5 * 60));
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountJob {
    account_id: Uuid,
}

// ---------------------------------------------------------------------------
// Accounts

#[derive(Clone, Debug, FromRow)]
pub(crate) struct UsageAccount {
    pub(crate) id: Uuid,
    pub(crate) organization_id: Uuid,
    pub(crate) owner_actor_id: Uuid,
    pub(crate) adapter_kind: String,
    pub(crate) name: String,
    home_path: String,
    max_concurrent_runs: i16,
    status: String,
    account_state: String,
}

impl UsageAccount {
    fn handle(&self) -> AccountHandle {
        AccountHandle {
            id: self.id,
            home: PathBuf::from(&self.home_path),
            max_concurrent_runs: u16::try_from(self.max_concurrent_runs).unwrap_or(1),
        }
    }

    /// Signed in and not switched off: its harness can be asked.
    fn readable(&self) -> bool {
        self.account_state == "authenticated" && self.status != "disabled"
    }
}

const ACCOUNT_COLUMNS: &str = r#"
    account.id, account.organization_id, account.owner_actor_id, account.adapter_kind,
    account.name, account.home_path, account.max_concurrent_runs, account.status,
    account.account_state
"#;

pub(crate) async fn load_account<'e, E: PgExecutor<'e>>(
    executor: E,
    account_id: Uuid,
) -> Result<Option<UsageAccount>, AppError> {
    Ok(sqlx::query_as::<_, UsageAccount>(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM provider_accounts account WHERE account.id = $1"
    ))
    .bind(account_id)
    .fetch_optional(executor)
    .await?)
}

/// The account, if `viewer` owns it and is still an enabled member of its
/// workspace. Anything else is "not found", so ids don't leak.
pub(crate) async fn owned_account<'e, E: PgExecutor<'e>>(
    executor: E,
    viewer: Uuid,
    account_id: Uuid,
) -> Result<UsageAccount, AppError> {
    sqlx::query_as::<_, UsageAccount>(&format!(
        r#"
        SELECT {ACCOUNT_COLUMNS}
        FROM provider_accounts account
        JOIN organization_memberships membership
          ON membership.organization_id = account.organization_id
         AND membership.actor_id = account.owner_actor_id AND membership.enabled
        JOIN actors actor ON actor.id = account.owner_actor_id AND actor.status = 'active'
        WHERE account.id = $1 AND account.owner_actor_id = $2
        "#
    ))
    .bind(account_id)
    .bind(viewer)
    .fetch_optional(executor)
    .await?
    .ok_or(AppError::NotFound("provider_account"))
}

// ---------------------------------------------------------------------------
// Reading and storing

/// Read the account's usage through a control session and store it.
/// Returns whether the harness answered.
async fn read_and_store(state: &AppState, account: &UsageAccount) -> Result<bool, JobError> {
    let result = async {
        let mut session = state
            .supervisors
            .acquire_control(&account.adapter_kind, account.handle())
            .await?;
        session.read_usage().await
    }
    .await;
    match result {
        Ok(report) => {
            let mut transaction = state.pool.begin().await?;
            store_report(&mut transaction, account, &report, true).await?;
            transaction.commit().await?;
            Ok(true)
        }
        // Every control slot is busy with account pages: try again soon.
        Err(HarnessError::ControlCapacity) => Err(JobError::retry("control sessions busy")),
        Err(error) => {
            let category = failure_category(&error);
            tracing::info!(account_id = %account.id, category, "usage read failed");
            sqlx::query(
                r#"
                INSERT INTO usage_accounts (account_id, organization_id, poll_failed_at, poll_failure)
                VALUES ($1, $2, now(), $3)
                ON CONFLICT (account_id) DO UPDATE
                SET poll_failed_at = now(), poll_failure = EXCLUDED.poll_failure, updated_at = now()
                "#,
            )
            .bind(account.id)
            .bind(account.organization_id)
            .bind(category)
            .execute(&state.pool)
            .await?;
            Ok(false)
        }
    }
}

fn failure_category(error: &HarnessError) -> &'static str {
    match error {
        HarnessError::HostCapacity | HarnessError::AccountCapacity => "busy",
        HarnessError::ControlCapacity => "busy",
        HarnessError::ShuttingDown => "shutting_down",
        HarnessError::CircuitOpen => "circuit_open",
        HarnessError::ConfigurationChanged | HarnessError::InvalidConfiguration => {
            "configuration"
        }
        HarnessError::Transport(_) => "transport",
        HarnessError::MissingResult(_) | HarnessError::InvalidResponse => "invalid_response",
        HarnessError::Unsupported(_) => "unsupported",
        HarnessError::UnknownAdapter => "unknown_harness",
    }
}

fn valid_window_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && key.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
        })
}

fn bounded_text(value: Option<&str>, maximum: usize) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
        .map(|text| text.chars().take(maximum).collect())
}

fn timestamp(seconds: Option<i64>) -> Option<DateTime<Utc>> {
    seconds.and_then(|seconds| DateTime::from_timestamp(seconds, 0))
}

/// Store a reading. A full read replaces the account's windows (ones it no
/// longer reports are gone, as when a plan changes); a run's sparse update
/// only refreshes the windows it names.
pub(crate) async fn store_report(
    transaction: &mut Transaction<'_, Postgres>,
    account: &UsageAccount,
    report: &UsageReport,
    full: bool,
) -> Result<(), AppError> {
    let mut keys = Vec::new();
    for window in &report.windows {
        if !valid_window_key(&window.key) || keys.contains(&window.key) {
            continue;
        }
        keys.push(window.key.clone());
        sqlx::query(
            r#"
            INSERT INTO usage_windows (
                account_id, organization_id, window_key, limit_name, duration_minutes,
                used_percent, resets_at, limit_reached, read_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
            ON CONFLICT (account_id, window_key) DO UPDATE SET
                limit_name = EXCLUDED.limit_name,
                duration_minutes = COALESCE(EXCLUDED.duration_minutes, usage_windows.duration_minutes),
                used_percent = EXCLUDED.used_percent,
                resets_at = COALESCE(EXCLUDED.resets_at, usage_windows.resets_at),
                limit_reached = EXCLUDED.limit_reached,
                read_at = now()
            "#,
        )
        .bind(account.id)
        .bind(account.organization_id)
        .bind(&window.key)
        .bind(bounded_text(window.limit_name.as_deref(), 80))
        .bind(
            window
                .duration_minutes
                .filter(|minutes| (1..=525_600).contains(minutes))
                .and_then(|minutes| i32::try_from(minutes).ok()),
        )
        .bind(i16::try_from(window.used_percent.min(100)).unwrap_or(100))
        .bind(timestamp(window.resets_at))
        .bind(window.limit_reached || window.used_percent >= 100)
        .execute(&mut **transaction)
        .await?;
    }
    if !full {
        return Ok(());
    }
    sqlx::query("DELETE FROM usage_windows WHERE account_id = $1 AND NOT (window_key = ANY($2))")
        .bind(account.id)
        .bind(&keys)
        .execute(&mut **transaction)
        .await?;
    let credits = report.reset_credits.as_ref().map(|credits| {
        credits
            .credits
            .iter()
            .take(50)
            .map(|credit| {
                json!({
                    "id": credit.id.chars().take(200).collect::<String>(),
                    "title": bounded_text(credit.title.as_deref(), 120),
                    "expiresAt": timestamp(credit.expires_at),
                    "grantedAt": timestamp(credit.granted_at),
                })
            })
            .collect::<Vec<_>>()
    });
    sqlx::query(
        r#"
        INSERT INTO usage_accounts (
            account_id, organization_id, plan, polled_at, reset_credits_available, reset_credits
        ) VALUES ($1, $2, $3, now(), $4, $5)
        ON CONFLICT (account_id) DO UPDATE SET
            plan = EXCLUDED.plan,
            polled_at = now(),
            poll_failed_at = NULL,
            poll_failure = NULL,
            reset_credits_available = EXCLUDED.reset_credits_available,
            reset_credits = EXCLUDED.reset_credits,
            updated_at = now()
        "#,
    )
    .bind(account.id)
    .bind(account.organization_id)
    .bind(bounded_text(report.plan.as_deref(), 40))
    .bind(
        report
            .reset_credits
            .as_ref()
            .map(|credits| i32::try_from(credits.available.min(10_000)).unwrap_or(0)),
    )
    .bind(serde_json::Value::Array(credits.unwrap_or_default()))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// A run reported usage while it ran. Stored at once, then the alert rules
/// run shortly after (one evaluation per account at a time, however many
/// updates arrive). Failures are logged, never the run's.
pub async fn record_run_report(state: &AppState, account_id: Uuid, report: UsageReport) {
    let result = async {
        let Some(account) = load_account(&state.pool, account_id).await? else {
            return Ok(());
        };
        let mut transaction = state.pool.begin().await?;
        store_report(&mut transaction, &account, &report, false).await?;
        jobs::enqueue(
            &mut *transaction,
            NewJob::new(EVALUATE_KIND)
                .workspace(account.organization_id)
                .payload(AccountJob { account_id })
                .unique(format!("{EVALUATE_KIND}:{account_id}"))
                .delay(StdDuration::from_secs(30)),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%account_id, category = error.log_category(), "run usage not recorded");
    }
}

/// One account's background read, then its alert rules.
pub async fn poll_account(state: &AppState, account_id: Uuid) -> Result<(), JobError> {
    let Some(account) = load_account(&state.pool, account_id).await? else {
        return Ok(());
    };
    if account.readable() {
        read_and_store(state, &account).await?;
    }
    alerts::evaluate(state, account_id, Utc::now()).await?;
    Ok(())
}

/// Queue reads for accounts that are due, and alert checks for accounts
/// whose stored or scheduled windows could fire one. Returns how many jobs
/// were queued.
pub async fn enqueue_due(pool: &PgPool) -> Result<usize, AppError> {
    let due = sqlx::query_as::<_, (Uuid, Uuid, bool)>(&format!(
        r#"
        SELECT account.id, account.organization_id,
               account.account_state = 'authenticated' AND account.status <> 'disabled'
                   AS readable
        FROM provider_accounts account
        LEFT JOIN usage_accounts usage ON usage.account_id = account.id
        WHERE (
            account.account_state = 'authenticated' AND account.status <> 'disabled'
            AND (usage.polled_at IS NULL OR usage.polled_at < now() - CASE
                WHEN EXISTS (
                    SELECT 1 FROM usage_windows window_row
                    WHERE window_row.account_id = account.id
                      AND (window_row.used_percent >= 80
                           OR window_row.resets_at < now() + interval '6 hours')
                ) THEN interval '{CLOSE_POLL_MINUTES} minutes'
                ELSE interval '{POLL_MINUTES} minutes' END)
            AND (usage.poll_failed_at IS NULL
                 OR usage.poll_failed_at < now() - interval '{POLL_MINUTES} minutes')
        )
        OR EXISTS (SELECT 1 FROM usage_schedules schedule WHERE schedule.account_id = account.id)
        OR EXISTS (
            SELECT 1 FROM usage_windows window_row
            WHERE window_row.account_id = account.id
              AND (window_row.limit_reached OR window_row.used_percent >= 50
                   OR window_row.resets_at < now() + interval '7 days')
        )
        OR COALESCE(usage.reset_credits_available, 0) > 0
        ORDER BY account.id
        LIMIT 2000
        "#
    ))
    .fetch_all(pool)
    .await?;
    let mut queued = 0;
    for (account_id, organization_id, readable) in due {
        let kind = if readable {
            POLL_ACCOUNT_KIND
        } else {
            EVALUATE_KIND
        };
        let result = jobs::enqueue(
            pool,
            NewJob::new(kind)
                .workspace(organization_id)
                .payload(AccountJob { account_id })
                .unique(format!("{kind}:{account_id}"))
                .max_attempts(3),
        )
        .await?;
        if matches!(result, jobs::Enqueued::Created(_)) {
            queued += 1;
        }
    }
    Ok(queued)
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowSource {
    /// The harness reported it.
    Reported,
    /// The owner's reset schedule.
    Schedule,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowView {
    pub key: String,
    /// Such as `5-hour` or `Weekly`.
    pub label: String,
    pub source: WindowSource,
    pub schedule_id: Option<Uuid>,
    pub duration_minutes: Option<i64>,
    /// `None` when nobody knows (a schedule's window).
    pub used_percent: Option<u16>,
    pub unused_percent: Option<u16>,
    pub resets_at: Option<DateTime<Utc>>,
    pub limit_reached: bool,
    /// When the harness last reported it.
    pub read_at: Option<DateTime<Utc>>,
    /// The window reset after the last reading, so it shows as empty with
    /// its next reset worked out from its length.
    pub rolled_over: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditView {
    pub id: String,
    pub title: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub granted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditsView {
    pub available: i32,
    pub credits: Vec<ResetCreditView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleView {
    pub id: Uuid,
    pub label: String,
    pub cadence: String,
    pub weekday: Option<i16>,
    pub month_day: Option<i16>,
    pub minute_of_day: i16,
    pub time_zone: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageView {
    pub account_id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub harness: String,
    pub harness_name: String,
    /// Signed in, so its harness is read in the background.
    pub connected: bool,
    pub plan: Option<String>,
    pub polled_at: Option<DateTime<Utc>>,
    pub poll_failed_at: Option<DateTime<Utc>>,
    pub poll_failure: Option<String>,
    pub windows: Vec<WindowView>,
    /// `None` when the harness has no banked resets.
    pub reset_credits: Option<ResetCreditsView>,
    pub schedules: Vec<ScheduleView>,
    pub rules: Vec<alerts::RuleView>,
}

#[derive(FromRow)]
struct WindowRow {
    window_key: String,
    limit_name: Option<String>,
    duration_minutes: Option<i32>,
    used_percent: i16,
    resets_at: Option<DateTime<Utc>>,
    limit_reached: bool,
    read_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct UsageRow {
    plan: Option<String>,
    polled_at: Option<DateTime<Utc>>,
    poll_failed_at: Option<DateTime<Utc>>,
    poll_failure: Option<String>,
    reset_credits_available: Option<i32>,
    reset_credits: serde_json::Value,
}

#[derive(FromRow)]
struct ScheduleRow {
    id: Uuid,
    label: String,
    cadence: String,
    weekday: Option<i16>,
    month_day: Option<i16>,
    minute_of_day: i16,
    time_zone: String,
}

/// A window's name from its length: `5-hour`, `Weekly`, `Monthly`.
pub fn window_label(duration_minutes: Option<i64>) -> String {
    match duration_minutes {
        Some(1440) => "Daily".to_owned(),
        Some(10080) => "Weekly".to_owned(),
        Some(minutes) if (40_320..=44_640).contains(&minutes) => "Monthly".to_owned(),
        Some(minutes) if minutes > 0 && minutes < 1440 && minutes % 60 == 0 => {
            format!("{}-hour", minutes / 60)
        }
        Some(minutes) if minutes > 0 && minutes < 1440 => format!("{minutes}-minute"),
        Some(minutes) if minutes % 1440 == 0 => format!("{}-day", minutes / 1440),
        _ => "Usage".to_owned(),
    }
}

/// The account's windows as they stand at `now`: reported ones (rolled
/// over when they reset since the reading) and scheduled ones.
pub(crate) async fn windows_at<'e, E: PgExecutor<'e> + Copy>(
    executor: E,
    account_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<WindowView>, AppError> {
    let rows = sqlx::query_as::<_, WindowRow>(
        r#"
        SELECT window_key, limit_name, duration_minutes, used_percent, resets_at,
               limit_reached, read_at
        FROM usage_windows WHERE account_id = $1
        ORDER BY duration_minutes NULLS LAST, window_key
        "#,
    )
    .bind(account_id)
    .fetch_all(executor)
    .await?;
    // Name the limit only when the account reports more than one.
    let buckets = rows
        .iter()
        .map(|row| row.window_key.split('.').next().unwrap_or_default())
        .collect::<BTreeSet<_>>();
    let mut windows = rows
        .iter()
        .map(|row| {
            let duration = row.duration_minutes.map(i64::from);
            let mut label = window_label(duration);
            if buckets.len() > 1 {
                let bucket = row
                    .limit_name
                    .clone()
                    .unwrap_or_else(|| row.window_key.split('.').next().unwrap_or("").to_owned());
                label = format!("{bucket} {}", label.to_lowercase());
            }
            let (mut used, mut resets_at, mut reached, mut rolled) = (
                Some(u16::try_from(row.used_percent).unwrap_or(100)),
                row.resets_at,
                row.limit_reached,
                false,
            );
            if let Some(at) = row.resets_at
                && at <= now
                && row.read_at < at
            {
                rolled = true;
                used = Some(0);
                reached = false;
                resets_at = duration.filter(|minutes| *minutes > 0).map(|minutes| {
                    let length = Duration::minutes(minutes);
                    let missed = (now - at).num_minutes() / minutes + 1;
                    at + length * i32::try_from(missed).unwrap_or(1)
                });
            }
            WindowView {
                key: row.window_key.clone(),
                label,
                source: WindowSource::Reported,
                schedule_id: None,
                duration_minutes: duration,
                used_percent: used,
                unused_percent: used.map(|used| 100 - used.min(100)),
                resets_at,
                limit_reached: reached,
                read_at: Some(row.read_at),
                rolled_over: rolled,
            }
        })
        .collect::<Vec<_>>();
    let schedules = sqlx::query_as::<_, ScheduleRow>(
        r#"
        SELECT id, label, cadence, weekday, month_day, minute_of_day, time_zone
        FROM usage_schedules WHERE account_id = $1 ORDER BY created_at, id
        "#,
    )
    .bind(account_id)
    .fetch_all(executor)
    .await?;
    for schedule in schedules {
        let Some(cadence) =
            schedule::Cadence::from_parts(&schedule.cadence, schedule.weekday, schedule.month_day)
        else {
            continue;
        };
        let Some(zone) = schedule::parse_time_zone(&schedule.time_zone) else {
            continue;
        };
        let minute = u16::try_from(schedule.minute_of_day).unwrap_or(0);
        let Some((next, previous)) = schedule::resets_around(cadence, minute, zone, now) else {
            continue;
        };
        windows.push(WindowView {
            key: format!("schedule.{}", schedule.id.simple()),
            label: schedule.label,
            source: WindowSource::Schedule,
            schedule_id: Some(schedule.id),
            duration_minutes: Some((next - previous).num_minutes()),
            used_percent: None,
            unused_percent: None,
            resets_at: Some(next),
            limit_reached: false,
            read_at: None,
            rolled_over: false,
        });
    }
    Ok(windows)
}

async fn account_view(
    state: &AppState,
    account: &UsageAccount,
    now: DateTime<Utc>,
) -> Result<AccountUsageView, AppError> {
    let usage = sqlx::query_as::<_, UsageRow>(
        r#"
        SELECT plan, polled_at, poll_failed_at, poll_failure, reset_credits_available,
               reset_credits
        FROM usage_accounts WHERE account_id = $1
        "#,
    )
    .bind(account.id)
    .fetch_optional(&state.pool)
    .await?;
    let windows = windows_at(&state.pool, account.id, now).await?;
    let schedules = sqlx::query_as::<_, ScheduleRow>(
        r#"
        SELECT id, label, cadence, weekday, month_day, minute_of_day, time_zone
        FROM usage_schedules WHERE account_id = $1 ORDER BY created_at, id
        "#,
    )
    .bind(account.id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|row| ScheduleView {
        id: row.id,
        label: row.label,
        cadence: row.cadence,
        weekday: row.weekday,
        month_day: row.month_day,
        minute_of_day: row.minute_of_day,
        time_zone: row.time_zone,
    })
    .collect();
    let rules = alerts::rules(&state.pool, account.id).await?;
    let harness_name = state
        .supervisors
        .manifest(&account.adapter_kind)
        .map_or_else(|| account.adapter_kind.clone(), |manifest| manifest.display_name.clone());
    let reset_credits = usage.as_ref().and_then(|usage| {
        usage.reset_credits_available.map(|available| ResetCreditsView {
            available,
            credits: stored_credits(&usage.reset_credits),
        })
    });
    Ok(AccountUsageView {
        account_id: account.id,
        organization_id: account.organization_id,
        name: account.name.clone(),
        harness: account.adapter_kind.clone(),
        harness_name,
        connected: account.readable(),
        plan: usage.as_ref().and_then(|usage| usage.plan.clone()),
        polled_at: usage.as_ref().and_then(|usage| usage.polled_at),
        poll_failed_at: usage.as_ref().and_then(|usage| usage.poll_failed_at),
        poll_failure: usage.as_ref().and_then(|usage| usage.poll_failure.clone()),
        windows,
        reset_credits,
        schedules,
        rules,
    })
}

pub(crate) fn stored_credits(value: &serde_json::Value) -> Vec<ResetCreditView> {
    let time = |credit: &serde_json::Value, name: &str| {
        credit
            .get(name)
            .and_then(serde_json::Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|at| at.with_timezone(&Utc))
    };
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|credit| {
            Some(ResetCreditView {
                id: credit.get("id")?.as_str()?.to_owned(),
                title: credit
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                expires_at: time(credit, "expiresAt"),
                granted_at: time(credit, "grantedAt"),
            })
        })
        .collect()
}

/// Every account `viewer` owns, in workspaces they're still an enabled
/// member of (or only `organization_id`), with its windows at `now`.
pub async fn list_for(
    state: &AppState,
    viewer: Uuid,
    organization_id: Option<Uuid>,
) -> Result<Vec<AccountUsageView>, AppError> {
    let accounts = sqlx::query_as::<_, UsageAccount>(&format!(
        r#"
        SELECT {ACCOUNT_COLUMNS}
        FROM provider_accounts account
        JOIN organization_memberships membership
          ON membership.organization_id = account.organization_id
         AND membership.actor_id = account.owner_actor_id AND membership.enabled
        WHERE account.owner_actor_id = $1
          AND ($2::uuid IS NULL OR account.organization_id = $2)
        ORDER BY account.created_at, account.id
        LIMIT 100
        "#
    ))
    .bind(viewer)
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?;
    let now = Utc::now();
    let mut views = Vec::with_capacity(accounts.len());
    for account in &accounts {
        views.push(account_view(state, account, now).await?);
    }
    Ok(views)
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/usage", get(list_http))
        .route("/api/v1/usage/accounts/{account_id}", get(read_http))
        .route(
            "/api/v1/usage/accounts/{account_id}/refresh",
            post(refresh_http),
        )
        .route(
            "/api/v1/usage/accounts/{account_id}/ready-tasks",
            get(ready_tasks_http),
        )
        .route(
            "/api/v1/usage/accounts/{account_id}/schedules",
            post(create_schedule_http),
        )
        .route(
            "/api/v1/usage/accounts/{account_id}/schedules/{schedule_id}",
            delete(delete_schedule_http),
        )
        .route(
            "/api/v1/usage/accounts/{account_id}/rules/{rule}",
            put(set_rule_http),
        )
        .route(
            "/api/v1/usage/accounts/{account_id}/reset-credits/spend",
            post(spend_credit_http),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    organization_id: Option<Uuid>,
}

#[derive(Serialize)]
struct ListResponse {
    accounts: Vec<AccountUsageView>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<ListResponse>, AppError> {
    Ok(Json(ListResponse {
        accounts: list_for(&state, principal.actor_id, query.organization_id).await?,
    }))
}

async fn read_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(account_id): ApiPath<Uuid>,
) -> Result<Json<AccountUsageView>, AppError> {
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    Ok(Json(account_view(&state, &account, Utc::now()).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

/// Read the account's usage now, as the account page opens or on Refresh.
async fn refresh_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<AccountUsageView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    if account.readable() {
        match read_and_store(&state, &account).await {
            Ok(_) => {}
            Err(_) => return Err(AppError::rate_limited(5)),
        }
        alerts::evaluate(&state, account_id, Utc::now()).await?;
    }
    Ok(Json(account_view(&state, &account, Utc::now()).await?))
}

async fn ready_tasks_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(account_id): ApiPath<Uuid>,
) -> Result<Json<ready::ReadyTasks>, AppError> {
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    let mut connection = state.pool.acquire().await?;
    Ok(Json(
        ready::ready_tasks(&mut connection, principal.actor_id, account.id, None).await?,
    ))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewSchedule {
    pub label: String,
    pub cadence: String,
    pub weekday: Option<i16>,
    pub month_day: Option<i16>,
    pub minute_of_day: i16,
    pub time_zone: String,
}

pub async fn create_schedule(
    state: &AppState,
    viewer: Uuid,
    account_id: Uuid,
    request: &NewSchedule,
) -> Result<AccountUsageView, AppError> {
    let mut transaction = state.pool.begin().await?;
    let account = owned_account(&mut *transaction, viewer, account_id).await?;
    let label = bounded_text(Some(&request.label), 60).ok_or(AppError::Validation("label"))?;
    if request.label.chars().count() > 60 {
        return Err(AppError::Validation("label"));
    }
    schedule::Cadence::from_parts(&request.cadence, request.weekday, request.month_day)
        .ok_or(AppError::Validation("cadence"))?;
    if !(0..=1439).contains(&request.minute_of_day) {
        return Err(AppError::Validation("minuteOfDay"));
    }
    if schedule::parse_time_zone(&request.time_zone).is_none() || request.time_zone.len() > 64 {
        return Err(AppError::Validation("timeZone"));
    }
    // Serialize schedule writes per account so the cap holds.
    sqlx::query("SELECT 1 FROM provider_accounts WHERE id = $1 FOR UPDATE")
        .bind(account.id)
        .execute(&mut *transaction)
        .await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM usage_schedules WHERE account_id = $1",
    )
    .bind(account.id)
    .fetch_one(&mut *transaction)
    .await?;
    if count >= MAX_SCHEDULES_PER_ACCOUNT {
        return Err(AppError::Conflict("usage_schedule", "too many schedules"));
    }
    sqlx::query(
        r#"
        INSERT INTO usage_schedules (
            id, account_id, organization_id, label, cadence, weekday, month_day,
            minute_of_day, time_zone, created_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(account.id)
    .bind(account.organization_id)
    .bind(label)
    .bind(&request.cadence)
    .bind(request.weekday)
    .bind(request.month_day)
    .bind(request.minute_of_day)
    .bind(&request.time_zone)
    .bind(viewer)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    account_view(state, &account, Utc::now()).await
}

async fn create_schedule_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<NewSchedule>,
) -> Result<Json<AccountUsageView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    Ok(Json(
        create_schedule(&state, principal.actor_id, account_id, &request).await?,
    ))
}

async fn delete_schedule_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((account_id, schedule_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<AccountUsageView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    let deleted = sqlx::query("DELETE FROM usage_schedules WHERE id = $1 AND account_id = $2")
        .bind(schedule_id)
        .bind(account.id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound("usage_schedule"));
    }
    Ok(Json(account_view(&state, &account, Utc::now()).await?))
}

async fn set_rule_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((account_id, rule)): ApiPath<(Uuid, String)>,
    ApiJson(request): ApiJson<alerts::RuleChange>,
) -> Result<Json<AccountUsageView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    let rule = alerts::Rule::parse(&rule).ok_or(AppError::NotFound("alert rule"))?;
    alerts::set_rule(&state.pool, &account, rule, &request).await?;
    Ok(Json(account_view(&state, &account, Utc::now()).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpendRequest {
    credit_id: Option<String>,
    /// Spending can't be undone, so the caller says it confirmed.
    confirm: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpendResponse {
    outcome: ResetOutcome,
    account: AccountUsageView,
}

/// U6: spend a banked reset. Takes an `Idempotency-Key`, which goes to the
/// harness, so a retried request never spends a second one.
async fn spend_credit_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SpendRequest>,
) -> Result<Json<SpendResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if !request.confirm {
        return Err(AppError::Validation("confirm"));
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    if request
        .credit_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 200 || id.chars().any(char::is_control))
    {
        return Err(AppError::Validation("creditId"));
    }
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    if !account.readable() {
        return Err(AppError::Conflict(
            "provider_account",
            "the account is signed out",
        ));
    }
    let outcome = async {
        let mut session = state
            .supervisors
            .acquire_control(&account.adapter_kind, account.handle())
            .await?;
        session
            .spend_reset_credit(
                request.credit_id.as_deref(),
                &format!("aw-{}-{}", account.id.simple(), idempotency_key.as_str()),
            )
            .await
    }
    .await
    .map_err(|error| match error {
        HarnessError::Unsupported(_) => {
            AppError::Conflict("provider_account", "this harness has no banked resets")
        }
        HarnessError::ControlCapacity | HarnessError::HostCapacity => AppError::rate_limited(5),
        other => {
            tracing::warn!(%account_id, category = failure_category(&other), "banked reset failed");
            AppError::Conflict("provider", "the harness could not spend the reset")
        }
    })?;
    tracing::info!(%account_id, ?outcome, "banked reset spent");
    // Read again so the refilled windows show at once.
    let _ = read_and_store(&state, &account).await;
    Ok(Json(SpendResponse {
        outcome,
        account: account_view(&state, &account, Utc::now()).await?,
    }))
}

#[cfg(test)]
mod tests {
    use super::{valid_window_key, window_label};

    #[test]
    fn windows_are_named_by_their_length() {
        assert_eq!(window_label(Some(300)), "5-hour");
        assert_eq!(window_label(Some(10080)), "Weekly");
        assert_eq!(window_label(Some(43_200)), "Monthly");
        assert_eq!(window_label(Some(1440)), "Daily");
        assert_eq!(window_label(Some(2880)), "2-day");
        assert_eq!(window_label(None), "Usage");
    }

    #[test]
    fn window_keys_are_checked() {
        assert!(valid_window_key("codex.primary"));
        assert!(!valid_window_key("Codex.primary"));
        assert!(!valid_window_key(".x"));
        assert!(!valid_window_key(&"a".repeat(65)));
    }
}

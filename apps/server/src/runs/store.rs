//! A1: the rows of the run tables as the run service reads them.

use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

use super::state::{Liveness, RunState};
use crate::error::AppError;

/// Every column of `runs` the service reads, in one place.
pub const RUN_COLUMNS: &str = r#"
    run.id, run.workspace_id, run.space_id, run.account_id, run.host_id,
    run.host_run_ref, run.backend, run.title, run.cause_kind, run.cause_ref,
    run.conversation_id, run.work_item_id, run.cause_message_id,
    run.requested_by_account_id, run.sponsored_by_account_id, run.via_app_id,
    run.parent_run_id, run.predecessor_run_id, run.launch_id, run.state, run.liveness,
    run.waiting_on_request_id, run.status_line, run.outcome_summary, run.failure_reason,
    run.external_url, run.client_run_id, run.report_interval_seconds, run.last_report_at,
    run.activity_seq, run.gap_count, run.settings, run.version, run.created_at,
    run.accepted_at, run.started_at, run.cancel_requested_at,
    run.cancel_requested_by_account_id, run.finished_at, run.updated_at
"#;

/// One `runs` row.
#[derive(Clone, Debug, FromRow)]
pub struct RunRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
    pub account_id: Uuid,
    pub host_id: Option<Uuid>,
    pub host_run_ref: Option<String>,
    pub backend: Option<String>,
    pub title: String,
    pub cause_kind: String,
    pub cause_ref: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
    pub cause_message_id: Option<Uuid>,
    pub requested_by_account_id: Option<Uuid>,
    pub sponsored_by_account_id: Option<Uuid>,
    pub via_app_id: Option<Uuid>,
    pub parent_run_id: Option<Uuid>,
    pub predecessor_run_id: Option<Uuid>,
    pub launch_id: Option<Uuid>,
    pub state: String,
    pub liveness: String,
    pub waiting_on_request_id: Option<Uuid>,
    pub status_line: Option<String>,
    pub outcome_summary: Option<String>,
    pub failure_reason: Option<String>,
    pub external_url: Option<String>,
    pub client_run_id: Option<String>,
    pub report_interval_seconds: i32,
    pub last_report_at: Option<DateTime<Utc>>,
    pub activity_seq: i64,
    pub gap_count: i32,
    pub settings: serde_json::Value,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub cancel_requested_by_account_id: Option<Uuid>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl RunRow {
    pub fn run_state(&self) -> Result<RunState, AppError> {
        self.state
            .parse()
            .map_err(|_| AppError::Internal("unknown run state"))
    }

    pub fn liveness(&self) -> Liveness {
        Liveness::parse(&self.liveness).unwrap_or(Liveness::Ok)
    }
}

/// Every column of `run_activities` the service reads.
pub const ACTIVITY_COLUMNS: &str = r#"
    activity.id, activity.run_id, activity.seq, activity.client_id, activity.kind,
    activity.step_ref, activity.parent_ref, activity.summary, activity.payload,
    activity.occurred_at, activity.received_at, activity.reported_by_account_id,
    activity.in_thread
"#;

/// One `run_activities` row.
#[derive(Clone, Debug, FromRow)]
pub struct ActivityRow {
    pub id: Uuid,
    pub run_id: Uuid,
    pub seq: i64,
    pub client_id: Option<String>,
    pub kind: String,
    pub step_ref: Option<String>,
    pub parent_ref: Option<String>,
    pub summary: String,
    pub payload: serde_json::Value,
    pub occurred_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub reported_by_account_id: Uuid,
    pub in_thread: bool,
}

/// A1.2: who can see a run, as one SQL condition on `run` for the viewer
/// bound at `$n`: who can see its conversation (`conversation_visible_to`);
/// else, for a run in a project, its members; else the account that does it
/// and who asked for it. Every read of runs for a viewer uses it (through
/// the `run_visible_to` function, migration 20261114073500).
pub fn visible(viewer_parameter: usize) -> String {
    format!("run_visible_to(run.id, ${viewer_parameter})")
}

/// Whether a conversation, or (for a channel) any of its threads, has a run
/// that hasn't finished.
pub async fn active_in_conversation_tree(
    executor: impl sqlx::PgExecutor<'_>,
    conversation_id: Uuid,
    include_threads: bool,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM runs run
            JOIN conversations owned ON owned.id = run.conversation_id
            WHERE run.state = ANY($3)
              AND (owned.id = $1 OR ($2 AND owned.parent_channel_id = $1))
        )
        "#,
    )
    .bind(conversation_id)
    .bind(include_threads)
    .bind(RunState::active_keys())
    .fetch_one(executor)
    .await?)
}

/// Whether a project has a run that hasn't finished, whoever can see it
/// (archiving waits for every run).
pub async fn active_under_space(
    executor: impl sqlx::PgExecutor<'_>,
    space_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM runs run WHERE run.space_id = $1 AND run.state = ANY($2))",
    )
    .bind(space_id)
    .bind(RunState::active_keys())
    .fetch_one(executor)
    .await?)
}

/// Whether a project's repository has a run that hasn't finished: a run on
/// the built-in host working in it.
pub async fn active_for_repository(
    executor: impl sqlx::PgExecutor<'_>,
    space_id: Uuid,
    repository_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM runs run
            JOIN agent_runs detail ON detail.id = run.id
            JOIN repositories repository ON repository.id = detail.repository_id
            WHERE repository.id = $1 AND repository.project_id = $2
              AND run.state = ANY($3)
        )
        "#,
    )
    .bind(repository_id)
    .bind(space_id)
    .bind(RunState::active_keys())
    .fetch_one(executor)
    .await?)
}

/// The project of a run the viewer can see.
pub async fn space_of_visible(
    executor: impl sqlx::PgExecutor<'_>,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(&format!(
        "SELECT run.space_id FROM runs run WHERE run.id = $2 AND {}",
        visible(1)
    ))
    .bind(viewer)
    .bind(run_id)
    .fetch_optional(executor)
    .await?
    .flatten())
}

/// The runs in a project the viewer can see, newest first.
pub async fn visible_in_space(
    executor: impl sqlx::PgExecutor<'_>,
    viewer: Uuid,
    space_id: Uuid,
    limit: i64,
) -> Result<Vec<RunRow>, AppError> {
    Ok(sqlx::query_as::<_, RunRow>(&format!(
        r#"
        SELECT {RUN_COLUMNS} FROM runs run
        WHERE run.space_id = $2 AND {}
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT $3
        "#,
        visible(1)
    ))
    .bind(viewer)
    .bind(space_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(executor)
    .await?)
}

/// A run by id, for a caller that has already checked who may see it.
pub async fn get(
    executor: impl sqlx::PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Option<RunRow>, AppError> {
    Ok(sqlx::query_as::<_, RunRow>(&format!(
        "SELECT {RUN_COLUMNS} FROM runs run WHERE run.id = $1"
    ))
    .bind(run_id)
    .fetch_optional(executor)
    .await?)
}

/// The project of a run the account does itself, while it still sees the
/// run's conversation. Every reporting call (and its replay) starts here.
pub async fn space_of_own(
    executor: impl sqlx::PgExecutor<'_>,
    account_id: Uuid,
    run_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        r#"
        SELECT run.space_id FROM runs run
        WHERE run.id = $1 AND run.account_id = $2
          AND (run.conversation_id IS NULL OR conversation_visible_to(run.conversation_id, $2))
        "#,
    )
    .bind(run_id)
    .bind(account_id)
    .fetch_optional(executor)
    .await?
    .flatten())
}

/// Where a built-in run's turn happens, for its session row (A1.4 moved this
/// from `agent_sessions`).
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct BuiltinTurnPlace {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub worker_actor_id: Uuid,
    pub harness: String,
}

pub async fn builtin_turn_place(
    executor: impl sqlx::PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Option<BuiltinTurnPlace>, AppError> {
    Ok(sqlx::query_as::<_, BuiltinTurnPlace>(
        r#"
        SELECT run.project_id, run.conversation_id, worker.actor_id AS worker_actor_id,
               COALESCE(run.config ->> 'harness', account.adapter_kind) AS harness
        FROM agent_runs run
        JOIN execution_workers worker ON worker.id = run.execution_worker_id
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(executor)
    .await?)
}

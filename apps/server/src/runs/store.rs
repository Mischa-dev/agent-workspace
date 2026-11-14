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

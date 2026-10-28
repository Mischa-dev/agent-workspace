//! Settings › Workspace › Server, for the server's operators: which build
//! is running, the checks `preflight` makes (run live), when the nightly
//! backup last ran and whether it worked, the job queue, and whether the
//! workspace can send email. Operators are whoever set the server up from
//! its terminal (`bootstrap-admin`) and anyone named in
//! `AGENT_WORKSPACE_OPERATOR_LOGINS`; a workspace's owner on a shared server
//! isn't one, and sees nothing of the server here.
//!
//! The backup timer runs as root, outside the app, so it leaves a small
//! status file in the data folder (`backup-status.json`, written by
//! `deploy/redhat/agent-workspace-daily-backup`) that the app only reads.
//! It holds times and step results, never where backups go.

use std::path::{Path, PathBuf};

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
    health::BUILD_COMMIT,
    jobs,
    preflight::{self, Level},
    workspace::{WorkspaceRole, caller_role},
};

pub const BACKUP_STATUS_FILE: &str = "backup-status.json";
const MAX_STATUS_BYTES: u64 = 16 * 1024;
/// A nightly backup older than this is reported as overdue.
const BACKUP_OVERDUE_HOURS: i64 = 36;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/organizations/{organization_id}/server",
        get(read_http),
    )
}

/// Whether `principal` runs this server: set it up from its terminal, or is
/// named in `AGENT_WORKSPACE_OPERATOR_LOGINS`.
pub async fn is_operator(
    state: &AppState,
    principal: &AuthenticatedActor,
) -> Result<bool, AppError> {
    if state
        .config
        .operator_logins
        .iter()
        .any(|login| login.eq_ignore_ascii_case(&principal.login_name))
    {
        return Ok(true);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM audit_events
            WHERE actor_id = $1 AND action = 'identity.bootstrap_admin' AND outcome = 'succeeded'
        )
        "#,
    )
    .bind(principal.actor_id)
    .fetch_one(&state.pool)
    .await?)
}

pub fn backup_status_path(data_root: &Path) -> PathBuf {
    data_root.join(BACKUP_STATUS_FILE)
}

/// What the backup script records.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub ok: bool,
    /// Each step's result, such as `{"database": "ok", "key": "failed"}`.
    #[serde(default)]
    pub steps: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupState {
    /// No record: the timer isn't set up or hasn't run yet.
    Never,
    Ok,
    Failed,
    /// The last run worked but was too long ago.
    Overdue,
    /// A record exists but couldn't be read.
    Unreadable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupView {
    pub state: BackupState,
    pub last: Option<BackupRecord>,
}

pub fn read_backup(data_root: &Path, now: DateTime<Utc>) -> BackupView {
    let path = backup_status_path(data_root);
    let record = match std::fs::metadata(&path) {
        Err(_) => {
            return BackupView {
                state: BackupState::Never,
                last: None,
            };
        }
        Ok(metadata) if !metadata.is_file() || metadata.len() > MAX_STATUS_BYTES => None,
        Ok(_) => std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BackupRecord>(&bytes).ok()),
    };
    let Some(record) = record else {
        return BackupView {
            state: BackupState::Unreadable,
            last: None,
        };
    };
    let state = if !record.ok {
        BackupState::Failed
    } else if now - record.finished_at > chrono::Duration::hours(BACKUP_OVERDUE_HOURS) {
        BackupState::Overdue
    } else {
        BackupState::Ok
    };
    BackupView {
        state,
        last: Some(record),
    }
}

/// One line for `preflight` about the nightly backup.
pub fn backup_line(data_root: &Path) -> (Level, String) {
    let backup = read_backup(data_root, Utc::now());
    let when = |record: &Option<BackupRecord>| {
        record
            .as_ref()
            .map(|record| record.finished_at.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default()
    };
    match backup.state {
        BackupState::Never => (
            Level::Warn,
            "no nightly backup has run yet (see docs/operations/backups.md)".to_owned(),
        ),
        BackupState::Ok => (
            Level::Ok,
            format!("last nightly backup worked ({})", when(&backup.last)),
        ),
        BackupState::Overdue => (
            Level::Warn,
            format!(
                "the last nightly backup worked but is overdue ({})",
                when(&backup.last)
            ),
        ),
        BackupState::Failed => (
            Level::Warn,
            format!(
                "the last nightly backup failed ({}); check journalctl -u agent-workspace-daily-backup",
                when(&backup.last)
            ),
        ),
        BackupState::Unreadable => (
            Level::Warn,
            "the nightly backup's status file can't be read".to_owned(),
        ),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckView {
    level: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MailView {
    configured: bool,
    enabled: bool,
    last_test_at: Option<DateTime<Utc>>,
    last_test_ok: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobsView {
    waiting: i64,
    running: i64,
    failed_last_day: i64,
    oldest_waiting_seconds: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerView {
    version: &'static str,
    commit: &'static str,
    environment: String,
    public_origin: String,
    checked_at: DateTime<Utc>,
    checks: Vec<CheckView>,
    backup: BackupView,
    mail: MailView,
    jobs: JobsView,
}

async fn read_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<ServerView>, AppError> {
    {
        let mut connection = state.pool.acquire().await?;
        if caller_role(&mut connection, &principal, organization_id, false).await?
            != WorkspaceRole::Owner
        {
            return Err(AppError::Forbidden);
        }
    }
    if !is_operator(&state, &principal).await? {
        return Err(AppError::Forbidden);
    }
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let report = preflight::run(&state.config, &state.pool)
        .await
        .map_err(|_| AppError::Internal("server checks"))?;
    let checks = report
        .lines
        .into_iter()
        .map(|(level, text)| CheckView {
            level: match level {
                Level::Ok => "ok",
                Level::Warn => "warn",
                Level::Fail => "fail",
            },
            text,
        })
        .collect();
    let mail = match crate::mail::settings_row(&state.pool, organization_id).await? {
        Some(row) => MailView {
            configured: true,
            enabled: row.enabled,
            last_test_at: row.last_test_at,
            last_test_ok: row.last_test_ok,
        },
        None => MailView {
            configured: false,
            enabled: false,
            last_test_at: None,
            last_test_ok: None,
        },
    };
    let queue = jobs::queue_stats(&state.pool).await?;
    Ok(Json(ServerView {
        version: env!("CARGO_PKG_VERSION"),
        commit: BUILD_COMMIT,
        environment: state.config.environment.to_string(),
        public_origin: crate::events::public_origin(&state),
        checked_at: Utc::now(),
        checks,
        backup: read_backup(&state.config.data_root, Utc::now()),
        mail,
        jobs: JobsView {
            waiting: queue.queued_due,
            running: queue.running,
            failed_last_day: queue.dead_last_day,
            oldest_waiting_seconds: queue.oldest_due_seconds,
        },
    }))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn backup_status_reads_ok_failed_overdue_and_missing() {
        let folder = tempfile::tempdir().expect("folder");
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        assert_eq!(read_backup(folder.path(), now).state, BackupState::Never);
        let write = |finished: &str, ok: bool| {
            std::fs::write(
                backup_status_path(folder.path()),
                format!(
                    r#"{{"startedAt":"2026-10-17T03:30:00Z","finishedAt":"{finished}","ok":{ok},"steps":{{"database":"ok"}}}}"#
                ),
            )
            .expect("write status");
        };
        write("2026-10-17T03:40:00Z", true);
        assert_eq!(read_backup(folder.path(), now).state, BackupState::Ok);
        write("2026-10-17T03:40:00Z", false);
        assert_eq!(read_backup(folder.path(), now).state, BackupState::Failed);
        write("2026-10-14T03:40:00Z", true);
        assert_eq!(read_backup(folder.path(), now).state, BackupState::Overdue);
        std::fs::write(backup_status_path(folder.path()), "not json").expect("write");
        assert_eq!(
            read_backup(folder.path(), now).state,
            BackupState::Unreadable
        );
    }
}

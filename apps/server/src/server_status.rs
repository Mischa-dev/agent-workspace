//! Settings › Workspace › Server, for the server's operators: which build
//! is running, the checks `preflight` makes (run live), when the nightly
//! backup last ran and whether it worked, the job queue, and whether the
//! workspace can send email. Operators are whoever set the server up from
//! its terminal (`bootstrap-admin`, which grants the operator server role)
//! and anyone named in
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
    workspace::standing_of,
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

/// The server role of whoever runs the server (K1.30).
pub const OPERATOR: &str = "operator";

/// Whether `principal` runs this server: holds the operator server role
/// (bootstrap-admin grants it), or is named in
/// `AGENT_WORKSPACE_OPERATOR_LOGINS`.
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
        "SELECT EXISTS (SELECT 1 FROM server_roles WHERE account_id = $1 AND role = $2)",
    )
    .bind(principal.actor_id)
    .bind(OPERATOR)
    .fetch_one(crate::db::scoped(&state.pool))
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
    event_log: EventLogView,
    instances: Vec<InstanceView>,
}

/// A server process sharing this database (K5.7): what it does, which
/// release, how long since its heartbeat, and the leases it holds.
#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct InstanceView {
    id: Uuid,
    label: Option<String>,
    roles: Vec<String>,
    release: String,
    started_at: DateTime<Utc>,
    heartbeat_seconds: f64,
    draining: bool,
    leases: Vec<String>,
}

/// The event log (K3.3): events waiting for a position, and parked events
/// a consumer moved past.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventLogView {
    waiting: i64,
    oldest_waiting_seconds: f64,
    parked: i64,
}

async fn read_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<ServerView>, AppError> {
    {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        // An owner of the workspace who also runs the server (K1.30 makes
        // this the server role).
        let standing =
            standing_of(&mut connection, principal.actor_id, organization_id, false).await?;
        if !crate::access::tier::owner(standing.role) {
            return Err(AppError::Forbidden);
        }
    }
    if !is_operator(&state, &principal).await? {
        return Err(AppError::Forbidden);
    }
    state
        .rate_limits
        .check_expensive_read(principal.session_id)
        .await?;
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
    let (waiting, oldest_waiting_seconds, parked) = crate::db::scoped_one(&state.pool, async |connection| sqlx::query_as::<_, (i64, f64, i64)>(
        "SELECT (SELECT count(*) FROM event_staging), \
                COALESCE((SELECT EXTRACT(EPOCH FROM now() - min(staged_at)) FROM event_staging), 0)::float8, \
                (SELECT count(*) FROM event_consumer_failures WHERE resolved_at IS NULL)",
    )
    .fetch_one(&mut *connection).await).await?;
    // Instances heard from in the last minute that haven't stopped.
    let instances = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, InstanceView>(
            "SELECT instance.id, instance.label, instance.roles, instance.release, \
                    instance.started_at, \
                    EXTRACT(EPOCH FROM now() - instance.heartbeat_at)::float8 AS heartbeat_seconds, \
                    instance.draining_at IS NOT NULL AS draining, \
                    COALESCE(ARRAY(SELECT lease.name FROM leases lease \
                                   WHERE lease.holder_instance_id = instance.id \
                                     AND lease.expires_at > now() \
                                   ORDER BY lease.name), '{}') AS leases \
             FROM server_instances instance \
             WHERE instance.stopped_at IS NULL \
               AND instance.heartbeat_at > now() - interval '1 minute' \
             ORDER BY instance.started_at",
        )
        .fetch_all(&mut *connection)
        .await
    })
    .await?;
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
        event_log: EventLogView {
            waiting,
            oldest_waiting_seconds,
            parked,
        },
        instances,
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

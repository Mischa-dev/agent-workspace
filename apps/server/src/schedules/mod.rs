//! A2.11: follow-ups an account sets for itself.
//!
//! An account (an agent, usually) asks to be told again later: once, or
//! every so many minutes, optionally about one thing it can see. When one is
//! due the account hears `schedule.due`, addressed to it, carrying the
//! schedule's and the thing's ids; the note stays in its row. Each due time
//! is one job on the queue, keyed by the schedule and the time, so however
//! many workers run it fires once, and nothing runs per workspace (R24). The
//! event's thing and space are checked again when it is read, so a thing the
//! account can no longer see is never named to it.

pub mod ops;

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    events::{Audience, NewEvent, catalog},
    jobs::{self, JobError, JobRegistry, NewJob},
};

pub const DUE_KIND: &str = "schedule.due";

#[derive(Debug, Deserialize, Serialize)]
struct DueJob {
    schedule_id: Uuid,
    at_micros: i64,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(DUE_KIND, Duration::from_secs(60), |context| async move {
        let job: DueJob = context.job.payload_as()?;
        due(&context.state, &job).await
    });
}

/// Queue the job that fires a schedule at `at`.
pub(crate) async fn queue(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    schedule_id: Uuid,
    at: DateTime<Utc>,
) -> Result<(), AppError> {
    jobs::enqueue(
        &mut *connection,
        NewJob::new(DUE_KIND)
            .workspace(workspace_id)
            .payload(DueJob {
                schedule_id,
                at_micros: at.timestamp_micros(),
            })
            .unique(format!(
                "{DUE_KIND}:{schedule_id}:{}",
                at.timestamp_micros()
            ))
            .run_at(at)
            .max_attempts(5),
    )
    .await?;
    Ok(())
}

/// Tells the account's other tabs and sessions its schedules changed.
pub(crate) async fn changed(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    account_id: Uuid,
    schedule_id: Uuid,
) -> Result<(), AppError> {
    crate::events::emit(
        connection,
        NewEvent {
            id: Uuid::now_v7(),
            ty: &catalog::SCHEDULE_CHANGED,
            workspace_id: Some(workspace_id),
            space_id: None,
            thing: Some((schedule_id, "schedule")),
            access_thing: None,
            version: None,
            changes: &[],
            audience: Audience::Account(account_id),
            payload: serde_json::json!({ "scheduleId": schedule_id }),
            actor: Some(account_id),
            legacy_type: None,
        },
    )
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct DueRow {
    id: Uuid,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    account_id: Uuid,
    thing_id: Option<Uuid>,
    thing_kind: Option<String>,
    every_minutes: Option<i32>,
    next_due_at: Option<DateTime<Utc>>,
}

/// The next time an interval schedule is due after `at`, skipping times
/// already past (a server that was down fires once, not once per miss).
pub(crate) fn next_after(
    at: DateTime<Utc>,
    every_minutes: i32,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let step = TimeDelta::minutes(i64::from(every_minutes.max(1)));
    let mut next = at + step;
    if next <= now {
        let behind = (now - next).num_minutes() / step.num_minutes() + 1;
        next += step * i32::try_from(behind).unwrap_or(i32::MAX);
    }
    next
}

async fn due(state: &AppState, job: &DueJob) -> Result<(), JobError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(row) = sqlx::query_as::<_, DueRow>(
        r#"
        SELECT id, workspace_id, space_id, account_id, thing_id, thing_kind, every_minutes,
               next_due_at
        FROM agent_schedules
        WHERE id = $1 AND state = 'active'
        FOR UPDATE
        "#,
    )
    .bind(job.schedule_id)
    .fetch_optional(&mut *transaction)
    .await?
    else {
        return Ok(());
    };
    let Some(at) = row.next_due_at else {
        return Ok(());
    };
    if at.timestamp_micros() != job.at_micros {
        // Moved since this job was queued; its own job fires it.
        return Ok(());
    }
    // The note stays in its row; the event carries ids.
    crate::events::emit(
        &mut transaction,
        NewEvent {
            id: Uuid::now_v7(),
            ty: &catalog::SCHEDULE_DUE,
            workspace_id: Some(row.workspace_id),
            space_id: row.space_id,
            thing: Some((row.id, "schedule")),
            access_thing: row.thing_id,
            version: None,
            changes: &[],
            audience: Audience::Account(row.account_id),
            payload: serde_json::json!({
                "source": "schedule",
                "scheduleId": row.id,
                "thingKind": row.thing_kind,
                "thingId": row.thing_id,
            }),
            actor: Some(row.account_id),
            legacy_type: None,
        },
    )
    .await?;
    match row.every_minutes {
        Some(every) => {
            let next = next_after(at, every, Utc::now());
            sqlx::query(
                "UPDATE agent_schedules SET next_due_at = $2, last_due_at = $3, \
                 times_due = times_due + 1, updated_at = now() WHERE id = $1",
            )
            .bind(row.id)
            .bind(next)
            .bind(at)
            .execute(&mut *transaction)
            .await?;
            queue(&mut transaction, row.workspace_id, row.id, next).await?;
        }
        None => {
            sqlx::query(
                "UPDATE agent_schedules SET state = 'done', next_due_at = NULL, last_due_at = $2, \
                 times_due = times_due + 1, version = version + 1, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(row.id)
            .bind(at)
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missed_interval_fires_once_at_the_next_time_ahead() {
        let at = DateTime::parse_from_rfc3339("2026-11-17T10:00:00Z")
            .expect("time")
            .with_timezone(&Utc);
        let soon = at + TimeDelta::minutes(5);
        assert_eq!(next_after(at, 60, soon), at + TimeDelta::minutes(60));
        let late = at + TimeDelta::minutes(185);
        assert_eq!(next_after(at, 60, late), at + TimeDelta::minutes(240));
    }
}

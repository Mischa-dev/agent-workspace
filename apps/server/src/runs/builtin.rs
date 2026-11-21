//! A1.14: the built-in host keeps its runs' records itself, as any host
//! reports on its runs. A built-in run's `runs` row is opened with its
//! detail row (`agent_runs`, the host's own) and moved with every change of
//! the host's state machine (`hosts::builtin::transition`); no trigger
//! copies one into the other.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::state::RunState;
use crate::{domain::RunState as BuiltinState, error::AppError};

/// What the record of a new built-in run says.
pub struct NewBuiltinRun<'a> {
    pub id: Uuid,
    pub project_id: Uuid,
    pub execution_worker_id: Uuid,
    pub prompt: &'a str,
    /// Asked for through a run request (an agent asking in a person's name).
    pub from_request: bool,
    pub conversation_id: Uuid,
    pub requested_by: Uuid,
    pub sponsored_by: Option<Uuid>,
    pub predecessor_run_id: Option<Uuid>,
    pub settings: &'a Value,
}

/// A run's title from its prompt: the first line, on one line, at most 200
/// characters; "Run" when that's empty.
pub fn title_from_prompt(prompt: &str) -> String {
    let first = prompt.trim().split('\n').next().unwrap_or_default();
    let mut title = String::new();
    let mut in_control = false;
    for character in first.chars() {
        if character.is_control() {
            if !in_control {
                title.push(' ');
            }
            in_control = true;
        } else {
            title.push(character);
            in_control = false;
        }
    }
    let title: String = title.trim().chars().take(200).collect();
    let title = title.trim_end().to_owned();
    if title.is_empty() {
        "Run".to_owned()
    } else {
        title
    }
}

/// Open the record of a new built-in run, queued. Call before the detail row
/// is written: the detail row's id references it.
pub async fn open(
    transaction: &mut Transaction<'_, Postgres>,
    run: &NewBuiltinRun<'_>,
) -> Result<(), AppError> {
    // A2.14: refused once its agent's budget on what runs cost is used up,
    // as a run on any host is (`runs::ops::open_with`).
    let account =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM execution_workers WHERE id = $1")
            .bind(run.execution_worker_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(AppError::NotFound("execution_worker"))?;
    Box::pin(crate::budgets::admit_run(
        transaction,
        run.project_id,
        Some(run.conversation_id),
        account,
    ))
    .await?;
    let opened = sqlx::query(
        r#"
        INSERT INTO runs (
            id, workspace_id, space_id, account_id, title, cause_kind, conversation_id,
            requested_by_account_id, sponsored_by_account_id, predecessor_run_id, state,
            settings, last_report_at, host_id
        )
        SELECT $1, project.organization_id, project.id, worker.actor_id, $4, $5, $6,
               $7, $8, $9, 'queued', $10, now(), host.id
          FROM projects project
          JOIN execution_workers worker ON worker.id = $3
          -- A1.16: the workspace's built-in host runs it, as a host does.
          JOIN hosts host ON host.owner_workspace_id = project.organization_id AND host.builtin
         WHERE project.id = $2
        "#,
    )
    .bind(run.id)
    .bind(run.project_id)
    .bind(run.execution_worker_id)
    .bind(title_from_prompt(run.prompt))
    .bind(if run.from_request {
        "request"
    } else {
        "thread"
    })
    .bind(run.conversation_id)
    .bind(run.requested_by)
    .bind(run.sponsored_by)
    .bind(run.predecessor_run_id)
    .bind(run.settings)
    .execute(&mut **transaction)
    .await?;
    if opened.rows_affected() != 1 {
        return Err(AppError::NotFound("execution_worker"));
    }
    // A1.16: the run's agent reports on it through the host protocol, as
    // itself, so it must see where it works: in a members-only channel (or
    // a thread of one) it joins, added by whoever asked for the run.
    sqlx::query(
        r#"
        INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
        SELECT root.project_id, root.id, worker.actor_id, COALESCE($3, worker.actor_id)
          -- visibility: the run's own conversation, which its requester could start it in
          FROM conversations conversation
          JOIN conversations root ON root.id = COALESCE(conversation.parent_channel_id, conversation.id)
          JOIN execution_workers worker ON worker.id = $2
         WHERE conversation.id = $1 AND root.kind = 'channel' AND root.visibility = 'members'
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(run.conversation_id)
    .bind(run.execution_worker_id)
    .bind(run.requested_by)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// A launch started this built-in run: its record names the launch and, for
/// a task, the task as its cause, as a run on any other host or agent does
/// (A1.13).
pub async fn launched(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    launch_id: Uuid,
    work_item_id: Option<Uuid>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE runs
           SET launch_id = $2,
               work_item_id = COALESCE($3, work_item_id),
               cause_kind = CASE WHEN $3::uuid IS NULL THEN cause_kind ELSE 'task' END,
               version = version + 1,
               updated_at = now()
         WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(launch_id)
    .bind(work_item_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Where a built-in run is now, as its detail row says after a change.
pub struct Moved<'a> {
    pub state: BuiltinState,
    pub failure_reason: Option<&'a str>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Move a built-in run's record with its host state. The caller holds the
/// detail row (`hosts::builtin::lock`).
pub async fn moved(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    moved: Moved<'_>,
) -> Result<(), AppError> {
    let state = RunState::from_legacy(moved.state, moved.failure_reason);
    let status_line =
        (moved.state == BuiltinState::Recovering).then_some("Recovering after a restart");
    let failure_reason = moved
        .failure_reason
        .map(|reason| reason.chars().take(2_000).collect::<String>());
    sqlx::query(
        r#"
        UPDATE runs
           SET state = $2,
               status_line = $3,
               failure_reason = $4,
               started_at = $5,
               finished_at = CASE WHEN $6 THEN COALESCE($7, now()) END,
               last_report_at = now(),
               liveness = 'ok',
               version = version + 1,
               updated_at = now()
         WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(state.as_str())
    .bind(status_line)
    .bind(failure_reason)
    .bind(moved.started_at)
    .bind(state.is_terminal())
    .bind(moved.finished_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The settings a built-in run froze once its harness chose them, on its
/// record too (only while the record has none).
pub async fn settings_chosen(
    executor: impl sqlx::PgExecutor<'_>,
    run_id: Uuid,
    settings: &Value,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE runs SET settings = $2, updated_at = now() WHERE id = $1 AND settings = '{}'::jsonb",
    )
    .bind(run_id)
    .bind(settings)
    .execute(executor)
    .await?;
    Ok(())
}

/// Where a built-in run is, locked for a change: its host state, thread and
/// space. `None` when there is no such run.
pub struct Place {
    pub state: BuiltinState,
    pub conversation_id: Uuid,
    pub space_id: Uuid,
}

pub async fn lock_place(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<Place>, AppError> {
    let row = sqlx::query_as::<_, (String, Uuid, Uuid)>(
        r#"
        SELECT run.state, run.conversation_id, conversation.project_id
        -- visibility: the run's own thread, to write its steps
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        WHERE run.id = $1
        FOR UPDATE OF run
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?;
    row.map(|(state, conversation_id, space_id)| {
        Ok(Place {
            state: state.parse::<BuiltinState>()?,
            conversation_id,
            space_id,
        })
    })
    .transpose()
}

/// A built-in run's host state, if there is such a run.
pub async fn state_of(
    executor: impl sqlx::PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Option<BuiltinState>, AppError> {
    sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(executor)
        .await?
        .map(|state| state.parse::<BuiltinState>())
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::title_from_prompt;

    #[test]
    fn a_title_is_the_prompts_first_line() {
        assert_eq!(
            title_from_prompt("  Fix the login bug\nDetails"),
            "Fix the login bug"
        );
        assert_eq!(title_from_prompt("a\u{1}\u{2}b"), "a b");
        assert_eq!(title_from_prompt("   \n"), "Run");
        assert_eq!(title_from_prompt(&"x".repeat(300)).chars().count(), 200);
    }
}

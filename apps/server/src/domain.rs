use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, external_events::enqueue_external_event};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Created,
    Queued,
    Starting,
    Working,
    Blocked,
    Recovering,
    Completed,
    Failed,
    Canceled,
    Interrupted,
}

impl RunState {
    pub const ALL: [Self; 10] = [
        Self::Created,
        Self::Queued,
        Self::Starting,
        Self::Working,
        Self::Blocked,
        Self::Recovering,
        Self::Completed,
        Self::Failed,
        Self::Canceled,
        Self::Interrupted,
    ];

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Canceled | Self::Interrupted
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Created,
                Self::Queued | Self::Starting | Self::Failed | Self::Canceled
            ) | (Self::Queued, Self::Starting | Self::Failed | Self::Canceled)
                | (
                    Self::Starting,
                    Self::Working
                        | Self::Blocked
                        | Self::Recovering
                        | Self::Failed
                        | Self::Canceled
                        | Self::Interrupted
                )
                | (
                    Self::Working,
                    Self::Blocked
                        | Self::Recovering
                        | Self::Completed
                        | Self::Failed
                        | Self::Canceled
                        | Self::Interrupted
                )
                | (
                    Self::Blocked,
                    Self::Working
                        | Self::Recovering
                        | Self::Failed
                        | Self::Canceled
                        | Self::Interrupted
                )
                | (
                    Self::Recovering,
                    Self::Working
                        | Self::Blocked
                        | Self::Failed
                        | Self::Canceled
                        | Self::Interrupted
                )
        )
    }
}

impl fmt::Display for RunState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Recovering => "recovering",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Interrupted => "interrupted",
        })
    }
}

impl FromStr for RunState {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "created" => Ok(Self::Created),
            "queued" => Ok(Self::Queued),
            "starting" => Ok(Self::Starting),
            "working" => Ok(Self::Working),
            "blocked" => Ok(Self::Blocked),
            "recovering" => Ok(Self::Recovering),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(AppError::Internal("unknown run state")),
        }
    }
}

/// Records, for this transaction only, which connected app a person is acting
/// through, so rows written by shared command code carry it: messages,
/// task activity and the audit rows that read `current_via_grant()`
/// (migration 0042). `None` (the browser, or an agent) leaves it unset.
pub(crate) async fn set_via_grant(
    transaction: &mut Transaction<'_, Postgres>,
    grant_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(grant_id) = grant_id {
        sqlx::query("SELECT set_config('aw.via_grant', $1, true)")
            .bind(grant_id.to_string())
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

pub async fn allocate_conversation_sequence(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
) -> Result<i64, AppError> {
    let sequence = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE conversations
           SET next_entry_sequence = next_entry_sequence + 1,
               updated_at = now()
         WHERE id = $1
         RETURNING next_entry_sequence - 1
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    Ok(sequence)
}

pub async fn transition_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    next: RunState,
    reason: Option<&str>,
) -> Result<RunState, AppError> {
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(AppError::NotFound("agent_run"))?
            .parse::<RunState>()?;

    if current == next {
        return Ok(current);
    }
    if !current.can_transition_to(next) {
        return Err(AppError::Conflict("agent_run", "invalid state transition"));
    }

    let terminal = next.is_terminal();
    sqlx::query(
        r#"
        UPDATE agent_runs
           SET state = $2,
               failure_reason = COALESCE($3, failure_reason),
               started_at = CASE
                   WHEN $2 IN ('starting', 'working') THEN COALESCE(started_at, now())
                   ELSE started_at
               END,
               finished_at = CASE WHEN $4 THEN now() ELSE finished_at END,
               updated_at = now()
         WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(next.to_string())
    .bind(reason)
    .bind(terminal)
    .execute(&mut **transaction)
    .await?;

    let event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (
            id, agent_run_id, event_key, kind, summary, payload
        )
        VALUES ($1, $2, $3, 'run_state_changed', $4, $5)
        "#,
    )
    .bind(event_id)
    .bind(run_id)
    .bind(format!("state:{next}:{event_id}"))
    .bind(format!("Run moved from {current} to {next}."))
    .bind(serde_json::json!({
        "from": current,
        "to": next,
        "reason": reason,
    }))
    .execute(&mut **transaction)
    .await?;

    if terminal {
        enqueue_external_terminal_run_event(transaction, run_id, next).await?;
    }

    Ok(next)
}

async fn enqueue_external_terminal_run_event(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    state: RunState,
) -> Result<(), AppError> {
    let available =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('external_agent_events') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await?;
    if !available {
        return Ok(());
    }
    let recipients = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        r#"
        SELECT connection.id, conversation.project_id, run.conversation_id
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        JOIN actors actor ON actor.id = run.requesting_actor_id
         AND actor.kind = 'external_agent'
        JOIN external_agent_connections connection
          ON connection.project_id = conversation.project_id
         AND connection.actor_id = run.requesting_actor_id
         AND connection.status = 'active'
        JOIN project_memberships membership
          ON membership.project_id = connection.project_id
         AND membership.actor_id = connection.actor_id
         AND membership.enabled
        WHERE run.id = $1 AND actor.status = 'active'
        ORDER BY connection.id
        FOR SHARE OF connection, actor, membership
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (connection_id, project_id, conversation_id) in recipients {
        enqueue_external_event(
            transaction,
            connection_id,
            project_id,
            "agent_run.terminal",
            &format!("Sponsored Worker run reached {state}"),
            "agent_run",
            run_id,
            &format!("/projects/{project_id}/threads/{conversation_id}"),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::RunState;

    #[test]
    fn terminal_states_never_transition() {
        for state in RunState::ALL
            .into_iter()
            .filter(|state| state.is_terminal())
        {
            for next in RunState::ALL {
                assert!(!state.can_transition_to(next), "{state} -> {next}");
            }
        }
    }

    #[test]
    fn transition_table_matches_the_source_of_truth() {
        let allowed = [
            (RunState::Created, RunState::Queued),
            (RunState::Created, RunState::Starting),
            (RunState::Created, RunState::Failed),
            (RunState::Created, RunState::Canceled),
            (RunState::Queued, RunState::Starting),
            (RunState::Queued, RunState::Failed),
            (RunState::Queued, RunState::Canceled),
            (RunState::Starting, RunState::Working),
            (RunState::Starting, RunState::Blocked),
            (RunState::Starting, RunState::Recovering),
            (RunState::Starting, RunState::Failed),
            (RunState::Starting, RunState::Canceled),
            (RunState::Starting, RunState::Interrupted),
            (RunState::Working, RunState::Blocked),
            (RunState::Working, RunState::Recovering),
            (RunState::Working, RunState::Completed),
            (RunState::Working, RunState::Failed),
            (RunState::Working, RunState::Canceled),
            (RunState::Working, RunState::Interrupted),
            (RunState::Blocked, RunState::Working),
            (RunState::Blocked, RunState::Recovering),
            (RunState::Blocked, RunState::Failed),
            (RunState::Blocked, RunState::Canceled),
            (RunState::Blocked, RunState::Interrupted),
            (RunState::Recovering, RunState::Working),
            (RunState::Recovering, RunState::Blocked),
            (RunState::Recovering, RunState::Failed),
            (RunState::Recovering, RunState::Canceled),
            (RunState::Recovering, RunState::Interrupted),
        ];

        for current in RunState::ALL {
            for next in RunState::ALL {
                assert_eq!(
                    current.can_transition_to(next),
                    allowed.contains(&(current, next)),
                    "{current} -> {next}"
                );
            }
        }
    }
}

//! A1.16: what the built-in host tells the server about a run goes only
//! through the host protocol's run reports; the reports themselves live in
//! the built-in host's crate. Here: finding the start a run's reports
//! fetch their credential from.

use uuid::Uuid;

pub use agent_workspace_builtin_host::runtime::report::{RunReports, activity};

use crate::{AppState, error::AppError};

/// A run the built-in host started before its runs went through the host
/// protocol has no start command: it gets one now, already accepted, so
/// its reports carry a credential like any run's and recovery can finish
/// it. Once only: the run's row is locked, and the start key keeps one
/// start per run.
async fn adopt(state: &AppState, run_id: Uuid) -> Result<Uuid, AppError> {
    use crate::hosts::dispatch::{Host, RemoteHost, StartInput};
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let held = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT start_command_id FROM agent_runs WHERE id = $1 FOR UPDATE",
    )
    .bind(run_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("agent_run"))?;
    if let Some(command_id) = held {
        transaction.commit().await?;
        return Ok(command_id);
    }
    let run = crate::runs::ops::locked(&mut transaction, run_id)
        .await?
        .ok_or(AppError::NotFound("agent_run"))?;
    let host_id = match run.host_id {
        Some(host_id) => host_id,
        None => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM hosts WHERE owner_workspace_id = $1 AND builtin",
            )
            .bind(run.workspace_id)
            .fetch_one(&mut *transaction)
            .await?
        }
    };
    let command_id = RemoteHost { id: host_id }
        .start(&mut transaction, &run, "builtin", StartInput::default())
        .await?;
    crate::hosts::commands::adopted(&mut transaction, command_id).await?;
    sqlx::query("UPDATE agent_runs SET start_command_id = $2 WHERE id = $1")
        .bind(run_id)
        .bind(command_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(command_id)
}

/// The reports of a run the built-in host started, by its start command
/// (fetched again for a credential).
pub async fn open(state: &AppState, run_id: Uuid) -> Result<RunReports, AppError> {
    let start_command_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT start_command_id FROM agent_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    .ok_or(AppError::NotFound("agent_run"))?;
    let start_command_id = match start_command_id {
        Some(id) => id,
        None => adopt(state, run_id).await?,
    };
    Ok(RunReports::for_start(super::transport::client(state), run_id, start_command_id).await?)
}

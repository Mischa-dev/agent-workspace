//! A2.26: `POST /api/v1/host/runs`. A host tells Workspace about a run it
//! started itself for one of its bound agents (a schedule or trigger on the
//! host's side). The run is opened once, as the agent's, in the host's
//! workspace, with a start already accepted: fetching that start gives the
//! host the run's credential, and it reports on the run like any other.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    HostRow, commands,
    dispatch::{Host, RemoteHost, StartInput},
    protocol::HostError,
};
use crate::{AppState, runs::ops::RunOpError};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdoptRequest {
    /// The bound agent the run is for.
    pub host_agent_ref: String,
    /// The host's own id for the run: the same id finds the same run.
    pub host_run_ref: String,
    /// What the run is doing, on one line.
    pub title: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adopted {
    pub run_id: Uuid,
    /// The run's start, already accepted: fetch it for the credential.
    pub command_id: Uuid,
    /// False when the run was adopted before.
    pub created: bool,
}

fn reference(field: &'static str, value: &str) -> Result<String, HostError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 200 || value.chars().any(char::is_control) {
        return Err(HostError::Invalid {
            field,
            message: format!("{field} is 1 to 200 characters on one line."),
        });
    }
    Ok(value.to_owned())
}

pub async fn adopt(
    state: &AppState,
    host: &HostRow,
    request: AdoptRequest,
) -> Result<Adopted, HostError> {
    let host_agent_ref = reference("hostAgentRef", &request.host_agent_ref)?;
    let host_run_ref = reference("hostRunRef", &request.host_run_ref)?;
    let title = crate::runs::builtin::title_from_prompt(&request.title);
    let Some(workspace_id) = host.owner_workspace_id else {
        return Err(HostError::Invalid {
            field: "hostRunRef",
            message: "Only a workspace's own hosts can add runs they started; a personal host's runs start from Workspace."
                .to_owned(),
        });
    };
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let account_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT account_id FROM host_agents WHERE host_id = $1 AND host_agent_ref = $2 AND status = 'active'",
    )
    .bind(host.id)
    .bind(&host_agent_ref)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(HostError::AgentUnknown)?;
    if let Some(run_id) =
        crate::runs::hosted::by_host_ref(&mut transaction, host.id, &host_run_ref).await?
    {
        let command_id = commands::start_of(&mut transaction, run_id)
            .await?
            .ok_or(HostError::CommandUnknown)?;
        transaction.commit().await?;
        return Ok(Adopted {
            run_id,
            command_id,
            created: false,
        });
    }
    // The agent works in the workspace's own space, as a member of the
    // workspace: an agent no longer in it gets no run there.
    let space_id = crate::spaces::workspace::join(&mut transaction, workspace_id, account_id)
        .await?
        .ok_or(HostError::AgentUnknown)?;
    let opened = crate::runs::hosted::adopt(
        &mut transaction,
        host.id,
        account_id,
        space_id,
        &host_run_ref,
        &title,
    )
    .await
    .map_err(|error| match error {
        RunOpError::Refused { field, message } => HostError::Invalid { field, message },
        RunOpError::Conflict(message) => HostError::Invalid {
            field: "hostRunRef",
            message,
        },
        RunOpError::NotFound | RunOpError::VersionConflict { .. } => HostError::AgentUnknown,
        RunOpError::App(error) => HostError::App(error),
    })?;
    let command_id = RemoteHost { id: host.id }
        .start(
            &mut transaction,
            &opened.run,
            &host_agent_ref,
            StartInput::default(),
        )
        .await?;
    commands::adopted(&mut transaction, command_id).await?;
    transaction.commit().await?;
    Ok(Adopted {
        run_id: opened.run.id,
        command_id,
        created: opened.created,
    })
}

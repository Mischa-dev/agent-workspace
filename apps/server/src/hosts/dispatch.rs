//! A1.12: the `Host` seam. Workspace asks a host to do something for a
//! run through this trait; [`RemoteHost`] writes the request as a
//! `host_commands` row the host reads (long-poll, stream or push). The
//! built-in host implements the same trait in process (A1.14), and A1.13's
//! one launch path calls [`start_run`] for an agent bound to a host.

use agent_workspace_host_protocol::CommandKind;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::commands::{self, CommandRow, NewCommand};
use crate::{
    error::AppError,
    runs::{
        ops::{self, RunOpError},
        store::RunRow,
    },
};

/// What a start carries besides the run itself.
#[derive(Clone, Debug, Default)]
pub struct StartInput {
    /// What the agent is asked to do. The run's title when `None`.
    pub prompt: Option<String>,
    /// Backend and settings the launch chose (never a secret).
    pub settings: Option<Value>,
}

/// One host as Workspace drives it. Every method writes inside the caller's
/// transaction, so the request and the run change land together.
#[async_trait]
pub trait Host: Send + Sync {
    fn host_id(&self) -> Uuid;

    /// Ask the host to start the run (already `starting` on this host).
    async fn start(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        host_agent_ref: &str,
        input: StartInput,
    ) -> Result<Uuid, AppError>;

    /// Ask the host to stop the run.
    async fn cancel(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        by: Uuid,
        reason: Option<&str>,
    ) -> Result<(), AppError>;

    /// Tell the running agent about a new message for it.
    async fn steer(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        message_id: Uuid,
    ) -> Result<Uuid, AppError>;

    /// Pass on the answer to a request the run asked.
    async fn answer(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        request_id: Uuid,
    ) -> Result<Uuid, AppError>;

    /// Ask whether a start that wasn't confirmed is running.
    async fn inspect(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
    ) -> Result<Uuid, AppError>;

    /// Run one of an agent's declared actions.
    async fn action(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        workspace_id: Uuid,
        host_agent_ref: &str,
        action: &str,
        input: Value,
    ) -> Result<Uuid, AppError>;
}

/// A host outside this server, reached through `host_commands`.
#[derive(Clone, Copy, Debug)]
pub struct RemoteHost {
    pub id: Uuid,
}

impl RemoteHost {
    async fn for_run(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        kind: CommandKind,
        request_id: Option<Uuid>,
        body: Value,
    ) -> Result<CommandRow, AppError> {
        commands::issue(
            transaction,
            NewCommand {
                workspace_id: run.workspace_id,
                host_id: self.id,
                run_id: Some(run.id),
                request_id,
                kind,
                start_key: None,
                body,
            },
        )
        .await
    }
}

#[async_trait]
impl Host for RemoteHost {
    fn host_id(&self) -> Uuid {
        self.id
    }

    async fn start(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        host_agent_ref: &str,
        input: StartInput,
    ) -> Result<Uuid, AppError> {
        let prompt = input.prompt.unwrap_or_else(|| run.title.clone());
        let body = json!({
            "agent": {"accountId": run.account_id, "hostAgentRef": host_agent_ref},
            "input": {"prompt": prompt},
            "cause": {
                "kind": run.cause_kind,
                "spaceId": run.space_id,
                "conversationId": run.conversation_id,
                "taskId": run.work_item_id,
                "messageId": run.cause_message_id,
            },
            "title": run.title,
            "requestedBy": run.requested_by_account_id.map(|id| json!({"accountId": id})),
            "settings": input.settings.unwrap_or_else(|| run.settings.clone()),
            // A repository token comes with A1.18's code-host seam.
            "repository": Value::Null,
        });
        let row = commands::issue(
            transaction,
            NewCommand {
                workspace_id: run.workspace_id,
                host_id: self.id,
                run_id: Some(run.id),
                request_id: None,
                kind: CommandKind::Start,
                start_key: Some(format!("run-{}", run.id)),
                body,
            },
        )
        .await?;
        Ok(row.id)
    }

    async fn cancel(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        by: Uuid,
        reason: Option<&str>,
    ) -> Result<(), AppError> {
        commands::cancel_run(transaction, run, by, reason).await
    }

    async fn steer(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        message_id: Uuid,
    ) -> Result<Uuid, AppError> {
        // The host reads the message with the run's credential: only its id
        // travels.
        let body = json!({"hostRunRef": run.host_run_ref, "messageId": message_id});
        Ok(self
            .for_run(transaction, run, CommandKind::Steer, None, body)
            .await?
            .id)
    }

    async fn answer(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
        request_id: Uuid,
    ) -> Result<Uuid, AppError> {
        let body = json!({"hostRunRef": run.host_run_ref});
        Ok(self
            .for_run(
                transaction,
                run,
                CommandKind::Answer,
                Some(request_id),
                body,
            )
            .await?
            .id)
    }

    async fn inspect(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        run: &RunRow,
    ) -> Result<Uuid, AppError> {
        let body = json!({"startKey": format!("run-{}", run.id), "hostRunRef": run.host_run_ref});
        Ok(self
            .for_run(transaction, run, CommandKind::Inspect, None, body)
            .await?
            .id)
    }

    async fn action(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        workspace_id: Uuid,
        host_agent_ref: &str,
        action: &str,
        input: Value,
    ) -> Result<Uuid, AppError> {
        Ok(commands::issue(
            transaction,
            NewCommand {
                workspace_id,
                host_id: self.id,
                run_id: None,
                request_id: None,
                kind: CommandKind::Action,
                start_key: None,
                body: json!({"hostAgentRef": host_agent_ref, "action": action, "input": input}),
            },
        )
        .await?
        .id)
    }
}

/// Why a run couldn't be handed to a host.
#[derive(Debug)]
pub enum StartError {
    /// The run's agent is bound to no active host.
    NoHost,
    Run(RunOpError),
    App(AppError),
}

impl From<AppError> for StartError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for StartError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

impl From<RunOpError> for StartError {
    fn from(error: RunOpError) -> Self {
        Self::Run(error)
    }
}

/// Hand a queued run to the host its agent is bound to: the run is
/// `starting` and the host has a start command for it. Starting the same
/// run twice is refused by the run's state, and the start key keeps one
/// command per run.
pub async fn start_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    input: StartInput,
) -> Result<(RunRow, Uuid), StartError> {
    let run = ops::locked(transaction, run_id)
        .await?
        .ok_or(StartError::Run(RunOpError::NotFound))?;
    let binding = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT binding.host_id, binding.host_agent_ref
        FROM host_agents binding
        JOIN hosts host ON host.id = binding.host_id
        WHERE binding.account_id = $1 AND binding.status = 'active'
          AND host.status <> 'revoked' AND NOT host.builtin
        ORDER BY (host.status = 'active') DESC, host.last_seen_at DESC NULLS LAST
        LIMIT 1
        "#,
    )
    .bind(run.account_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((host_id, host_agent_ref)) = binding else {
        return Err(StartError::NoHost);
    };
    let run = ops::dispatched(transaction, run.id, host_id).await?;
    let host = RemoteHost { id: host_id };
    let command_id = host
        .start(transaction, &run, &host_agent_ref, input)
        .await?;
    Ok((run, command_id))
}

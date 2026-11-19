//! Long operations (K2.21): a call that would take a while is answered
//! `started` and finished by a job, so a client never holds a request open
//! for it.
//!
//! An operation with `LONG` decides per call (`runs_long`: a big batch, a
//! commit); only the MCP edge defers (`DispatchRequest::defer_long`), after
//! the dispatcher has decoded, located and checked the call. The call is
//! kept in `operation_tasks` with the caller as the edge resolved it (ids
//! and flags, never a token), and a job (`ops.task`, its payload only the
//! task's id) runs it again through the dispatcher, as that caller, checked
//! again. The answer is stored there, so any server process can tell the
//! caller: a client that declared the MCP Tasks extension gets a task handle
//! and asks `tasks/get`; any other client gets `{outcome: "started", value:
//! {jobId}}` and asks `job_get`. Both read only the caller's own tasks.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use rmcp::{
    model::{CallToolResult, DetailedTask, Task, TaskPayload, TaskStatus},
    schemars::{self, JsonSchema},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    Answer, Caller, DispatchRequest, Edge, Idempotency, Located, OpCall, OpError, OpKind,
    Operation, Perm, Registry, Target, Targeted, Unkeyed, dispatch, refs::parse_id,
};
use crate::{
    AppState,
    error::AppError,
    idempotency::RequestHash,
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    policy::ActorKind,
};

/// The job that runs a deferred call.
pub const JOB_KIND: &str = "ops.task";
/// How often a client should ask again, in milliseconds.
pub const POLL_INTERVAL_MS: u64 = 2_000;
/// How long a finished task's answer is kept (retention deletes it after).
pub const KEEP_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
/// Attempts before a task is answered `unavailable`.
const ATTEMPTS: i32 = 3;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<JobGet>();
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(JOB_KIND, |context: JobContext| async move {
        run_task(context).await
    });
}

// ---------------------------------------------------------------------------
// The caller, kept

/// The caller as the edge resolved it, kept to run the call again later:
/// who, through which credential and with what reach. A person's app is
/// held to their delegation limit, which is worked out again when it runs.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeptCaller {
    account_id: Uuid,
    kind: String,
    via_grant_id: Option<Uuid>,
    can_read: bool,
    can_write: bool,
    connection_id: Option<Uuid>,
    confined_space: Option<Uuid>,
    delegated: bool,
    requested_by: Option<Uuid>,
    run_id: Option<Uuid>,
    /// Through a host's run credential (A1.12); such calls aren't kept.
    #[serde(default)]
    for_run: bool,
    /// The credential the call came through, which must still stand.
    credential_id: Option<Uuid>,
    oauth_grant_id: Option<Uuid>,
}

/// The credential and OAuth grant a call came through.
fn credential_of(edge: &Edge) -> (Option<Uuid>, Option<Uuid>) {
    match edge {
        Edge::Mcp {
            credential_id,
            oauth_grant_id,
            ..
        }
        | Edge::PublicApi {
            credential_id,
            oauth_grant_id,
            ..
        } => (*credential_id, *oauth_grant_id),
        _ => (None, None),
    }
}

const KINDS: &[(ActorKind, &str)] = &[
    (ActorKind::Human, "human"),
    (ActorKind::ExternalAgent, "external_agent"),
    (ActorKind::ExecutionWorker, "execution_worker"),
    (ActorKind::Integration, "integration"),
    (ActorKind::System, "system"),
];

impl KeptCaller {
    fn of(caller: &Caller, edge: &Edge) -> Self {
        let (credential_id, oauth_grant_id) = credential_of(edge);
        Self {
            account_id: caller.account_id,
            kind: KINDS
                .iter()
                .find(|(kind, _)| *kind == caller.kind)
                .map_or("human", |(_, name)| name)
                .to_owned(),
            via_grant_id: caller.via_grant_id,
            can_read: caller.can_read,
            can_write: caller.can_write,
            connection_id: caller.connection_id,
            confined_space: caller.confined_space,
            delegated: caller.limits.is_some(),
            requested_by: caller.requested_by,
            run_id: caller.run_id,
            for_run: caller.for_run,
            credential_id,
            oauth_grant_id,
        }
    }

    /// The caller again, if its account and credential still stand.
    async fn again(&self, state: &AppState) -> Result<Option<Caller>, AppError> {
        let Some(kind) = KINDS
            .iter()
            .find(|(_, name)| *name == self.kind)
            .map(|(kind, _)| *kind)
        else {
            return Ok(None);
        };
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        // A run credential's calls are never kept (ops/mcp.rs runs them at
        // once): one found here doesn't stand.
        if self.for_run {
            return Ok(None);
        }
        let standing = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (SELECT 1 FROM actors WHERE id = $1 AND status = 'active')
               AND ($2::uuid IS NULL OR EXISTS (
                   SELECT 1 FROM account_credentials credential
                   WHERE credential.id = $2 AND credential.status = 'active'
                     AND credential.revoked_at IS NULL))
               AND ($3::uuid IS NULL OR EXISTS (
                   SELECT 1 FROM mcp_oauth_grants grant_row
                   WHERE grant_row.id = $3 AND grant_row.revoked_at IS NULL))
            "#,
        )
        .bind(self.account_id)
        .bind(self.credential_id)
        .bind(self.oauth_grant_id)
        .fetch_one(&mut *connection)
        .await?;
        if !standing {
            return Ok(None);
        }
        let limits = if self.delegated {
            Some(
                crate::roles::app_access(&mut connection, self.account_id)
                    .await?
                    .limits,
            )
        } else {
            None
        };
        connection.commit().await?;
        Ok(Some(Caller {
            account_id: self.account_id,
            kind,
            via_grant_id: self.via_grant_id,
            can_read: self.can_read,
            can_write: self.can_write,
            connection_id: self.connection_id,
            confined_space: self.confined_space,
            limits,
            requested_by: self.requested_by,
            run_id: self.run_id,
            credential_id: self.credential_id,
            for_run: self.for_run,
        }))
    }
}

// ---------------------------------------------------------------------------
// Starting

/// Keep a checked call and queue the job that runs it; the answer says
/// `started`. A retry with the same request id finds the task it started.
pub(super) async fn start(
    state: &AppState,
    request: &DispatchRequest,
    op: &'static str,
    located: &Located,
    input: &Value,
    request_id: Option<&str>,
) -> Result<Answer, OpError> {
    let failed = |error: AppError| OpError::logged(op, &error);
    let hash = hex::encode(
        RequestHash::for_value(input)
            .map_err(|_| OpError::Unavailable)?
            .as_bytes(),
    );
    let caller = serde_json::to_value(KeptCaller::of(&request.caller, &request.edge))
        .map_err(|_| OpError::Unavailable)?;
    let mut tx = crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: Some(request.caller.account_id),
            workspace: located.workspace_id,
        },
        crate::db::scoped_tx(&state.pool),
    )
    .await
    .map_err(|error| failed(error.into()))?;
    let id = Uuid::now_v7();
    let inserted = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
        r#"
        INSERT INTO operation_tasks (
            id, workspace_id, space_id, account_id, op, request_id, input_hash, input, caller
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (account_id, op, request_id) WHERE request_id IS NOT NULL DO NOTHING
        RETURNING id, created_at
        "#,
    )
    .bind(id)
    .bind(located.workspace_id)
    .bind(located.space_id)
    .bind(request.caller.account_id)
    .bind(op)
    .bind(request_id)
    .bind(&hash)
    .bind(input)
    .bind(&caller)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| failed(error.into()))?;
    let (id, created_at, replayed) = match inserted {
        Some((id, created_at)) => {
            let job = NewJob::new(JOB_KIND)
                .payload(json!({ "taskId": id }))
                .max_attempts(ATTEMPTS);
            let job = match located.space_id {
                Some(space) => job.project(located.workspace_id, space),
                None => job.workspace(located.workspace_id),
            };
            crate::jobs::enqueue(&mut *tx, job).await.map_err(failed)?;
            (id, created_at, false)
        }
        None => {
            let (id, created_at, kept_hash) = sqlx::query_as::<_, (Uuid, DateTime<Utc>, String)>(
                r#"
                SELECT id, created_at, input_hash FROM operation_tasks
                WHERE account_id = $1 AND op = $2 AND request_id = $3
                "#,
            )
            .bind(request.caller.account_id)
            .bind(op)
            .bind(request_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| failed(error.into()))?;
            if kept_hash != hash {
                return Err(OpError::invalid(
                    "clientRequestId",
                    "That request id was used for a different call. Use a new one for this one.",
                ));
            }
            (id, created_at, true)
        }
    };
    tx.commit().await.map_err(|error| failed(error.into()))?;
    let mut body = json!({
        "outcome": "started",
        "value": {
            "jobId": id,
            "op": op,
            "status": "working",
            "createdAt": created_at,
        },
        "replayed": replayed,
    });
    if let Some(request_id) = request_id {
        body["requestId"] = request_id.into();
    }
    Ok(Answer {
        status: StatusCode::ACCEPTED,
        body,
    })
}

// ---------------------------------------------------------------------------
// Running

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobPayload {
    task_id: Uuid,
}

#[derive(FromRow)]
struct Claimed {
    op: String,
    workspace_id: Uuid,
    request_id: Option<String>,
    input: Value,
    caller: Value,
}

async fn run_task(context: JobContext) -> Result<(), JobError> {
    let JobPayload { task_id } = context.job.payload_as()?;
    let state = &context.state;
    let claimed = sqlx::query_as::<_, Claimed>(
        r#"
        UPDATE operation_tasks SET status = 'working', updated_at = now()
        WHERE id = $1 AND finished_at IS NULL AND input IS NOT NULL
        RETURNING op, workspace_id, request_id, input, caller
        "#,
    )
    .bind(task_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?;
    // Cancelled before it ran, or already answered.
    let Some(claimed) = claimed else {
        return Ok(());
    };
    let kept = serde_json::from_value::<KeptCaller>(claimed.caller)
        .map_err(|_| JobError::permanent("the task's caller doesn't parse"))?;
    let answer = match kept.again(state).await {
        Ok(Some(caller)) => {
            let request = DispatchRequest {
                caller,
                edge: Edge::Job {
                    job_id: context.job.id,
                    source: JOB_KIND,
                },
                trace_id: None,
                workspace: Some(claimed.workspace_id),
                // A job runs at least once: its own request id makes a second
                // attempt answer with the first one's result.
                request_id: Some(
                    claimed
                        .request_id
                        .unwrap_or_else(|| format!("task-{}", task_id.simple())),
                ),
                unkeyed: Unkeyed::RunOnce,
                preview: false,
                renamed_from: None,
                defer_long: false,
            };
            dispatch(state, request, &claimed.op, claimed.input).await
        }
        Ok(None) => Answer::error(&OpError::Unauthenticated),
        Err(error) if context.job.attempts < context.job.max_attempts => {
            return Err(error.into());
        }
        Err(_) => Answer::error(&OpError::Unavailable),
    };
    finish(state, task_id, &answer.body).await?;
    Ok(())
}

async fn finish(state: &AppState, task_id: Uuid, body: &Value) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE operation_tasks
        SET status = 'completed', result = $2, input = NULL, finished_at = now(), updated_at = now()
        WHERE id = $1 AND status = 'working'
        "#,
    )
    .bind(task_id)
    .bind(body)
    .execute(crate::db::scoped(&state.pool))
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading and cancelling

/// One of the caller's tasks.
#[derive(Clone, Debug)]
pub struct KeptTask {
    pub id: Uuid,
    pub op: String,
    pub status: String,
    pub result: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct TaskRow {
    id: Uuid,
    op: String,
    status: String,
    result: Option<Value>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
}

/// `reader`'s task `id`, or `None` for anyone else's or a missing one. Its
/// result is shown only while the reader, through the credential it calls
/// with now (its reach, the space it is held to, an app's delegation), may
/// still run the operation where it ran: someone who left the space, or a
/// read-only app of theirs, doesn't read a batch's or an export's answer.
pub async fn read(
    state: &AppState,
    reader: &Caller,
    id: Uuid,
) -> Result<Option<KeptTask>, AppError> {
    let Some(row) = sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT id, op, status, result, created_at, updated_at, workspace_id, space_id
        FROM operation_tasks
        WHERE id = $1 AND account_id = $2
        "#,
    )
    .bind(id)
    .bind(reader.account_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    else {
        return Ok(None);
    };
    let target = match row.space_id {
        Some(space) => Target::Space(space),
        None => Target::Workspace(Some(row.workspace_id)),
    };
    let result = match row.result {
        Some(result) => {
            match super::dispatch::may_run(state, reader, Some(row.workspace_id), &row.op, &target)
                .await
            {
                Ok(()) => Some(result),
                Err(_) => return Ok(None),
            }
        }
        None => None,
    };
    Ok(Some(KeptTask {
        id: row.id,
        op: row.op,
        status: row.status,
        result,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// Cancel `reader`'s task `id` if it hasn't started. `None` for anyone
/// else's or a missing one; a task already running finishes.
pub async fn cancel(
    state: &AppState,
    reader: &Caller,
    id: Uuid,
) -> Result<Option<KeptTask>, AppError> {
    if read(state, reader, id).await?.is_none() {
        return Ok(None);
    }
    sqlx::query(
        r#"
        UPDATE operation_tasks
        SET status = 'cancelled', input = NULL, finished_at = now(), updated_at = now()
        WHERE id = $1 AND account_id = $2 AND status = 'queued'
        "#,
    )
    .bind(id)
    .bind(reader.account_id)
    .execute(crate::db::scoped(&state.pool))
    .await?;
    read(state, reader, id).await
}

impl KeptTask {
    /// Queued and running both read as working.
    pub fn status_word(&self) -> &'static str {
        match self.status.as_str() {
            "completed" => "completed",
            "cancelled" => "cancelled",
            _ => "working",
        }
    }

    /// The task as the MCP Tasks extension describes it; a finished one
    /// carries the tool call's result (its envelope).
    pub fn detailed(&self) -> DetailedTask {
        let task = Task::new(
            self.id.to_string(),
            TaskStatus::Working,
            self.created_at.to_rfc3339(),
            self.updated_at.to_rfc3339(),
        )
        .with_ttl_ms(KEEP_MS)
        .with_poll_interval_ms(POLL_INTERVAL_MS);
        let payload = match (self.status_word(), &self.result) {
            ("completed", Some(envelope)) => {
                let result = serde_json::to_value(CallToolResult::structured(envelope.clone()))
                    .ok()
                    .and_then(|value| match value {
                        Value::Object(object) => Some(object),
                        _ => None,
                    })
                    .unwrap_or_default();
                TaskPayload::Completed { result }
            }
            ("cancelled", _) => TaskPayload::Cancelled,
            _ => TaskPayload::Working,
        };
        DetailedTask::new(task, payload)
    }
}

// ---------------------------------------------------------------------------
// job_get

pub(crate) struct JobGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct JobGetInput {
    /// The jobId a call answered "started" with.
    pub job_id: String,
}

impl Targeted for JobGetInput {
    fn target(&self) -> Target {
        Target::thing(
            "operation_task",
            parse_id(&self.job_id).unwrap_or_else(Uuid::nil),
        )
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobValue {
    #[schemars(with = "String")]
    pub job_id: Uuid,
    /// The operation it runs.
    pub op: String,
    /// working, completed or cancelled.
    pub status: &'static str,
    #[schemars(with = "String")]
    pub created_at: DateTime<Utc>,
    #[schemars(with = "String")]
    pub updated_at: DateTime<Utc>,
    /// Once completed: the call's answer, as it would have come back at
    /// once (outcome, value or the refusal).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// When to ask again while it works, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
}

impl Operation for JobGet {
    const NAME: &'static str = "job_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get job";
    const DESCRIPTION: &'static str = "Follow a call that answered \"started\" (a big batch or \
        import runs in the background): its status, and once completed the call's answer as it \
        would have come back at once. Only you can read your own.";

    type Input = JobGetInput;
    type Output = JobValue;

    async fn run(call: &mut OpCall<'_>, input: JobGetInput) -> Result<JobValue, OpError> {
        let id = parse_id(&input.job_id).ok_or(OpError::not_found("That job"))?;
        let task = read(call.state, &call.ctx.caller, id)
            .await
            .map_err(|error| OpError::logged(Self::NAME, &error))?
            .ok_or(OpError::not_found("That job"))?;
        let status = task.status_word();
        Ok(JobValue {
            job_id: task.id,
            op: task.op,
            status,
            created_at: task.created_at,
            updated_at: task.updated_at,
            poll_interval_ms: (status == "working").then_some(POLL_INTERVAL_MS),
            result: task.result,
        })
    }
}

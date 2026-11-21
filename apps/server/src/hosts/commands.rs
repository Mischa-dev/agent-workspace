//! A1.12: the host command channel (A1 section 3.9). Workspace writes
//! `host_commands`; a host learns of them by long-poll or a stream on
//! `GET /api/v1/host/commands`, or from a signed push, fetches each whole
//! (`GET /api/v1/host/commands/{id}`, where the run's credential is minted)
//! and acknowledges it once (`POST …/ack`).
//!
//! A start whose acknowledgement doesn't come within [`ACK_SECONDS`], or
//! that the host answers `uncertain`, leaves its run `start_uncertain`. A
//! host that declared `idempotentStart` is offered the same command again
//! (same id, same start key) on a backing-off schedule until it expires; any
//! other host is never re-offered: it is asked to `inspect` when it can, and
//! a person can mark the run as not started. No command body holds a secret.

use std::time::Duration;

use agent_workspace_host_protocol::{
    Ack, AckOutcome, AckReply, CommandKind, CommandList, CommandSummary, PushEvent, sign_push,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{HOST_COLUMNS, HostRow, protocol::HostError};
use crate::{
    AppState,
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    runs::{
        RunState,
        activities::redact_value,
        ops::{HostAnswer, RunOpError, host_answered},
        store::RunRow,
    },
};

/// How long a host has to acknowledge a start it fetched (or was pushed).
pub const ACK_SECONDS: i64 = 60;
/// How long a command is offered.
pub const LIFETIME_HOURS: i64 = 24;
/// The most commands one long-poll returns.
const MAX_LIST: i64 = 100;
/// The biggest acknowledgement kept (an action's reply).
const MAX_ACK_BYTES: usize = 65_536;
/// How often a waiting long-poll or stream looks again.
const POLL_EVERY: Duration = Duration::from_secs(1);

pub const SWEEP_KIND: &str = "hosts.command_sweep";
pub const PUSH_KIND: &str = "hosts.push_command";

/// When an uncertain start is offered again, after it was offered `offers`
/// times: 30 s, 2 min, 10 min, 30 min, then hourly.
pub fn reoffer_after(offers: i32) -> i64 {
    match offers {
        ..=1 => 30,
        2 => 120,
        3 => 600,
        4 => 1_800,
        _ => 3_600,
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct CommandRow {
    pub id: Uuid,
    pub seq: i64,
    pub workspace_id: Uuid,
    pub host_id: Uuid,
    pub run_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    pub kind: String,
    pub start_key: Option<String>,
    pub body: Value,
    pub state: String,
    pub attempts: i32,
    pub ack_due_at: Option<DateTime<Utc>>,
    pub ack: Option<Value>,
    pub acked_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

const COMMAND_COLUMNS: &str = "command.id, command.seq, command.workspace_id, command.host_id, \
     command.run_id, command.request_id, command.kind, command.start_key, command.body, \
     command.state, command.attempts, command.ack_due_at, command.ack, \
     command.acked_at, command.expires_at, command.created_at";

impl CommandRow {
    fn kind(&self) -> CommandKind {
        CommandKind::parse(&self.kind).unwrap_or(CommandKind::Action)
    }

    fn summary(&self) -> CommandSummary {
        CommandSummary {
            command_id: self.id,
            seq: self.seq,
            kind: self.kind(),
            run_id: self.run_id,
            offer: self.attempts.max(1),
        }
    }
}

fn capability(host: &HostRow, name: &str) -> bool {
    host.capabilities.get(name).and_then(Value::as_bool) == Some(true)
}

fn app(error: RunOpError) -> AppError {
    match error {
        RunOpError::App(error) => error,
        RunOpError::NotFound => AppError::NotFound("run"),
        RunOpError::Conflict(_) | RunOpError::VersionConflict { .. } => {
            AppError::Conflict("run", "changed")
        }
        RunOpError::Refused { field, .. } => AppError::Validation(field),
    }
}

// ------------------------------------------------------------------ write

/// A command to write. The body is what the host may read; never a secret.
#[derive(Clone, Debug)]
pub struct NewCommand {
    pub workspace_id: Uuid,
    pub host_id: Uuid,
    pub run_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    pub kind: CommandKind,
    pub start_key: Option<String>,
    pub body: Value,
}

/// Write a command for a host, or find the start it already has for the
/// same start key. A push host is sent it by a job.
pub async fn issue(
    transaction: &mut Transaction<'_, Postgres>,
    command: NewCommand,
) -> Result<CommandRow, AppError> {
    let body = redact_value(command.body);
    let inserted = sqlx::query_as::<_, CommandRow>(&format!(
        r#"
        INSERT INTO host_commands AS command (
            id, workspace_id, host_id, run_id, request_id, kind, start_key, body, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(hours => $9::int))
        ON CONFLICT (host_id, start_key) DO NOTHING
        RETURNING {COMMAND_COLUMNS}
        "#
    ))
    .bind(Uuid::now_v7())
    .bind(command.workspace_id)
    .bind(command.host_id)
    .bind(command.run_id)
    .bind(command.request_id)
    .bind(command.kind.as_str())
    .bind(command.start_key.as_deref())
    .bind(&body)
    .bind(LIFETIME_HOURS as i32)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(row) = inserted else {
        // The same start, issued again: there is only ever one.
        return Ok(sqlx::query_as::<_, CommandRow>(&format!(
            "SELECT {COMMAND_COLUMNS} FROM host_commands command WHERE command.host_id = $1 AND command.start_key = $2"
        ))
        .bind(command.host_id)
        .bind(command.start_key.as_deref())
        .fetch_one(&mut **transaction)
        .await?);
    };
    push_later(transaction, &row).await?;
    Ok(row)
}

/// A command the process that wrote it carries out itself (the server
/// asking its own built-in host, in process): taken at once, so no host
/// loop lists it too.
pub async fn take_here(
    transaction: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE host_commands
           SET state = 'fetched', attempts = 1,
               ack_due_at = now() + make_interval(secs => $2::double precision),
               updated_at = now()
         WHERE id = $1 AND state = 'pending'
        "#,
    )
    .bind(command_id)
    .bind(ACK_SECONDS as f64)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The host's acknowledgement of an action, once it has one.
pub async fn answer_of(pool: &PgPool, command_id: Uuid) -> Result<Option<Ack>, AppError> {
    let ack = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT ack FROM host_commands WHERE id = $1 AND state IN ('acked', 'refused')",
    )
    .bind(command_id)
    .fetch_optional(crate::db::scoped(pool))
    .await?
    .flatten();
    Ok(ack.and_then(|ack| serde_json::from_value(ack).ok()))
}

/// An action no host has taken up in time is closed: a host that comes to
/// it later finds it gone. One a host has fetched is left to finish.
/// Returns whether it was closed.
pub async fn close_if_not_taken(pool: &PgPool, command_id: Uuid) -> Result<bool, AppError> {
    let closed = sqlx::query(
        "UPDATE host_commands SET state = 'expired', ack_due_at = NULL, updated_at = now() \
         WHERE id = $1 AND state = 'pending'",
    )
    .bind(command_id)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(closed.rows_affected() > 0)
}

/// Once its asker has read the answer, a built-in host's account question
/// is gone: no row keeps what the account said, and usage polls don't pile
/// up. (A question still in flight stays until its host answers it.)
pub async fn forget_answered(pool: &PgPool, command_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM host_commands WHERE id = $1 AND kind = 'action' \
           AND state IN ('acked', 'refused', 'expired') \
           AND host_id IN (SELECT id FROM hosts WHERE builtin)",
    )
    .bind(command_id)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(())
}

/// For a push host, a job that sends the host this command's thin event;
/// for an A2A host, the job that makes the call.
async fn push_later(
    transaction: &mut Transaction<'_, Postgres>,
    row: &CommandRow,
) -> Result<(), AppError> {
    let (pushed, a2a) = sqlx::query_as::<_, (bool, bool)>(
        "SELECT connection = 'push' AND push_url IS NOT NULL AND status <> 'revoked', \
                connection = 'a2a' AND status <> 'revoked' \
         FROM hosts WHERE id = $1",
    )
    .bind(row.host_id)
    .fetch_one(&mut **transaction)
    .await?;
    if a2a {
        // An A2A agent is called by Workspace itself (A1.20).
        super::a2a::send_later(transaction, row.id, row.workspace_id).await?;
    } else if pushed {
        crate::jobs::enqueue(
            &mut **transaction,
            NewJob::new(PUSH_KIND)
                .payload(json!({"commandId": row.id}))
                .workspace(row.workspace_id)
                .unique(format!("host-push:{}:{}", row.id, row.attempts))
                .max_attempts(8),
        )
        .await?;
    }
    Ok(())
}

/// The run's start is offered no more: a person marked it as not started.
pub async fn close_start(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE host_commands SET state = 'expired', updated_at = now()
         WHERE run_id = $1 AND kind = 'start' AND state IN ('pending', 'fetched', 'uncertain')
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The run's start was confirmed some other way (a heartbeat listed it, an
/// inspect knew it): its start command is settled.
pub async fn start_confirmed(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE host_commands
           SET state = 'acked', acked_at = COALESCE(acked_at, now()), updated_at = now()
         WHERE run_id = $1 AND kind = 'start' AND state IN ('pending', 'fetched', 'uncertain')
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Ask a run's host to stop it. The run stays `cancel_requested` until the
/// host confirms (an acknowledgement `done`, or the run's own report).
pub async fn cancel_run(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    by: Uuid,
    reason: Option<&str>,
) -> Result<(), AppError> {
    let Some(host_id) = run.host_id else {
        return Ok(());
    };
    issue(
        transaction,
        NewCommand {
            workspace_id: run.workspace_id,
            host_id,
            run_id: Some(run.id),
            request_id: None,
            kind: CommandKind::Cancel,
            start_key: None,
            body: json!({
                "hostRunRef": run.host_run_ref,
                "reason": reason,
                "requestedBy": {"accountId": by},
            }),
        },
    )
    .await?;
    Ok(())
}

// ------------------------------------------------------------------- read

/// The commands due for `host` after its cursor `after`: new ones, and
/// uncertain starts due to be offered again. Reading counts as a heartbeat.
pub async fn due(pool: &PgPool, host: &HostRow, after: i64) -> Result<CommandList, AppError> {
    due_for(pool, Some(host), after).await
}

/// [`due`] for the built-in host (A1.16): the commands of every
/// workspace's built-in row, in one list (`seq` is global).
pub async fn due_builtin(pool: &PgPool, after: i64) -> Result<CommandList, AppError> {
    due_for(pool, None, after).await
}

async fn due_for(
    pool: &PgPool,
    host: Option<&HostRow>,
    after: i64,
) -> Result<CommandList, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    if let Some(host) = host {
        sqlx::query(
            r#"
        UPDATE hosts
           SET last_seen_at = now(),
               status = CASE WHEN status = 'offline' THEN 'active' ELSE status END,
               last_command_seq_fetched = GREATEST(last_command_seq_fetched, $2)
         WHERE id = $1
        "#,
        )
        .bind(host.id)
        .bind(after)
        .execute(&mut *transaction)
        .await?;
    }
    let rows = sqlx::query_as::<_, CommandRow>(&format!(
        r#"
        UPDATE host_commands command
           SET attempts = CASE WHEN command.state = 'uncertain' OR command.attempts = 0
                               THEN command.attempts + 1 ELSE command.attempts END,
               next_offer_at = CASE WHEN command.state = 'uncertain'
                                    THEN now() + make_interval(secs => $4::double precision)
                                    ELSE command.next_offer_at END,
               updated_at = now()
         WHERE command.id IN (
               SELECT due.id FROM host_commands due
               WHERE (due.host_id = $1
                      OR ($1 IS NULL AND due.host_id IN (SELECT id FROM hosts WHERE builtin)))
                 AND due.expires_at > now()
                 AND ((due.state = 'pending' AND due.seq > $2)
                      OR (due.state = 'uncertain' AND due.next_offer_at <= now()))
               ORDER BY due.seq
               LIMIT $3
               FOR UPDATE SKIP LOCKED)
        RETURNING {COMMAND_COLUMNS}
        "#
    ))
    .bind(host.map(|host| host.id))
    .bind(after)
    .bind(MAX_LIST)
    // The next re-offer of a start offered again now; each row's own
    // schedule is set below once its count is known.
    .bind(reoffer_after(1) as f64)
    .fetch_all(&mut *transaction)
    .await?;
    for row in rows.iter().filter(|row| row.state == "uncertain") {
        sqlx::query(
            "UPDATE host_commands SET next_offer_at = now() + make_interval(secs => $2::double precision) WHERE id = $1",
        )
        .bind(row.id)
        .bind(reoffer_after(row.attempts) as f64)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    let mut rows = rows;
    rows.sort_by_key(|row| row.seq);
    let next = rows
        .iter()
        .filter(|row| row.state == "pending")
        .map(|row| row.seq)
        .max()
        .unwrap_or(after)
        .max(after);
    Ok(CommandList {
        commands: rows.iter().map(CommandRow::summary).collect(),
        next,
    })
}

/// [`due`], waiting up to `wait` seconds (at most 30) for a command.
pub async fn long_poll(
    pool: &PgPool,
    host: &HostRow,
    after: i64,
    wait: u64,
) -> Result<CommandList, AppError> {
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(wait.min(agent_workspace_host_protocol::MAX_WAIT_SECONDS));
    loop {
        let listed = due(pool, host, after).await?;
        if !listed.commands.is_empty() || tokio::time::Instant::now() >= deadline {
            return Ok(listed);
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A command whole, as the host fetches it: a start's credential for the
/// run's agent is minted now and is in no row. Fetching starts the clock
/// on its acknowledgement.
pub async fn fetch(state: &AppState, host: &HostRow, command_id: Uuid) -> Result<Value, HostError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let row = sqlx::query_as::<_, CommandRow>(&format!(
        "SELECT {COMMAND_COLUMNS} FROM host_commands command WHERE command.id = $1 AND command.host_id = $2 FOR UPDATE"
    ))
    .bind(command_id)
    .bind(host.id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(HostError::CommandUnknown)?;
    // A command's lifetime bounds how long it waits to be carried out. A
    // start the host accepted stays fetchable past it while its run is
    // live, since fetching it is how the host gets a fresh credential for
    // a run that outlasts a day (no credential is minted once it ends).
    let accepted_start = row.kind == "start" && row.state == "acked";
    if matches!(row.state.as_str(), "expired" | "superseded")
        || (row.expires_at <= Utc::now() && !accepted_start)
    {
        return Err(HostError::CommandClosed);
    }
    if row.state == "pending" {
        sqlx::query(
            r#"
            UPDATE host_commands
               SET state = 'fetched', attempts = GREATEST(attempts, 1),
                   ack_due_at = COALESCE(ack_due_at, now() + make_interval(secs => $2::double precision)),
                   updated_at = now()
             WHERE id = $1
            "#,
        )
        .bind(row.id)
        .bind(ACK_SECONDS as f64)
        .execute(&mut *transaction)
        .await?;
    }
    let mut command = match row.body.clone() {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    command.insert("commandId".into(), json!(row.id));
    command.insert("seq".into(), json!(row.seq));
    command.insert("kind".into(), json!(row.kind));
    if let Some(run_id) = row.run_id {
        command.insert("runId".into(), json!(run_id));
    }
    if let Some(request_id) = row.request_id {
        command.insert("requestId".into(), json!(request_id));
        // The answer is read when fetched, never stored in the command.
        if row.kind == "answer"
            && let Some(request) = crate::requests::get(&mut transaction, request_id).await?
        {
            let options = crate::requests::options_of(&request.options);
            command.insert(
                "answer".into(),
                json!({
                    "kind": request.kind,
                    "status": request.status,
                    "answer": crate::requests::answer_of(request.answer.as_ref(), &options),
                    "answeredBy": request.answered_by_account_id,
                    "answeredAt": request.answered_at.map(timestamp),
                }),
            );
        }
    }
    if let Some(start_key) = &row.start_key {
        command.insert("startKey".into(), json!(start_key));
    }
    if row.kind == "action" && host.builtin {
        // An API key to sign in with is handed over now and kept in no
        // command (A1.17).
        crate::hosts::builtin::accounts::attach_secret(
            state,
            &mut transaction,
            &row.body,
            &mut command,
        )
        .await?;
    }
    command.insert("issuedAt".into(), json!(timestamp(row.created_at)));
    command.insert("expiresAt".into(), json!(timestamp(row.expires_at)));
    let runnable = row.kind == "start"
        && matches!(
            row.state.as_str(),
            "pending" | "fetched" | "uncertain" | "acked"
        );
    if runnable && let Some(run_id) = row.run_id {
        let active = crate::runs::ops::state_of(&mut *transaction, run_id)
            .await?
            .is_some_and(|(_, active)| active);
        if active
            && let Some(minted) =
                crate::runs::credentials::mint(&mut transaction, run_id, host.id, row.id).await?
        {
            command.insert(
                "workspaceCredential".into(),
                json!({
                    "token": minted.token,
                    "expiresAt": timestamp(minted.expires_at),
                    "scope": minted.scope,
                }),
            );
        }
    }
    if let Some(origin) = state.config.public_origin.as_deref() {
        let origin = origin.trim_end_matches('/');
        command.insert(
            "reportTo".into(),
            json!({"mcp": format!("{origin}/api/v1/mcp"), "api": format!("{origin}/api/v1")}),
        );
    }
    transaction.commit().await?;
    Ok(Value::Object(command))
}

// -------------------------------------------------------------------- ack

/// A start the host didn't confirm: its run is `start_uncertain`. An
/// idempotent host is offered it again; any other is asked to inspect when
/// it can, and otherwise a person decides.
async fn uncertain(
    transaction: &mut Transaction<'_, Postgres>,
    host: &HostRow,
    row: &CommandRow,
    message: &str,
) -> Result<Option<RunRow>, AppError> {
    let idempotent = capability(host, "idempotentStart");
    sqlx::query(
        r#"
        UPDATE host_commands
           SET state = 'uncertain', ack_due_at = NULL, updated_at = now(),
               next_offer_at = CASE WHEN $2 THEN now() + make_interval(secs => $3::double precision)
                                    ELSE 'infinity'::timestamptz END
         WHERE id = $1
        "#,
    )
    .bind(row.id)
    .bind(idempotent)
    .bind(reoffer_after(row.attempts) as f64)
    .execute(&mut **transaction)
    .await?;
    let Some(run_id) = row.run_id else {
        return Ok(None);
    };
    let run = host_answered(
        transaction,
        run_id,
        host.id,
        HostAnswer::Uncertain {
            message: message.to_owned(),
        },
    )
    .await
    .map_err(app)?;
    if !idempotent && capability(host, "inspect") && run.state == RunState::StartUncertain.as_str()
    {
        issue(
            transaction,
            NewCommand {
                workspace_id: row.workspace_id,
                host_id: host.id,
                run_id: Some(run_id),
                request_id: None,
                kind: CommandKind::Inspect,
                start_key: None,
                body: json!({"startKey": row.start_key}),
            },
        )
        .await?;
    }
    Ok(Some(run))
}

fn text(value: Option<&str>, fallback: &str) -> String {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    crate::run_log::redact(value.unwrap_or(fallback))
        .chars()
        .take(200)
        .collect()
}

/// The host's answer to a command. Answering again with the same outcome
/// changes nothing.
pub async fn ack(
    pool: &PgPool,
    host: &HostRow,
    command_id: Uuid,
    ack: Ack,
) -> Result<AckReply, HostError> {
    let outcome = ack.outcome.ok_or(HostError::Invalid {
        field: "outcome",
        message: "Give outcome: accepted, refused, uncertain or done.".to_owned(),
    })?;
    let stored = redact_value(serde_json::to_value(&ack).unwrap_or(Value::Null));
    if serde_json::to_vec(&stored).map_or(true, |bytes| bytes.len() > MAX_ACK_BYTES) {
        return Err(HostError::Invalid {
            field: "result",
            message: "Keep an acknowledgement under 64 KiB.".to_owned(),
        });
    }
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let row = sqlx::query_as::<_, CommandRow>(&format!(
        "SELECT {COMMAND_COLUMNS} FROM host_commands command WHERE command.id = $1 AND command.host_id = $2 FOR UPDATE"
    ))
    .bind(command_id)
    .bind(host.id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(HostError::CommandUnknown)?;
    let kind = row.kind();
    let fits = match outcome {
        AckOutcome::Accepted | AckOutcome::Refused => true,
        AckOutcome::Uncertain => kind == CommandKind::Start,
        AckOutcome::Done => kind != CommandKind::Start,
    };
    if !fits {
        return Err(HostError::Invalid {
            field: "outcome",
            message: format!("A {} command can't be answered {outcome:?}.", kind.as_str())
                .to_lowercase(),
        });
    }
    // Settled already: the first answer stands (a start acknowledged after
    // a person marked it as not started still revives its run).
    let settled = matches!(row.state.as_str(), "acked" | "refused" | "superseded");
    let revives =
        row.state == "expired" && kind == CommandKind::Start && outcome == AckOutcome::Accepted;
    if (settled || row.state == "expired") && !revives {
        let answer = reply(&mut transaction, &row, row.state.clone()).await?;
        transaction.commit().await?;
        return Ok(answer);
    }
    let next_state = match outcome {
        AckOutcome::Accepted | AckOutcome::Done => "acked",
        AckOutcome::Refused => "refused",
        AckOutcome::Uncertain => "uncertain",
    };
    if outcome != AckOutcome::Uncertain {
        sqlx::query(
            r#"
            UPDATE host_commands
               SET state = $2, ack = $3, acked_at = now(), ack_due_at = NULL, updated_at = now()
             WHERE id = $1
            "#,
        )
        .bind(row.id)
        .bind(next_state)
        .bind(&stored)
        .execute(&mut *transaction)
        .await?;
    } else {
        sqlx::query("UPDATE host_commands SET ack = $2 WHERE id = $1")
            .bind(row.id)
            .bind(&stored)
            .execute(&mut *transaction)
            .await?;
    }
    if let Some(run_id) = row.run_id {
        let answer = match (kind, outcome) {
            (CommandKind::Start, AckOutcome::Accepted) => Some(HostAnswer::Accepted {
                host_run_ref: ack.host_run_ref.clone(),
            }),
            (CommandKind::Start, AckOutcome::Refused) => Some(HostAnswer::Refused {
                message: text(
                    ack.message.as_deref().or(ack.reason.as_deref()),
                    "The host refused the run.",
                ),
            }),
            (CommandKind::Cancel, AckOutcome::Done) => Some(HostAnswer::Stopped),
            (CommandKind::Inspect, AckOutcome::Done) => {
                let result = ack.result.as_ref();
                let known = result
                    .and_then(|result| result.get("known"))
                    .and_then(Value::as_bool)
                    == Some(true);
                if known {
                    start_confirmed(&mut transaction, run_id).await?;
                    Some(HostAnswer::Accepted {
                        host_run_ref: result
                            .and_then(|result| result.get("hostRunRef"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                } else {
                    crate::runs::ops::start_unknown_to_host(&mut transaction, run_id).await?;
                    None
                }
            }
            _ => None,
        };
        if outcome == AckOutcome::Uncertain {
            uncertain(
                &mut transaction,
                host,
                &row,
                &text(ack.message.as_deref(), "The host didn't confirm the start."),
            )
            .await?;
        } else if revives {
            // Marked as not started, then the host said it had started it.
            crate::runs::ops::revive_late_start(
                &mut transaction,
                run_id,
                host.id,
                ack.host_run_ref.clone(),
            )
            .await
            .map_err(app)?;
        } else if let Some(answer) = answer {
            host_answered(&mut transaction, run_id, host.id, answer)
                .await
                .map_err(app)?;
        }
    }
    let answer = reply(&mut transaction, &row, next_state.to_owned()).await?;
    transaction.commit().await?;
    Ok(answer)
}

async fn reply(
    transaction: &mut Transaction<'_, Postgres>,
    row: &CommandRow,
    state: String,
) -> Result<AckReply, AppError> {
    let run_state = match row.run_id {
        Some(run_id) => crate::runs::ops::state_of(&mut **transaction, run_id)
            .await?
            .map(|(state, _)| state),
        None => None,
    };
    Ok(AckReply {
        command_id: row.id,
        state,
        run_state,
    })
}

// ------------------------------------------------------------------ sweep

/// What the clock does: starts not acknowledged in time become uncertain,
/// commands past their life expire, and hosts silent for three report
/// intervals go offline (their runs then read `not_responding`). Returns
/// how many things changed.
pub async fn sweep(pool: &PgPool) -> Result<usize, AppError> {
    // Server-wide: every host's commands and liveness.
    crate::tenancy::system("runs.host", Box::pin(sweep_here(pool))).await
}

async fn sweep_here(pool: &PgPool) -> Result<usize, AppError> {
    let mut changed = 0;
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let overdue = sqlx::query_as::<_, CommandRow>(&format!(
        r#"
        SELECT {COMMAND_COLUMNS} FROM host_commands command
        WHERE command.kind = 'start' AND command.state IN ('pending', 'fetched')
          AND command.ack_due_at < now()
        ORDER BY command.ack_due_at
        LIMIT 200
        FOR UPDATE SKIP LOCKED
        "#
    ))
    .fetch_all(&mut *transaction)
    .await?;
    for row in &overdue {
        let host = sqlx::query_as::<_, HostRow>(&format!(
            "SELECT {HOST_COLUMNS} FROM hosts host WHERE host.id = $1"
        ))
        .bind(row.host_id)
        .fetch_one(&mut *transaction)
        .await?;
        uncertain(
            &mut transaction,
            &host,
            row,
            "The host didn't acknowledge the start in time.",
        )
        .await?;
        changed += 1;
    }
    changed += sqlx::query(
        r#"
        UPDATE host_commands SET state = 'expired', updated_at = now()
         WHERE state IN ('pending', 'fetched', 'uncertain') AND expires_at <= now()
        "#,
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected() as usize;
    changed += sqlx::query(
        r#"
        UPDATE hosts SET status = 'offline', updated_at = now()
         WHERE status = 'active' AND NOT builtin AND connection <> 'a2a'
           AND last_seen_at < now() - make_interval(secs => report_interval_seconds * 3)
        "#,
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected() as usize;
    transaction.commit().await?;
    Ok(changed)
}

// ------------------------------------------------------------------- push

/// Send a push host one command's thin event, signed (Standard Webhooks).
/// A failure is retried by the job queue; the host fetches the command
/// either way once it hears of it.
async fn push(state: &AppState, command_id: Uuid) -> Result<(), JobError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<Uuid>,
            Option<String>,
            Option<Uuid>,
            Option<Uuid>,
            String,
            String,
        ),
    >(
        r#"
        SELECT command.host_id, command.kind, command.run_id, host.push_url,
               host.push_secret_id, host.host_token_secret_id, command.state, host.status
        FROM host_commands command JOIN hosts host ON host.id = command.host_id
        WHERE command.id = $1
        "#,
    )
    .bind(command_id);
    let row = crate::db::scoped_one(&state.pool, async |connection| {
        row.fetch_optional(connection).await
    })
    .await
    .map_err(|_: sqlx::Error| JobError::retry("database"))?;
    let Some((_, kind, run_id, Some(url), Some(signing), host_token, command_state, host_status)) =
        row
    else {
        return Ok(());
    };
    if command_state != "pending" || host_status == "revoked" {
        return Ok(());
    }
    let keys =
        crate::secrets::keyring(&state.config).map_err(|_| JobError::permanent("keyring"))?;
    let secret = crate::secrets::reveal(&state.pool, keys.as_ref(), signing, "host.push.sign")
        .await
        .map_err(|_| JobError::retry("push secret"))?;
    let event = PushEvent {
        event_type: "host.command".to_owned(),
        command_id,
        kind: CommandKind::parse(&kind).unwrap_or(CommandKind::Action),
        run_id,
    };
    let body = serde_json::to_string(&event).map_err(|_| JobError::permanent("encode"))?;
    let id = format!("msg_{command_id}");
    let now = Utc::now().timestamp();
    let signature = sign_push(secret.expose(), &id, now, &body)
        .ok_or_else(|| JobError::permanent("push secret"))?;
    let (url, client) =
        crate::outbound::webhook_client(&url, &state.config.webhook_trusted_origins)
            .await
            .map_err(JobError::retry)?;
    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .header("webhook-id", &id)
        .header("webhook-timestamp", now.to_string())
        .header("webhook-signature", signature)
        .header(super::routes::PROTOCOL_HEADER, super::PROTOCOL)
        .body(body);
    if let Some(token_secret) = host_token {
        let token =
            crate::secrets::reveal(&state.pool, keys.as_ref(), token_secret, "host.push.auth")
                .await
                .map_err(|_| JobError::retry("host token"))?;
        request = request.bearer_auth(token.expose());
    }
    let response = request
        .send()
        .await
        .map_err(|_| JobError::retry("push unreachable"))?;
    if !response.status().is_success() {
        return Err(JobError::retry(format!(
            "push answered {}",
            response.status().as_u16()
        )));
    }
    // Pushed: the acknowledgement clock starts as if it was fetched.
    let started = sqlx::query(
        r#"
        UPDATE host_commands
           SET attempts = GREATEST(attempts, 1),
               ack_due_at = COALESCE(ack_due_at, now() + make_interval(secs => $2::double precision))
         WHERE id = $1 AND state = 'pending'
        "#,
    )
    .bind(command_id)
    .bind(ACK_SECONDS as f64);
    crate::db::scoped_one(&state.pool, async |connection| {
        started.execute(connection).await
    })
    .await
    .map_err(|_: sqlx::Error| JobError::retry("database"))?;
    Ok(())
}

/// A1.16: a start written for a run its host already runs (one the
/// built-in host started before its runs went through the protocol):
/// accepted at once, so fetching it gives the run a credential.
pub async fn adopted(
    transaction: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE host_commands
           SET state = 'acked', ack = '{"outcome": "accepted"}'::jsonb, acked_at = now(),
               ack_due_at = NULL, updated_at = now()
         WHERE id = $1 AND kind = 'start'
        "#,
    )
    .bind(command_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// A2.26: the start of a run, for a host that adopted it to fetch again.
pub async fn start_of(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM host_commands WHERE run_id = $1 AND kind = 'start' \
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        SWEEP_KIND,
        Duration::from_secs(60),
        |context: JobContext| async move {
            sweep(&context.state.pool)
                .await
                .map_err(|error| JobError::retry(error.log_category()))?;
            Ok(())
        },
    );
    registry.every(SWEEP_KIND, Duration::from_secs(30));
    registry.register_with_timeout(
        PUSH_KIND,
        Duration::from_secs(30),
        |context: JobContext| async move {
            let command_id = context
                .job
                .payload
                .get("commandId")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok())
                .ok_or_else(|| JobError::permanent("payload"))?;
            push(&context.state, command_id).await
        },
    );
}

#[cfg(test)]
mod tests {
    use super::reoffer_after;

    #[test]
    fn uncertain_starts_are_offered_again_less_and_less_often() {
        let schedule = (1..=7).map(reoffer_after).collect::<Vec<_>>();
        assert_eq!(schedule, [30, 120, 600, 1_800, 3_600, 3_600, 3_600]);
    }
}

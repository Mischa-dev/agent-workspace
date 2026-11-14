//! A1.3: run operations for any agent (A1 3.7). An account opens a run for
//! its own work and reports on it: state, activities, usage and results.
//! Anyone who may stop runs where it lives can ask it to stop.
//!
//! Every write takes the caller's transaction, so the run, its activities
//! and its event commit together. Callers check the caller's permission in
//! the run's space (`runs.report`, `runs.interrupt`) before calling; this
//! module checks that a reporter writes only its own runs and still sees
//! where they live.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    activities::{self, CleanActivity, NewActivity},
    state::{ActivityEffect, RunEvent, RunState, reduce},
    store::{RUN_COLUMNS, RunRow},
};
use crate::{
    error::AppError,
    outbox::{self, OutboxEventPayload, RunChangedPayload},
    requests,
};

/// What went wrong, in words the caller can act on.
#[derive(Debug)]
pub enum RunOpError {
    Refused {
        field: &'static str,
        message: String,
    },
    /// Missing, or not the caller's to see: the same answer for both.
    NotFound,
    Conflict(String),
    VersionConflict {
        current_version: i64,
    },
    App(AppError),
}

impl From<AppError> for RunOpError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for RunOpError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

fn refused(field: &'static str, message: impl Into<String>) -> RunOpError {
    RunOpError::Refused {
        field,
        message: message.into(),
    }
}

/// Who reports on a run.
#[derive(Clone, Copy, Debug)]
pub enum Reporter {
    /// An account reporting its own work: an agent, or a person running a
    /// script. The same rules for both.
    Account { account_id: Uuid },
}

impl Reporter {
    pub const fn account_id(self) -> Uuid {
        match self {
            Self::Account { account_id } => account_id,
        }
    }
}

/// The most runs one account may have open at once.
pub const MAX_OPEN_PER_ACCOUNT: i64 = 50;
/// The most results in one call.
pub const MAX_RESULTS: usize = 20;
const MAX_TITLE_CHARS: usize = 200;
const MAX_LINE_CHARS: usize = 200;
const MAX_TEXT_CHARS: usize = 2_000;
const MAX_SETTINGS_BYTES: usize = 16_384;

/// Why a run exists.
#[derive(Clone, Debug)]
pub struct Cause {
    /// task, thread, message, schedule, host_trigger, automation, request or
    /// manual.
    pub kind: String,
    pub conversation_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
    pub message_id: Option<Uuid>,
    /// A schedule's or trigger's name.
    pub reference: Option<String>,
}

const CAUSE_KINDS: [&str; 8] = [
    "task",
    "thread",
    "message",
    "schedule",
    "host_trigger",
    "automation",
    "request",
    "manual",
];

/// A run to open.
#[derive(Clone, Debug)]
pub struct RunOpen {
    pub space_id: Uuid,
    pub client_run_id: Option<String>,
    pub title: String,
    pub cause: Cause,
    pub parent_run_id: Option<Uuid>,
    pub report_interval_seconds: Option<i32>,
    pub settings: Option<Value>,
    /// `working` (the default) or `accepted`.
    pub initial: RunState,
    pub requested_by: Option<Uuid>,
    pub via_app_id: Option<Uuid>,
    pub launch_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub struct Opened {
    pub run: RunRow,
    pub created: bool,
}

/// A change `run_update` asks for. `None` leaves a field as it is; an empty
/// string clears an optional one.
#[derive(Clone, Debug, Default)]
pub struct RunUpdate {
    pub state: Option<RunState>,
    pub status_line: Option<String>,
    pub title: Option<String>,
    pub external_url: Option<String>,
    pub backend: Option<String>,
    pub failure_reason: Option<String>,
    pub outcome_summary: Option<String>,
    pub expected_version: Option<i64>,
}

/// A request an `interrupt` activity opened.
#[derive(Clone, Debug)]
pub struct RaisedRequest {
    pub activity_id: String,
    pub request_id: Uuid,
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Appended {
    pub accepted: usize,
    pub duplicates: usize,
    /// Refused because the run reached its activity limit.
    pub over_limit: usize,
    /// Requests the batch's interrupts opened (or had opened before).
    pub requests: Vec<RaisedRequest>,
    pub run: RunRow,
}

/// One line of text: control characters become spaces, runs of spaces one.
pub fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn line(value: &str, max: usize, field: &'static str, message: &str) -> Result<String, RunOpError> {
    let value = one_line(value);
    if value.chars().count() > max {
        return Err(refused(field, message));
    }
    Ok(value)
}

fn https_url(value: &str, field: &'static str) -> Result<String, RunOpError> {
    let value = value.trim();
    let valid = value.len() <= 2_000
        && value.starts_with("https://")
        && value.len() > "https://".len()
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        && url::Url::parse(value).is_ok_and(|url| url.scheme() == "https" && url.host().is_some());
    if valid {
        Ok(value.to_owned())
    } else {
        Err(refused(field, "Give an https:// address."))
    }
}

/// The run's change as an event (the outbox until K3's log): realtime
/// clients refresh it, and it is what A2 delivers to agents.
pub async fn emit(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    change: &str,
) -> Result<(), AppError> {
    let Some(space_id) = run.space_id else {
        return Ok(());
    };
    outbox::enqueue_typed(
        transaction,
        space_id,
        OutboxEventPayload::RunChanged(RunChangedPayload {
            run_id: run.id,
            conversation_id: run.conversation_id,
            change: change.to_owned(),
            state: run.state.clone(),
            version: run.version,
        }),
    )
    .await?;
    Ok(())
}

async fn load(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<RunRow>, AppError> {
    Ok(sqlx::query_as::<_, RunRow>(&format!(
        "SELECT {RUN_COLUMNS} FROM runs run WHERE run.id = $1 FOR UPDATE"
    ))
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// A run the reporter may write: its own, with no built-in detail row (the
/// built-in host reports through `agent_runs` until A1.14), where it still
/// sees the conversation and belongs to the space. Locked for the change.
async fn load_for_report(
    transaction: &mut Transaction<'_, Postgres>,
    reporter: Reporter,
    run_id: Uuid,
) -> Result<RunRow, RunOpError> {
    let run = load(transaction, run_id)
        .await?
        .ok_or(RunOpError::NotFound)?;
    if run.account_id != reporter.account_id() {
        return Err(RunOpError::NotFound);
    }
    let reachable = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT NOT EXISTS (SELECT 1 FROM agent_runs WHERE id = $1)
           AND ($3::uuid IS NULL OR EXISTS (
                   SELECT 1 FROM project_memberships membership
                   JOIN projects project ON project.id = membership.project_id
                   WHERE membership.project_id = $3 AND membership.actor_id = $2
                     AND membership.enabled AND project.archived_at IS NULL))
           AND ($4::uuid IS NULL OR conversation_visible_to($4, $2))
        "#,
    )
    .bind(run.id)
    .bind(reporter.account_id())
    .bind(run.space_id)
    .bind(run.conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !reachable {
        return Err(RunOpError::NotFound);
    }
    Ok(run)
}

/// Open a run for the reporter's own work, or return the one it already
/// opened under the same `clientRunId`.
pub async fn open(
    transaction: &mut Transaction<'_, Postgres>,
    reporter: Reporter,
    input: RunOpen,
) -> Result<Opened, RunOpError> {
    let account_id = reporter.account_id();
    let client_run_id = match input.client_run_id.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(value) if value.chars().count() > 200 || value.chars().any(char::is_control) => {
            return Err(refused(
                "clientRunId",
                "Keep clientRunId to 200 characters on one line.",
            ));
        }
        Some(value) => Some(value.to_owned()),
    };
    if let Some(client_run_id) = &client_run_id
        && let Some(run) = existing_by_client_id(transaction, account_id, client_run_id).await?
    {
        return replayed(transaction, account_id, input.space_id, run).await;
    }
    let title = line(
        &input.title,
        MAX_TITLE_CHARS,
        "title",
        "Keep the title to 200 characters.",
    )?;
    if title.is_empty() {
        return Err(refused("title", "Give the run a title: what it is doing."));
    }
    let cause = &input.cause;
    let Some(cause_kind) = CAUSE_KINDS.into_iter().find(|kind| *kind == cause.kind) else {
        return Err(refused(
            "cause.kind",
            format!("Use one of: {}.", CAUSE_KINDS.join(", ")),
        ));
    };
    let cause_ref = match cause.reference.as_deref().map(one_line) {
        Some(value) if value.chars().count() > 200 => {
            return Err(refused("cause.ref", "Keep cause.ref to 200 characters."));
        }
        Some(value) if value.is_empty() => None,
        other => other,
    };
    let interval = input.report_interval_seconds.unwrap_or(900);
    if !(30..=86_400).contains(&interval) {
        return Err(refused(
            "reportIntervalSeconds",
            "Report at least every 30 seconds to every 86,400 (a day).",
        ));
    }
    let settings = input
        .settings
        .clone()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if !settings.is_object()
        || serde_json::to_vec(&settings).map_or(usize::MAX, |bytes| bytes.len())
            > MAX_SETTINGS_BYTES
    {
        return Err(refused(
            "settings",
            "settings is a JSON object of at most 16 KiB.",
        ));
    }
    let settings = activities::redact_value(settings);
    if !matches!(input.initial, RunState::Working | RunState::Accepted) {
        return Err(refused("state", "A run opens working or accepted."));
    }

    // Where it lives: the conversation, task and message must be in the
    // space and visible to the reporter.
    let mut conversation_id = cause.conversation_id;
    if let Some(message_id) = cause.message_id {
        let found = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT message.conversation_id FROM messages message
            WHERE message.id = $1 AND message.project_id = $2
              AND conversation_visible_to(message.conversation_id, $3)
            "#,
        )
        .bind(message_id)
        .bind(input.space_id)
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| {
            refused(
                "cause.messageId",
                "That message wasn't found, or you can't see it.",
            )
        })?;
        if conversation_id.is_some_and(|id| id != found) {
            return Err(refused(
                "cause.messageId",
                "That message is in another conversation than the one you named.",
            ));
        }
        conversation_id = Some(found);
    }
    if let Some(conversation) = conversation_id {
        let visible = sqlx::query_scalar::<_, bool>(
            "SELECT conversation_visible_to(id, $3) FROM conversations WHERE id = $1 AND project_id = $2",
        )
        .bind(conversation)
        .bind(input.space_id)
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await?
        .unwrap_or(false);
        if !visible {
            return Err(refused(
                "cause.conversationId",
                "That conversation wasn't found in this project, or you can't see it.",
            ));
        }
    }
    if let Some(work_item) = cause.work_item_id {
        let found = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_items WHERE id = $1 AND project_id = $2)",
        )
        .bind(work_item)
        .bind(input.space_id)
        .fetch_one(&mut **transaction)
        .await?;
        if !found {
            return Err(refused(
                "cause.taskId",
                "That task wasn't found in this project.",
            ));
        }
    }
    match cause_kind {
        "task" if cause.work_item_id.is_none() => {
            return Err(refused("cause.taskId", "A task cause names the task."));
        }
        "thread" if conversation_id.is_none() => {
            return Err(refused(
                "cause.conversationId",
                "A thread cause names the conversation.",
            ));
        }
        "message" if cause.message_id.is_none() => {
            return Err(refused(
                "cause.messageId",
                "A message cause names the message.",
            ));
        }
        "schedule" | "host_trigger" if cause_ref.is_none() => {
            return Err(refused(
                "cause.ref",
                "Name the schedule or trigger in cause.ref.",
            ));
        }
        _ => {}
    }
    if let Some(parent) = input.parent_run_id {
        let found = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM runs WHERE id = $1 AND account_id = $2 AND space_id = $3)",
        )
        .bind(parent)
        .bind(account_id)
        .bind(input.space_id)
        .fetch_one(&mut **transaction)
        .await?;
        if !found {
            return Err(refused(
                "parentRunId",
                "A child run hangs off one of your own runs in the same project.",
            ));
        }
    }

    // One account can't pile up open runs: counted under a lock so two
    // calls at once count each other.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('runs_open:' || $1::text, 0))")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    if let Some(client_run_id) = &client_run_id
        && let Some(run) = existing_by_client_id(transaction, account_id, client_run_id).await?
    {
        return replayed(transaction, account_id, input.space_id, run).await;
    }
    let open_runs = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM runs WHERE account_id = $1 AND state = ANY($2)",
    )
    .bind(account_id)
    .bind(RunState::active_keys())
    .fetch_one(&mut **transaction)
    .await?;
    if open_runs >= MAX_OPEN_PER_ACCOUNT {
        return Err(RunOpError::Conflict(format!(
            "You already have {open_runs} runs that haven't finished. Finish some (run_update or a run_finished activity) before opening more."
        )));
    }

    let run_id = Uuid::now_v7();
    let working = input.initial == RunState::Working;
    let run = sqlx::query_as::<_, RunRow>(&format!(
        r#"
        INSERT INTO runs AS run (
            id, workspace_id, space_id, account_id, title, cause_kind, cause_ref,
            conversation_id, work_item_id, cause_message_id, requested_by_account_id,
            via_app_id, parent_run_id, launch_id, state, client_run_id,
            report_interval_seconds, last_report_at, activity_seq, settings,
            accepted_at, started_at
        )
        SELECT $1, project.organization_id, project.id, $3, $4, $5, $6, $7, $8, $9, $10,
               $11, $12, $13, $14, $15, $16, now(), CASE WHEN $18 THEN 1 ELSE 0 END, $17,
               now(), CASE WHEN $18 THEN now() END
        FROM projects project
        WHERE project.id = $2
        RETURNING {RUN_COLUMNS}
        "#
    ))
    .bind(run_id)
    .bind(input.space_id)
    .bind(account_id)
    .bind(&title)
    .bind(cause_kind)
    .bind(&cause_ref)
    .bind(conversation_id)
    .bind(cause.work_item_id)
    .bind(cause.message_id)
    .bind(input.requested_by)
    .bind(input.via_app_id)
    .bind(input.parent_run_id)
    .bind(input.launch_id)
    .bind(input.initial.as_str())
    .bind(&client_run_id)
    .bind(interval)
    .bind(&settings)
    .bind(working)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RunOpError::NotFound)?;
    if working {
        insert_activity_row(
            transaction,
            &run,
            1,
            &CleanActivity {
                client_id: "run_open".to_owned(),
                kind: "run_started",
                occurred_at: run.created_at,
                step_ref: None,
                parent_ref: None,
                summary: "Started.".to_owned(),
                payload: serde_json::json!({}),
                effect: ActivityEffect::Report,
                always_kept: true,
            },
            account_id,
        )
        .await?;
    }
    emit(transaction, &run, "opened").await?;
    Ok(Opened { run, created: true })
}

/// A retried `run_open` answers with the run it opened, but only where the
/// caller could open it now: in the same project, which it still belongs
/// to, and in a conversation it still sees. A `clientRunId` already used in
/// another project is refused without saying anything about that run.
async fn replayed(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    space_id: Uuid,
    run: RunRow,
) -> Result<Opened, RunOpError> {
    if run.space_id != Some(space_id) {
        return Err(RunOpError::Conflict(
            "That clientRunId already names another of your runs. Use a new one.".to_owned(),
        ));
    }
    let reachable = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
                   SELECT 1 FROM project_memberships membership
                   JOIN projects project ON project.id = membership.project_id
                   WHERE membership.project_id = $2 AND membership.actor_id = $1
                     AND membership.enabled AND project.archived_at IS NULL)
           AND ($3::uuid IS NULL OR conversation_visible_to($3, $1))
        "#,
    )
    .bind(account_id)
    .bind(space_id)
    .bind(run.conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !reachable {
        return Err(RunOpError::NotFound);
    }
    Ok(Opened {
        run,
        created: false,
    })
}

async fn existing_by_client_id(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    client_run_id: &str,
) -> Result<Option<RunRow>, AppError> {
    Ok(sqlx::query_as::<_, RunRow>(&format!(
        "SELECT {RUN_COLUMNS} FROM runs run WHERE run.account_id = $1 AND run.client_run_id = $2"
    ))
    .bind(account_id)
    .bind(client_run_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Write the run's new state and report time after a change, bumping its
/// version.
async fn save_state(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    next: RunState,
    reported: bool,
) -> Result<RunRow, AppError> {
    if next.is_terminal() && run.run_state().is_ok_and(|current| !current.is_terminal()) {
        // What a finished run still asked for is withdrawn.
        requests::withdraw_for_run(transaction, run.id).await?;
    }
    Ok(sqlx::query_as::<_, RunRow>(&format!(
        r#"
        UPDATE runs run
           SET state = $2,
               last_report_at = CASE WHEN $3 THEN now() ELSE run.last_report_at END,
               liveness = CASE WHEN $3 THEN 'ok' ELSE run.liveness END,
               started_at = CASE WHEN $2 = 'working' THEN COALESCE(run.started_at, now())
                                 ELSE run.started_at END,
               finished_at = CASE WHEN $4 THEN COALESCE(run.finished_at, now()) ELSE NULL END,
               version = run.version + 1,
               updated_at = now()
         WHERE run.id = $1
        RETURNING {RUN_COLUMNS}
        "#
    ))
    .bind(run.id)
    .bind(next.as_str())
    .bind(reported)
    .bind(next.is_terminal())
    .fetch_one(&mut **transaction)
    .await?)
}

/// Change a run the reporter owns: an explicit state along the allowed
/// edges, and its status line, title, link, backend or outcome text.
pub async fn update(
    transaction: &mut Transaction<'_, Postgres>,
    reporter: Reporter,
    run_id: Uuid,
    change: RunUpdate,
) -> Result<RunRow, RunOpError> {
    let run = load_for_report(transaction, reporter, run_id).await?;
    if let Some(expected) = change.expected_version
        && expected != run.version
    {
        return Err(RunOpError::VersionConflict {
            current_version: run.version,
        });
    }
    let current = run.run_state()?;
    let next = match change.state {
        Some(next) => reduce(current, RunEvent::Update(next)).map_err(|refused_move| {
            refused(
                "state",
                format!(
                    "A run that is {} can't move to {}.",
                    refused_move.from, refused_move.to
                ),
            )
        })?,
        None => current,
    };
    let optional = |value: Option<String>,
                    max: usize,
                    field: &'static str|
     -> Result<Option<Option<String>>, RunOpError> {
        value
            .map(|value| {
                let value = crate::run_log::redact(&value);
                line(
                    &value,
                    max,
                    field,
                    &format!("Keep {field} to {max} characters."),
                )
                .map(|value| (!value.is_empty()).then_some(value))
            })
            .transpose()
    };
    let status_line = optional(change.status_line, MAX_LINE_CHARS, "statusLine")?;
    let failure_reason = optional(change.failure_reason, MAX_TEXT_CHARS, "failureReason")?;
    let outcome_summary = optional(change.outcome_summary, MAX_TEXT_CHARS, "outcomeSummary")?;
    let backend = optional(change.backend, 80, "backend")?;
    let title = match change.title {
        Some(title) => {
            let title = line(
                &title,
                MAX_TITLE_CHARS,
                "title",
                "Keep the title to 200 characters.",
            )?;
            if title.is_empty() {
                return Err(refused("title", "A run's title can't be empty."));
            }
            Some(title)
        }
        None => None,
    };
    let external_url = match change.external_url {
        Some(url) if url.trim().is_empty() => Some(None),
        Some(url) => Some(Some(https_url(&url, "externalUrl")?)),
        None => None,
    };
    let saved = save_state(transaction, &run, next, true).await?;
    let saved = sqlx::query_as::<_, RunRow>(&format!(
        r#"
        UPDATE runs run
           SET status_line = CASE WHEN $2 THEN $3 ELSE run.status_line END,
               failure_reason = CASE WHEN $4 THEN $5 ELSE run.failure_reason END,
               outcome_summary = CASE WHEN $6 THEN $7 ELSE run.outcome_summary END,
               backend = CASE WHEN $8 THEN $9 ELSE run.backend END,
               title = COALESCE($10, run.title),
               external_url = CASE WHEN $11 THEN $12 ELSE run.external_url END
         WHERE run.id = $1
        RETURNING {RUN_COLUMNS}
        "#
    ))
    .bind(saved.id)
    .bind(status_line.is_some())
    .bind(status_line.flatten())
    .bind(failure_reason.is_some())
    .bind(failure_reason.flatten())
    .bind(outcome_summary.is_some())
    .bind(outcome_summary.flatten())
    .bind(backend.is_some())
    .bind(backend.flatten())
    .bind(title)
    .bind(external_url.is_some())
    .bind(external_url.flatten())
    .fetch_one(&mut **transaction)
    .await?;
    let change = if next.is_terminal() && !current.is_terminal() {
        "finished"
    } else {
        "updated"
    };
    emit(transaction, &saved, change).await?;
    Ok(saved)
}

async fn insert_activity_row(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    seq: i64,
    activity: &CleanActivity,
    reporter: Uuid,
) -> Result<bool, AppError> {
    let inserted = sqlx::query(
        r#"
        INSERT INTO run_activities (
            id, run_id, workspace_id, seq, client_id, kind, step_ref, parent_ref,
            summary, payload, occurred_at, reported_by_account_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        ON CONFLICT (run_id, client_id) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(run.id)
    .bind(run.workspace_id)
    .bind(seq)
    .bind(&activity.client_id)
    .bind(activity.kind)
    .bind(&activity.step_ref)
    .bind(&activity.parent_ref)
    .bind(&activity.summary)
    .bind(&activity.payload)
    .bind(activity.occurred_at)
    .bind(reporter)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(inserted == 1)
}

/// Append a batch of activities to a run the reporter owns. Each is
/// deduplicated by its id; the state follows what was stored.
pub async fn append(
    transaction: &mut Transaction<'_, Postgres>,
    reporter: Reporter,
    run_id: Uuid,
    batch: Vec<NewActivity>,
) -> Result<Appended, RunOpError> {
    if batch.is_empty() {
        return Err(refused("activities", "Send at least one activity."));
    }
    if batch.len() > activities::MAX_BATCH {
        return Err(refused(
            "activities",
            "Send at most 100 activities at a time.",
        ));
    }
    let run = load_for_report(transaction, reporter, run_id).await?;
    let mut state = run.run_state()?;
    let mut open = requests::open_count(transaction, run.id).await?;
    let mut seq = run.activity_seq;
    let mut accepted = 0;
    let mut duplicates = 0;
    let mut over_limit = 0;
    let mut gaps = 0;
    let mut finished_now = false;
    let mut raised = Vec::new();
    for activity in batch {
        if activity.kind == "interrupt_resolved"
            && let Some(reference) = activity.payload.get("requestId").and_then(Value::as_str)
        {
            requests::resolved_by_run(transaction, &run, reference).await?;
            open = requests::open_count(transaction, run.id).await?;
        }
        let activity = activities::clean(activity, open)?;
        if seq >= activities::MAX_PER_RUN && !activity.always_kept {
            over_limit += 1;
            continue;
        }
        let inserted =
            insert_activity_row(transaction, &run, seq + 1, &activity, reporter.account_id())
                .await?;
        if activity.effect == ActivityEffect::Interrupt
            && let Some(request) = activity
                .payload
                .get("request")
                .filter(|request| request.is_object())
        {
            // A repeated interrupt finds the request it opened the first time.
            let opened = requests::open_for_run(transaction, &run, &activity.client_id, request)
                .await
                .map_err(|error| match error {
                    requests::OpenError::Refused { field, message } => {
                        RunOpError::Refused { field, message }
                    }
                    requests::OpenError::App(error) => RunOpError::App(error),
                })?;
            raised.push(RaisedRequest {
                activity_id: activity.client_id.clone(),
                request_id: opened.request_id,
                client_request_id: opened.client_request_id,
            });
            open = requests::open_count(transaction, run.id).await?;
        }
        if !inserted {
            duplicates += 1;
            continue;
        }
        seq += 1;
        accepted += 1;
        if activity.kind == "gap" {
            gaps += 1;
        }
        let before = state;
        state = reduce(state, RunEvent::Activity(activity.effect)).unwrap_or(state);
        finished_now |= state.is_terminal() && !before.is_terminal();
    }
    if over_limit > 0 {
        let gap = activities::clean(
            NewActivity {
                client_id: format!("limit:{}", seq + 1),
                kind: "gap".to_owned(),
                occurred_at: Utc::now(),
                step_ref: None,
                parent_ref: None,
                summary: Some(format!(
                    "{over_limit} steps weren't kept: the run reached its limit of 20,000 activities."
                )),
                payload: serde_json::json!({"reason": "limit_reached", "missed": over_limit}),
            },
            open,
        )?;
        if insert_activity_row(transaction, &run, seq + 1, &gap, reporter.account_id()).await? {
            seq += 1;
            gaps += 1;
        }
    }
    let saved = save_state(transaction, &run, state, true).await?;
    let saved = sqlx::query_as::<_, RunRow>(&format!(
        r#"
        UPDATE runs run SET activity_seq = $2, gap_count = run.gap_count + $3
        WHERE run.id = $1
        RETURNING {RUN_COLUMNS}
        "#
    ))
    .bind(saved.id)
    .bind(seq)
    .bind(gaps)
    .fetch_one(&mut **transaction)
    .await?;
    if accepted > 0 {
        emit(
            transaction,
            &saved,
            if finished_now { "finished" } else { "activity" },
        )
        .await?;
    }
    Ok(Appended {
        accepted,
        duplicates,
        over_limit,
        requests: raised,
        run: saved,
    })
}

/// What a run produced, as a reporter says it.
#[derive(Clone, Debug)]
pub struct RunResult {
    /// pull_request, branch, diff, thing, file or link.
    pub kind: String,
    pub url: Option<String>,
    pub id: Option<String>,
    pub title: Option<String>,
    pub branch: Option<String>,
    pub number: Option<i64>,
    pub thing_kind: Option<String>,
}

const RESULT_KINDS: [&str; 6] = ["pull_request", "branch", "diff", "thing", "file", "link"];

/// Add results to a run the reporter owns, each once (by kind and address).
pub async fn add_results(
    transaction: &mut Transaction<'_, Postgres>,
    reporter: Reporter,
    run_id: Uuid,
    results: Vec<RunResult>,
) -> Result<(RunRow, usize), RunOpError> {
    if results.is_empty() || results.len() > MAX_RESULTS {
        return Err(refused("results", "Send 1 to 20 results at a time."));
    }
    let mut batch = Vec::with_capacity(results.len());
    for result in results {
        let Some(kind) = RESULT_KINDS.into_iter().find(|kind| *kind == result.kind) else {
            return Err(refused(
                "results.kind",
                format!("Use one of: {}.", RESULT_KINDS.join(", ")),
            ));
        };
        let url = result
            .url
            .as_deref()
            .map(|url| https_url(url, "results.url"))
            .transpose()?;
        let id = result
            .id
            .as_deref()
            .map(|id| {
                line(
                    id,
                    200,
                    "results.id",
                    "Keep a result's id to 200 characters.",
                )
            })
            .transpose()?
            .filter(|id| !id.is_empty());
        let branch = result
            .branch
            .as_deref()
            .map(|branch| {
                line(
                    branch,
                    200,
                    "results.branch",
                    "Keep a branch name to 200 characters.",
                )
            })
            .transpose()?
            .filter(|branch| !branch.is_empty());
        let key = match kind {
            "pull_request" | "link" => url.clone(),
            "branch" => branch.clone(),
            _ => id.clone(),
        }
        .ok_or_else(|| {
            refused(
                "results",
                match kind {
                    "pull_request" | "link" => "A pull request or link result needs its url.",
                    "branch" => "A branch result needs its branch name.",
                    _ => "A diff, thing or file result needs its id.",
                },
            )
        })?;
        let mut payload = serde_json::Map::new();
        payload.insert("kind".to_owned(), Value::String(kind.to_owned()));
        for (name, value) in [
            ("url", url),
            ("id", id),
            ("branch", branch),
            ("title", result.title.as_deref().map(one_line)),
            ("thingKind", result.thing_kind.as_deref().map(one_line)),
        ] {
            if let Some(value) = value {
                payload.insert(name.to_owned(), Value::String(value));
            }
        }
        if let Some(number) = result.number.filter(|number| *number > 0) {
            payload.insert("number".to_owned(), Value::from(number));
        }
        let key_hash = {
            use sha2::{Digest, Sha256};
            hex::encode(&Sha256::digest(key.as_bytes())[..12])
        };
        batch.push(NewActivity {
            client_id: format!("result:{kind}:{key_hash}"),
            kind: "result".to_owned(),
            occurred_at: Utc::now(),
            step_ref: None,
            parent_ref: None,
            summary: Some(format!(
                "{}: {}",
                kind.replace('_', " "),
                result.title.as_deref().map(one_line).unwrap_or(key)
            )),
            payload: Value::Object(payload),
        });
    }
    let appended = append(transaction, reporter, run_id, batch).await?;
    if appended.accepted > 0 {
        emit(transaction, &appended.run, "result_added").await?;
    }
    Ok((appended.run, appended.accepted))
}

/// Usage as a reporter says it: for one of its runs, or for a time window.
#[derive(Clone, Debug)]
pub struct UsageInput {
    pub report_key: String,
    pub run_id: Option<Uuid>,
    pub window: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// The space a window report is for (the call's project).
    pub space_id: Uuid,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cost: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UsageRecorded {
    pub report_id: Uuid,
    pub duplicate: bool,
    pub run_id: Option<Uuid>,
}

/// Record usage once per `(reporter, reportKey)`. Usage for a run adds to
/// the run's totals as exact; a window report is kept as it came, for the
/// window split (A1.15).
pub async fn record_usage(
    transaction: &mut Transaction<'_, Postgres>,
    reporter: Reporter,
    input: UsageInput,
) -> Result<UsageRecorded, RunOpError> {
    let account_id = reporter.account_id();
    let report_key = line(
        &input.report_key,
        200,
        "reportKey",
        "Keep reportKey to 200 characters.",
    )?;
    if report_key.is_empty() {
        return Err(refused(
            "reportKey",
            "Give each report a reportKey, the same on a retry.",
        ));
    }
    let model = line(
        &input.model,
        200,
        "model",
        "Keep the model name to 200 characters.",
    )?;
    if model.is_empty() {
        return Err(refused("model", "Name the model the usage was on."));
    }
    if input.input_tokens < 0 || input.output_tokens < 0 || input.cached_input_tokens < 0 {
        return Err(refused("inputTokens", "Token counts can't be negative."));
    }
    if input
        .cost
        .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
    {
        return Err(refused("cost", "Cost is a number of at least 0."));
    }
    let currency = match input.currency.as_deref().map(str::trim) {
        Some(code)
            if code.len() == 3 && code.chars().all(|character| character.is_ascii_uppercase()) =>
        {
            Some(code.to_owned())
        }
        Some(_) => {
            return Err(refused(
                "currency",
                "Give the currency as a code, like USD.",
            ));
        }
        None => input.cost.map(|_| "USD".to_owned()),
    };
    if let Some(existing) = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        "SELECT id, run_id FROM usage_reports WHERE reported_by_account_id = $1 AND report_key = $2",
    )
    .bind(account_id)
    .bind(&report_key)
    .fetch_optional(&mut **transaction)
    .await?
    {
        return Ok(UsageRecorded {
            report_id: existing.0,
            duplicate: true,
            run_id: existing.1,
        });
    }
    let (run, workspace_id) = match (input.run_id, input.window) {
        (Some(run_id), None) => {
            let run = load_for_report(transaction, reporter, run_id).await?;
            let workspace = run.workspace_id;
            (Some(run), workspace)
        }
        (None, Some((start, end))) => {
            if end <= start {
                return Err(refused("windowEnd", "windowEnd comes after windowStart."));
            }
            let workspace =
                sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                    .bind(input.space_id)
                    .fetch_optional(&mut **transaction)
                    .await?
                    .ok_or(RunOpError::NotFound)?;
            (None, workspace)
        }
        _ => {
            return Err(refused(
                "runId",
                "Report usage for one run (runId) or for a window (windowStart and windowEnd), not both.",
            ));
        }
    };
    let report_id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"
        INSERT INTO usage_reports (
            id, workspace_id, reported_by_account_id, account_id, run_id, report_key, model,
            window_start, window_end, input_tokens, output_tokens, cached_input_tokens,
            reported_cost, reported_currency
        )
        VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        ON CONFLICT (reported_by_account_id, report_key) DO NOTHING
        "#,
    )
    .bind(report_id)
    .bind(workspace_id)
    .bind(account_id)
    .bind(run.as_ref().map(|run| run.id))
    .bind(&report_key)
    .bind(&model)
    .bind(input.window.map(|window| window.0))
    .bind(input.window.map(|window| window.1))
    .bind(input.input_tokens)
    .bind(input.output_tokens)
    .bind(input.cached_input_tokens)
    .bind(input.cost)
    .bind(&currency)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(RunOpError::Conflict(
            "That report is still being recorded. Try again in a moment.".to_owned(),
        ));
    }
    if let Some(run) = run {
        super::usage::add_reported(transaction, &run, &model, &input, currency.as_deref()).await?;
        sqlx::query("UPDATE usage_reports SET attributed_at = now() WHERE id = $1")
            .bind(report_id)
            .execute(&mut **transaction)
            .await?;
        let touched = sqlx::query_as::<_, RunRow>(&format!(
            r#"
            UPDATE runs run SET last_report_at = now(), liveness = 'ok',
                   version = run.version + 1, updated_at = now()
            WHERE run.id = $1
            RETURNING {RUN_COLUMNS}
            "#
        ))
        .bind(run.id)
        .fetch_one(&mut **transaction)
        .await?;
        emit(transaction, &touched, "usage_updated").await?;
        return Ok(UsageRecorded {
            report_id,
            duplicate: false,
            run_id: Some(run.id),
        });
    }
    Ok(UsageRecorded {
        report_id,
        duplicate: false,
        run_id: None,
    })
}

/// Whether a run has a built-in detail row (the built-in host drives it).
pub async fn is_builtin(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM agent_runs WHERE id = $1)")
            .bind(run_id)
            .fetch_one(&mut **transaction)
            .await?,
    )
}

/// Ask a run that reports for itself to stop. The caller has checked that
/// `by` may stop runs where it lives and can see it. Until it confirms
/// (`run_finished` or `run_update`), the run is `cancel_requested`; a run
/// still queued is cancelled at once. The agent hears `run.cancel`.
pub async fn request_cancel(
    transaction: &mut Transaction<'_, Postgres>,
    by: Uuid,
    run_id: Uuid,
    reason: Option<String>,
) -> Result<RunRow, RunOpError> {
    let run = load(transaction, run_id)
        .await?
        .ok_or(RunOpError::NotFound)?;
    let current = run.run_state()?;
    if current.is_terminal() || current == RunState::CancelRequested {
        return Ok(run);
    }
    let reason = reason
        .as_deref()
        .map(|reason| {
            line(
                reason,
                MAX_LINE_CHARS,
                "reason",
                "Keep the reason to 200 characters.",
            )
        })
        .transpose()?
        .filter(|reason| !reason.is_empty());
    let next = reduce(current, RunEvent::CancelRequested { has_runner: true })
        .map_err(|_| RunOpError::Conflict("That run can't be stopped now.".to_owned()))?;
    let saved = save_state(transaction, &run, next, false).await?;
    let saved = sqlx::query_as::<_, RunRow>(&format!(
        r#"
        UPDATE runs run
           SET cancel_requested_at = now(), cancel_requested_by_account_id = $2,
               status_line = COALESCE($3, run.status_line)
         WHERE run.id = $1
        RETURNING {RUN_COLUMNS}
        "#
    ))
    .bind(saved.id)
    .bind(by)
    // The status line holds 200 characters; the prefix comes out of them.
    .bind(reason.map(|reason| {
        format!("Stop asked: {reason}")
            .chars()
            .take(MAX_LINE_CHARS)
            .collect::<String>()
    }))
    .fetch_one(&mut **transaction)
    .await?;
    emit(
        transaction,
        &saved,
        if next.is_terminal() {
            "finished"
        } else {
            "cancel"
        },
    )
    .await?;
    if !next.is_terminal() {
        notify_agent(
            transaction,
            &saved,
            "run.cancel",
            "You were asked to stop this run.",
        )
        .await?;
    }
    Ok(saved)
}

/// Tell the agent that does a run something about it, on each of its active
/// connections in the run's project (the agent event feed until A2's
/// delivery).
pub async fn notify_agent(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    kind: &str,
    summary: &str,
) -> Result<(), AppError> {
    let Some(space_id) = run.space_id else {
        return Ok(());
    };
    let connections = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT connection.id FROM external_agent_connections connection
        JOIN project_memberships membership
          ON membership.project_id = connection.project_id
         AND membership.actor_id = connection.actor_id
         AND membership.enabled
        WHERE connection.project_id = $1 AND connection.actor_id = $2
          AND connection.status = 'active'
        ORDER BY connection.id
        "#,
    )
    .bind(space_id)
    .bind(run.account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for connection in connections {
        crate::external_events::enqueue_external_event(
            transaction,
            connection,
            space_id,
            kind,
            summary,
            "run",
            run.id,
            &format!("/runs/{}", run.id),
        )
        .await?;
    }
    Ok(())
}

/// Mark a run whose start is uncertain as never started.
pub async fn mark_not_started(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<RunRow, RunOpError> {
    let run = load(transaction, run_id)
        .await?
        .ok_or(RunOpError::NotFound)?;
    let current = run.run_state()?;
    if current != RunState::StartUncertain {
        return Err(RunOpError::Conflict(format!(
            "Only a run whose start is uncertain can be marked as not started; this one is {current}."
        )));
    }
    let saved = save_state(transaction, &run, RunState::Failed, false).await?;
    let saved = sqlx::query_as::<_, RunRow>(&format!(
        "UPDATE runs run SET failure_reason = 'Marked as not started' WHERE run.id = $1 RETURNING {RUN_COLUMNS}"
    ))
    .bind(saved.id)
    .fetch_one(&mut **transaction)
    .await?;
    emit(transaction, &saved, "finished").await?;
    Ok(saved)
}

/// A host can no longer report these runs: it was unlinked, retired the
/// agent, or the agent was unbound from it (A1.11). Every active run on the
/// host (for one agent, when named) without a built-in detail row becomes
/// `interrupted` with `reason`. Returns the runs it changed.
pub async fn host_lost(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
    account_id: Option<Uuid>,
    reason: &str,
) -> Result<Vec<Uuid>, AppError> {
    let active = RunState::ALL
        .into_iter()
        .filter(|state| state.is_active())
        .map(RunState::as_str)
        .collect::<Vec<_>>();
    let runs = sqlx::query_as::<_, RunRow>(&format!(
        r#"
        SELECT {RUN_COLUMNS} FROM runs run
        WHERE run.host_id = $1 AND ($2::uuid IS NULL OR run.account_id = $2)
          AND run.state = ANY($3)
          AND NOT EXISTS (SELECT 1 FROM agent_runs builtin WHERE builtin.id = run.id)
        ORDER BY run.id
        FOR UPDATE
        "#
    ))
    .bind(host_id)
    .bind(account_id)
    .bind(&active)
    .fetch_all(&mut **transaction)
    .await?;
    let mut changed = Vec::with_capacity(runs.len());
    for run in runs {
        let Ok(current) = run.run_state() else {
            continue;
        };
        let next = reduce(current, RunEvent::HostLost).unwrap_or(current);
        if next == current {
            continue;
        }
        let saved = save_state(transaction, &run, next, false).await?;
        let saved = sqlx::query_as::<_, RunRow>(&format!(
            r#"
            UPDATE runs run SET failure_reason = $2, status_line = $2
             WHERE run.id = $1
            RETURNING {RUN_COLUMNS}
            "#
        ))
        .bind(saved.id)
        .bind(reason)
        .fetch_one(&mut **transaction)
        .await?;
        emit(transaction, &saved, "finished").await?;
        changed.push(saved.id);
    }
    Ok(changed)
}

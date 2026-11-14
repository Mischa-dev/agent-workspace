//! A1.3: the run writes as operations (K2). An account opens a run for its
//! own work and reports on it (`run_create`, `run_update`,
//! `run_activity_report`, `run_usage_report`, `run_result_report`); anyone
//! who may stop runs where one lives can ask it to stop (`run_cancel`) or
//! settle an uncertain start (`run_mark_not_started`). The rules are
//! `runs::ops`; these check what only a call knows (whose run it is, the
//! report rate) and shape the answers. The old tool names `run_open`,
//! `run_activity` and `run_result` stay callable (`ops::aliases`).
//!
//! Retries: a run with a `clientRunId`, an activity's `id`, a usage
//! `reportKey` and a result's address are each kept once by the run
//! service, so those operations keep no stored answer (a replayed answer
//! would say "accepted" where the service says "duplicate"). `run_update`
//! and `run_cancel` keep one only for a `clientRequestId` the caller gave,
//! so the same change asked twice later still applies.

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    RunCaller, RunState,
    activities::NewActivity,
    ops::{self, Cause, Reporter, RunOpError, RunOpen, RunResult, RunUpdate, UsageInput},
    store::{self, RunRow},
};
use crate::{
    idempotency::{IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve},
    ops::{
        ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, access_shim, refs::IdRef, target::Located,
    },
};

/// The most bytes of activities in one call.
const MAX_CALL_BYTES: usize = 256 * 1024;
const RUN: &str = "That run";

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<RunCreate>()
        .add::<RunUpdateOp>()
        .add::<RunActivityReport>()
        .add::<RunUsageReport>()
        .add::<RunResultReport>()
        .add::<RunCancel>()
        .add::<RunMarkNotStarted>();
}

// ------------------------------------------------------------------ shared

/// A run as the run operations return it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run_id: String,
    pub title: String,
    /// queued, starting, start_uncertain, accepted, working, waiting,
    /// cancel_requested, done, failed, cancelled, refused or interrupted.
    pub state: String,
    /// ok, quiet (no report for one interval) or not_responding (three).
    pub liveness: String,
    /// Pass it as expectedVersion to change the run only if nobody else has.
    pub version: i64,
    /// How many activities the run has.
    pub activity_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_run_id: Option<String>,
    pub report_interval_seconds: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_requested_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

impl From<&RunRow> for RunRecord {
    fn from(run: &RunRow) -> Self {
        Self {
            run_id: run.id.to_string(),
            title: run.title.clone(),
            state: run.state.clone(),
            liveness: run.liveness.clone(),
            version: run.version,
            activity_count: run.activity_seq,
            status_line: run.status_line.clone(),
            external_url: run.external_url.clone(),
            client_run_id: run.client_run_id.clone(),
            report_interval_seconds: run.report_interval_seconds,
            cancel_requested_at: run.cancel_requested_at,
            finished_at: run.finished_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRecordValue {
    pub run: RunRecord,
    /// True when this answers a retry of a call that already went through.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

impl RunRecordValue {
    fn of(run: &RunRow) -> Self {
        Self {
            run: RunRecord::from(run),
            idempotent_replay: false,
        }
    }
}

impl From<RunOpError> for OpError {
    fn from(error: RunOpError) -> Self {
        match error {
            RunOpError::Refused { field, message } => Self::Invalid {
                field: Cow::Borrowed(field),
                message: message.into(),
            },
            RunOpError::NotFound => Self::not_found(RUN),
            RunOpError::Conflict(message) => Self::Conflict {
                reason: ConflictReason::Transition,
                message: message.into(),
                current: None,
            },
            RunOpError::VersionConflict { current_version } => Self::Conflict {
                reason: ConflictReason::Version { current_version },
                message: format!(
                    "The run changed first (now version {current_version}). Read it again, then retry with expectedVersion {current_version}."
                )
                .into(),
                current: None,
            },
            RunOpError::App(error) => error.into(),
        }
    }
}

fn reporter(call: &OpCall<'_>) -> Reporter {
    Reporter::Account {
        account_id: call.account_id(),
    }
}

/// Reports are rate limited per account: batch activities instead.
fn report_rate(call: &OpCall<'_>) -> Result<(), OpError> {
    call.state
        .rate_limits
        .check_run_report(call.account_id())
        .map_err(OpError::from)
}

/// The run is the caller's own (dispatch found it where they may see it):
/// someone else's run is not found, the same as a missing one.
async fn own_run(call: &mut OpCall<'_>, run_id: Uuid) -> Result<(), OpError> {
    let account = call.account_id();
    let space = call.ctx.space_id;
    let tx = call.tx().await?;
    match store::space_of_own(&mut **tx, account, run_id).await? {
        Some(own) if Some(own) == space => Ok(()),
        _ => Err(OpError::not_found(RUN)),
    }
}

/// The caller's own request id, when they gave one: a derived one (from
/// the same call within ten minutes) isn't kept, so asking for the same
/// change again later still applies it.
fn given_key(call: &OpCall<'_>) -> Option<IdempotencyKey> {
    call.ctx
        .request_id
        .as_deref()
        .filter(|key| !key.starts_with("d-"))
        .and_then(|key| IdempotencyKey::parse(key).ok())
}

enum Keyed {
    Replay(RunRecordValue),
    Reserved(Option<crate::idempotency::IdempotencyReservation>),
}

/// Reserve the caller's request id for one change of a run.
async fn reserve_change<I: Serialize>(
    call: &mut OpCall<'_>,
    run_id: Uuid,
    action: &str,
    input: &I,
) -> Result<Keyed, OpError> {
    let Some(key) = given_key(call) else {
        return Ok(Keyed::Reserved(None));
    };
    let account = call.account_id();
    let scope = crate::idempotency::resource_bound_scope("runs", run_id, action)?;
    let hash = RequestHash::for_value(input).map_err(|_| OpError::Unavailable)?;
    let tx = call.tx().await?;
    Ok(match reserve(tx, account, &scope, &key, hash).await? {
        IdempotencyOutcome::Replay(replay) => {
            let mut value = replay.decode::<RunRecordValue>()?;
            value.idempotent_replay = true;
            call.mark_replayed(true);
            Keyed::Replay(value)
        }
        IdempotencyOutcome::Reserved(reservation) => Keyed::Reserved(Some(reservation)),
    })
}

async fn finish_change(
    call: &mut OpCall<'_>,
    reservation: Option<crate::idempotency::IdempotencyReservation>,
    value: &RunRecordValue,
) -> Result<(), OpError> {
    if let Some(reservation) = reservation {
        let tx = call.tx().await?;
        complete(tx, &reservation, axum::http::StatusCode::OK, value).await?;
    }
    Ok(())
}

/// A task in the space, by id, link or key (such as "AWD-12").
async fn task_in_space(
    call: &mut OpCall<'_>,
    space: Uuid,
    reference: &str,
) -> Result<Uuid, OpError> {
    let tx = call.tx().await?;
    let found = match crate::ops::refs::parse_id(reference) {
        Some(id) => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM work_items WHERE id = $1 AND project_id = $2",
            )
            .bind(id)
            .bind(space)
            .fetch_optional(&mut **tx)
            .await
        }
        None => {
            let Some((key, number)) = reference
                .trim()
                .rsplit_once('-')
                .and_then(|(key, number)| Some((key, number.parse::<i64>().ok()?)))
            else {
                return Err(OpError::not_found("That task"));
            };
            sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT item.id FROM work_items item
                JOIN projects project ON project.id = item.project_id
                WHERE item.project_id = $3
                  AND upper(project.project_key) = upper($1) AND item.number = $2
                "#,
            )
            .bind(key)
            .bind(number)
            .bind(space)
            .fetch_optional(&mut **tx)
            .await
        }
    };
    found
        .map_err(|error| OpError::logged("run_create", &error.into()))?
        .ok_or(OpError::not_found("That task"))
}

fn rfc3339(value: &str, field: &'static str) -> Result<DateTime<Utc>, OpError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|at| at.with_timezone(&Utc))
        .map_err(|_| {
            OpError::invalid(
                field,
                "Give the time as RFC 3339, like 2026-11-14T09:00:00Z.",
            )
        })
}

// -------------------------------------------------------------- run_create

pub struct RunCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCauseInput {
    /// Why the run exists: task, thread, message, schedule, host_trigger,
    /// automation, request or manual.
    pub kind: String,
    /// The task it works on (key like AWD-12, id or link). Needed for task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The conversation or thread it belongs to (id or link). Needed for
    /// thread. People who can't see the conversation can't see the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<IdRef>,
    /// The message that asked for it (id or link). Needed for message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<crate::ops::refs::MessageRef>,
    /// The schedule's or trigger's name. Needed for schedule and
    /// host_trigger.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCreateInput {
    /// The space the run is in: its id, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// Your own id for this run, unique among your runs. Opening again with
    /// the same one returns the run you already opened (created: false), so
    /// a retry never makes a second run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_run_id: Option<String>,
    /// What the run is doing, in a line (at most 200 characters).
    pub title: String,
    pub cause: RunCauseInput,
    /// A run of yours in the same space this one is part of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<IdRef>,
    /// How often you promise to report, in seconds (30 to 86,400; default
    /// 900). A run silent for one interval shows as quiet, for three as not
    /// responding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_interval_seconds: Option<i32>,
    /// What the run was started with (a JSON object of at most 16 KiB).
    /// Fixed once set; secrets are removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    /// working (the default) or accepted, for a run that hasn't started
    /// work yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl Targeted for RunCreateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunCreateOutput {
    pub run: RunRecord,
    /// False when clientRunId named a run you had already opened.
    pub created: bool,
}

impl Operation for RunCreate {
    const NAME: &'static str = "run_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RunsReport);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Open a run";
    const DESCRIPTION: &'static str = "Open a run for your own work so people can follow it: what \
        you're doing, why (cause: a task, thread, message, schedule, trigger, automation, request \
        or by hand) and how often you'll report. Pass a clientRunId and a retry returns the same \
        run (created: false). Then report with run_activity_report (steps, tool calls, plan, \
        messages, run_finished), run_usage_report, run_result_report and run_update.";
    const EMITS: &'static [&'static str] = &["run.opened"];

    type Input = RunCreateInput;
    type Output = RunCreateOutput;

    async fn run(call: &mut OpCall<'_>, input: RunCreateInput) -> Result<RunCreateOutput, OpError> {
        report_rate(call)?;
        let space = call.ctx.space_id.ok_or(OpError::not_found("That space"))?;
        let initial = match input.state.as_deref() {
            None | Some("working") => RunState::Working,
            Some("accepted") => RunState::Accepted,
            Some(_) => {
                return Err(OpError::invalid(
                    "state",
                    "A run opens working (the default) or accepted.",
                ));
            }
        };
        let work_item_id = match input.cause.task_id.as_deref() {
            Some(reference) => Some(task_in_space(call, space, reference).await?),
            None => None,
        };
        let reporter = reporter(call);
        let tx = call.tx().await?;
        let opened = ops::open(
            tx,
            reporter,
            RunOpen {
                space_id: space,
                client_run_id: input.client_run_id,
                title: input.title,
                cause: Cause {
                    kind: input.cause.kind,
                    conversation_id: input.cause.conversation_id.map(IdRef::id),
                    work_item_id,
                    message_id: input.cause.message_id.map(|message| message.id()),
                    reference: input.cause.reference,
                },
                parent_run_id: input.parent_run_id.map(IdRef::id),
                report_interval_seconds: input.report_interval_seconds,
                settings: input.settings,
                initial,
                requested_by: None,
                via_app_id: None,
                launch_id: None,
            },
        )
        .await?;
        call.audit_target(
            "run",
            opened.run.id,
            json!({ "causeKind": opened.run.cause_kind, "created": opened.created }),
        );
        call.mark_replayed(!opened.created);
        Ok(RunCreateOutput {
            run: RunRecord::from(&opened.run),
            created: opened.created,
        })
    }
}

// -------------------------------------------------------------- run_update

pub struct RunUpdateOp;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunUpdateInput {
    /// The run (one you opened).
    pub run_id: IdRef,
    /// Move the run: accepted, working, waiting, done, failed or cancelled
    /// (along the allowed steps; a finished run stays finished).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// One line on what it's doing now. Empty clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// An https:// address where the run can be followed elsewhere. Empty
    /// clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_url: Option<String>,
    /// What runs it (a harness or framework name), as you name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Why it failed, for a failed run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// What it did, for a finished run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_summary: Option<String>,
    /// Change only if the run is still at this version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for RunUpdateInput {
    fn target(&self) -> Target {
        Target::thing("run", self.run_id.id())
    }
}

impl Operation for RunUpdateOp {
    const NAME: &'static str = "run_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RunsReport);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Update a run";
    const DESCRIPTION: &'static str = "Change a run you opened: move it (accepted, working, \
        waiting, done, failed, cancelled, along the allowed steps), or set its status line, \
        title, link, backend, failure reason or outcome. Any call counts as a report, so a quiet \
        run is back to ok. expectedVersion changes it only if nobody else has; clientRequestId \
        makes a retry apply once.";
    const EMITS: &'static [&'static str] = &["run.changed"];

    type Input = RunUpdateInput;
    type Output = RunRecordValue;

    async fn run(call: &mut OpCall<'_>, input: RunUpdateInput) -> Result<RunRecordValue, OpError> {
        report_rate(call)?;
        let run_id = input.run_id.id();
        let next = match input.state.as_deref().map(str::parse::<RunState>) {
            None => None,
            Some(Ok(next)) => Some(next),
            Some(Err(_)) => {
                return Err(OpError::invalid(
                    "state",
                    "Use accepted, working, waiting, done, failed or cancelled.",
                ));
            }
        };
        own_run(call, run_id).await?;
        let reservation = match reserve_change(call, run_id, "update", &input).await? {
            Keyed::Replay(value) => return Ok(value),
            Keyed::Reserved(reservation) => reservation,
        };
        let reporter = reporter(call);
        let tx = call.tx().await?;
        let run = ops::update(
            tx,
            reporter,
            run_id,
            RunUpdate {
                state: next,
                status_line: input.status_line,
                title: input.title,
                external_url: input.external_url,
                backend: input.backend,
                failure_reason: input.failure_reason,
                outcome_summary: input.outcome_summary,
                expected_version: input.expected_version,
            },
        )
        .await?;
        let value = RunRecordValue::of(&run);
        finish_change(call, reservation, &value).await?;
        call.audit_target("run", run.id, json!({ "state": run.state }));
        Ok(value)
    }
}

// ----------------------------------------------------- run_activity_report

pub struct RunActivityReport;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunActivityInput {
    /// Your own id for this activity, unique within the run. Sending it
    /// again is counted as a duplicate, never an error, so a retried batch
    /// is safe.
    pub id: String,
    /// run_started, run_finished, run_error, step_started, step_finished,
    /// text_message, reasoning, tool_call, plan, state_snapshot,
    /// state_delta, interrupt, interrupt_resolved, usage, result, gap or
    /// custom.
    pub kind: String,
    /// When it happened (RFC 3339). Defaults to now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The step it belongs to, your own name for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_ref: Option<String>,
    /// The activity it is under, your own name for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<String>,
    /// One line to show for it. Made from the payload when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// A JSON object, by kind: run_finished {outcome: done, failed or
    /// cancelled}; text_message and reasoning {text}; tool_call {toolCallRef,
    /// tool, status, ...}, the same toolCallRef for every report of one call;
    /// plan {items: [{title, status}]}, the whole list each time; interrupt
    /// {request: {kind: question or approval, title, detail?, options?:
    /// [{id, label, style?}], allowText?, audience?: {accounts, roles},
    /// clientRequestId?, expiresInSeconds?}}, which opens a request, or
    /// {requestId}; interrupt_resolved {requestId}; gap {reason}. At most
    /// 48 KiB; secrets are removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunActivityReportInput {
    /// The run (one you opened).
    pub run_id: IdRef,
    /// 1 to 100 activities, in the order they happened (at most 256 KiB).
    pub activities: Vec<RunActivityInput>,
}

impl Targeted for RunActivityReportInput {
    fn target(&self) -> Target {
        Target::thing("run", self.run_id.id())
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunActivityOutput {
    /// Activities stored by this call.
    pub accepted: usize,
    /// Activities already stored under the same id.
    pub duplicates: usize,
    /// Activities refused because the run reached its limit of 20,000; one
    /// gap activity records them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub over_limit: usize,
    /// The sequence number of the run's last activity.
    pub last_seq: i64,
    pub state: String,
    pub version: i64,
    /// The requests this batch's interrupts opened: answer them with the
    /// request tools, or report interrupt_resolved when you no longer wait.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<RaisedRequestOutput>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RaisedRequestOutput {
    /// The interrupt activity's id.
    pub activity_id: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

const fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl Operation for RunActivityReport {
    const NAME: &'static str = "run_activity_report";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RunsReport);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const MAX_BODY_BYTES: usize = 512 * 1024;
    const TITLE: &'static str = "Report run activity";
    const DESCRIPTION: &'static str = "Report what a run you opened did, up to 100 activities a \
        call: steps, tool calls, the plan, messages, reasoning, gaps, interrupts and run_finished \
        (payload.outcome done, failed or cancelled, which finishes the run). Each has your own \
        id; sending it again counts as a duplicate, so retry a whole batch freely. Secrets are \
        removed before anything is stored. An interrupt that asks people something needs \
        requests.raise.";
    const EMITS: &'static [&'static str] = &["run.activity"];

    type Input = RunActivityReportInput;
    type Output = RunActivityOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RunActivityReportInput,
    ) -> Result<RunActivityOutput, OpError> {
        report_rate(call)?;
        let run_id = input.run_id.id();
        if serde_json::to_vec(&input.activities).map_or(usize::MAX, |bytes| bytes.len())
            > MAX_CALL_BYTES
        {
            return Err(OpError::invalid(
                "activities",
                "Send at most 256 KiB of activities at a time; split the batch.",
            ));
        }
        let raises = input.activities.iter().any(|activity| {
            activity.kind == "interrupt"
                && activity
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("request"))
                    .is_some_and(serde_json::Value::is_object)
        });
        let mut batch = Vec::with_capacity(input.activities.len());
        for activity in input.activities {
            let occurred_at = match activity.at.as_deref() {
                Some(at) => rfc3339(at, "activities.at")?,
                None => Utc::now(),
            };
            batch.push(NewActivity {
                client_id: activity.id,
                kind: activity.kind,
                occurred_at,
                step_ref: activity.step_ref,
                parent_ref: activity.parent_ref,
                summary: activity.summary,
                payload: activity
                    .payload
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
            });
        }
        own_run(call, run_id).await?;
        if raises {
            // Asking people something needs requests.raise where the run is.
            let located = Located {
                workspace_id: call.ctx.workspace_id,
                space_id: call.ctx.space_id,
            };
            let mut connection = call
                .state
                .pool
                .acquire()
                .await
                .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
            access_shim::check(
                &mut connection,
                &call.ctx.caller,
                &call.ctx.target,
                &located,
                Some(Perm::RequestsRaise),
                true,
                "Raising a request",
            )
            .await?;
        }
        let reporter = reporter(call);
        let tx = call.tx().await?;
        let appended = ops::append(tx, reporter, run_id, batch).await?;
        call.audit_target(
            "run",
            run_id,
            json!({ "accepted": appended.accepted, "duplicates": appended.duplicates }),
        );
        Ok(RunActivityOutput {
            accepted: appended.accepted,
            duplicates: appended.duplicates,
            over_limit: appended.over_limit,
            last_seq: appended.run.activity_seq,
            state: appended.run.state.clone(),
            version: appended.run.version,
            requests: appended
                .requests
                .into_iter()
                .map(|raised| RaisedRequestOutput {
                    activity_id: raised.activity_id,
                    request_id: raised.request_id.to_string(),
                    client_request_id: raised.client_request_id,
                })
                .collect(),
        })
    }
}

// -------------------------------------------------------- run_usage_report

pub struct RunUsageReport;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunUsageReportInput {
    /// Your own id for this report, the same on a retry: a report is
    /// recorded once.
    pub report_key: String,
    /// The run it was for (one you opened). Leave out for usage over a time
    /// window (windowStart and windowEnd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<IdRef>,
    /// Where the window starts (RFC 3339), for usage not tied to one run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start: Option<String>,
    /// Where the window ends (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_end: Option<String>,
    /// The space a window report is for: its id, link or key. Not needed
    /// with runId.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// The model the tokens were on, as its provider names it.
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    /// Input tokens served from a cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<i64>,
    /// What it cost, if you know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// The cost's currency code (default USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

impl Targeted for RunUsageReportInput {
    fn target(&self) -> Target {
        match (self.run_id, self.space_id) {
            (Some(run), _) => Target::thing("run", run.id()),
            (None, Some(space)) => Target::Space(space.id()),
            (None, None) => Target::Workspace(None),
        }
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunUsageReportOutput {
    pub report_id: String,
    /// True when this reportKey was recorded before; nothing was added.
    pub duplicate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

impl Operation for RunUsageReport {
    const NAME: &'static str = "run_usage_report";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RunsReport);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Report usage";
    const DESCRIPTION: &'static str = "Report tokens (and cost, if you know it) for a run you \
        opened, or for a time window in a space when you meter by time (windowStart and \
        windowEnd). Each reportKey is recorded once, so a retry adds nothing. run_usage reads the \
        totals.";
    const EMITS: &'static [&'static str] = &["run.usage_reported"];

    type Input = RunUsageReportInput;
    type Output = RunUsageReportOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RunUsageReportInput,
    ) -> Result<RunUsageReportOutput, OpError> {
        report_rate(call)?;
        let space = call.ctx.space_id.ok_or_else(|| {
            OpError::invalid(
                "spaceId",
                "Name the run (runId), or the space the usage was for (spaceId).",
            )
        })?;
        let window = match (input.window_start.as_deref(), input.window_end.as_deref()) {
            (Some(start), Some(end)) => {
                Some((rfc3339(start, "windowStart")?, rfc3339(end, "windowEnd")?))
            }
            (None, None) => None,
            _ => {
                return Err(OpError::invalid(
                    "windowEnd",
                    "A window report gives both windowStart and windowEnd.",
                ));
            }
        };
        let run_id = input.run_id.map(IdRef::id);
        if let Some(run_id) = run_id {
            own_run(call, run_id).await?;
        }
        let reporter = reporter(call);
        let tx = call.tx().await?;
        let recorded = ops::record_usage(
            tx,
            reporter,
            UsageInput {
                report_key: input.report_key,
                run_id,
                window,
                space_id: space,
                model: input.model,
                input_tokens: input.input_tokens.unwrap_or(0),
                output_tokens: input.output_tokens.unwrap_or(0),
                cached_input_tokens: input.cached_input_tokens.unwrap_or(0),
                cost: input.cost,
                currency: input.currency,
            },
        )
        .await?;
        call.audit_target(
            "usage_report",
            recorded.report_id,
            json!({ "duplicate": recorded.duplicate }),
        );
        call.mark_replayed(recorded.duplicate);
        Ok(RunUsageReportOutput {
            report_id: recorded.report_id.to_string(),
            duplicate: recorded.duplicate,
            run_id: recorded.run_id.map(|id| id.to_string()),
        })
    }
}

// ------------------------------------------------------- run_result_report

pub struct RunResultReport;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunResultInput {
    /// pull_request, branch, diff, thing, file or link.
    pub kind: String,
    /// Its https:// address. Needed for pull_request and link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Its id. Needed for diff, thing and file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The branch. Needed for branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// A pull request's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    /// For a thing: what kind (task, page, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_kind: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunResultReportInput {
    /// The run (one you opened).
    pub run_id: IdRef,
    /// 1 to 20 results. Each is kept once (by kind and address), so sending
    /// one again changes nothing.
    pub results: Vec<RunResultInput>,
}

impl Targeted for RunResultReportInput {
    fn target(&self) -> Target {
        Target::thing("run", self.run_id.id())
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunResultOutput {
    /// Results added by this call.
    pub added: usize,
    pub run: RunRecord,
}

impl Operation for RunResultReport {
    const NAME: &'static str = "run_result_report";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RunsReport);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Report run results";
    const DESCRIPTION: &'static str = "Attach what a run of yours produced: a pull request, \
        branch, diff, file, link, or a thing in the workspace. Each result is kept once by kind \
        and address, so sending one again changes nothing.";
    const EMITS: &'static [&'static str] = &["run.result_added"];

    type Input = RunResultReportInput;
    type Output = RunResultOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RunResultReportInput,
    ) -> Result<RunResultOutput, OpError> {
        report_rate(call)?;
        let run_id = input.run_id.id();
        own_run(call, run_id).await?;
        let results = input
            .results
            .into_iter()
            .map(|result| RunResult {
                kind: result.kind,
                url: result.url,
                id: result.id,
                title: result.title,
                branch: result.branch,
                number: result.number,
                thing_kind: result.thing_kind,
            })
            .collect();
        let reporter = reporter(call);
        let tx = call.tx().await?;
        let (run, added) = ops::add_results(tx, reporter, run_id, results).await?;
        call.audit_target("run", run.id, json!({ "added": added }));
        Ok(RunResultOutput {
            added,
            run: RunRecord::from(&run),
        })
    }
}

// -------------------------------------------- run_cancel, mark_not_started

pub struct RunCancel;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCancelInput {
    /// The run.
    pub run_id: IdRef,
    /// Why, in a line; the agent sees it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Targeted for RunCancelInput {
    fn target(&self) -> Target {
        Target::thing("run", self.run_id.id())
    }
}

impl Operation for RunCancel {
    const NAME: &'static str = "run_cancel";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RunsInterrupt);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Stop a run";
    const DESCRIPTION: &'static str = "Ask a run to stop. A run still queued is cancelled at \
        once; one that's going shows cancel_requested until the agent doing it confirms (it hears \
        run.cancel, or loses its claim on the request it answers), and a stop nobody confirms \
        ends it after ten minutes. A run on the built-in host stops as its Stop button does. \
        Asking again changes nothing.";
    const EMITS: &'static [&'static str] = &["run.cancel_requested"];

    type Input = RunCancelInput;
    type Output = RunRecordValue;

    async fn run(call: &mut OpCall<'_>, input: RunCancelInput) -> Result<RunRecordValue, OpError> {
        let run_id = input.run_id.id();
        let builtin = {
            let tx = call.tx().await?;
            ops::is_builtin(tx, run_id).await?
        };
        if builtin {
            // A run on the built-in host stops the way its Stop button does.
            let key = given_key(call)
                .map(Ok)
                .unwrap_or_else(|| IdempotencyKey::parse(&Uuid::now_v7().to_string()))
                .map_err(|_| OpError::Unavailable)?;
            let actor = crate::work_item_commands::CommandActor::from(call.ctx);
            let caller = RunCaller {
                actor_id: call.ctx.caller.account_id,
                actor_kind: call.ctx.caller.kind,
                via_grant_id: call.ctx.caller.via_grant_id,
                audit: actor.audit,
            };
            crate::api::interrupt_run_in(call.state, &caller, run_id, Ok(key)).await?;
            let run = store::get(&call.state.pool, run_id)
                .await?
                .ok_or(OpError::not_found(RUN))?;
            call.audit_target("run", run.id, json!({ "state": run.state }));
            return Ok(RunRecordValue::of(&run));
        }
        let reservation = match reserve_change(call, run_id, "cancel", &input).await? {
            Keyed::Replay(value) => return Ok(value),
            Keyed::Reserved(reservation) => reservation,
        };
        let by = call.account_id();
        let tx = call.tx().await?;
        // A launched run stops through its reply request, so a runner that
        // only knows the reply tools hears it and loses its claim (A1.4).
        if let Some(request_id) = super::launch_paths::open_request_of(tx, run_id).await? {
            crate::replies::stop_for_run(tx, by, request_id, input.reason.clone()).await?;
        }
        let run = ops::request_cancel(tx, by, run_id, input.reason).await?;
        let value = RunRecordValue::of(&run);
        finish_change(call, reservation, &value).await?;
        call.audit_target("run", run.id, json!({ "state": run.state }));
        Ok(value)
    }
}

pub struct RunMarkNotStarted;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunMarkNotStartedInput {
    /// The run: one whose start is uncertain (start_uncertain).
    pub run_id: IdRef,
}

impl Targeted for RunMarkNotStartedInput {
    fn target(&self) -> Target {
        Target::thing("run", self.run_id.id())
    }
}

impl Operation for RunMarkNotStarted {
    const NAME: &'static str = "run_mark_not_started";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::RunsInterrupt);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Mark a run not started";
    const DESCRIPTION: &'static str = "Mark a run whose start is uncertain (start_uncertain: the \
        host may or may not have started it) as never started. It becomes failed, \"Marked as not \
        started\", so it can be launched again.";
    const EMITS: &'static [&'static str] = &["run.changed"];

    type Input = RunMarkNotStartedInput;
    type Output = RunRecordValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: RunMarkNotStartedInput,
    ) -> Result<RunRecordValue, OpError> {
        let tx = call.tx().await?;
        let run = ops::mark_not_started(tx, input.run_id.id()).await?;
        call.audit_target("run", run.id, json!({}));
        Ok(RunRecordValue::of(&run))
    }
}

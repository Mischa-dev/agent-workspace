//! A1.3: run operations for any agent, as MCP tools (and through the public
//! API, which serves every tool). An account opens a run for its own work
//! and reports on it; anyone who may stop runs where it lives can ask it to
//! stop. The rules are `crate::runs::ops`; this module checks the caller's
//! permission in the run's project and shapes the answers.
//!
//! The tools answer in the `{outcome, value}` envelope and take
//! `clientRequestId` where a retry could repeat a change, so moving them
//! onto K2's operation registry is mechanical.

use chrono::{DateTime, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id, parse_message_id, project_is_chosen},
    talk::can_write_in,
    tasks::resolve_task,
    writes::{REQUEST_ID_MESSAGE, parse_request_id},
};
use crate::{
    AppState,
    error::AppError,
    idempotency::{IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope},
    policy::Action,
    runs::{
        RunState,
        activities::NewActivity,
        ops::{self, Cause, Reporter, RunOpError, RunOpen, RunResult, RunUpdate, UsageInput},
    },
};

/// The most bytes of activities in one call.
const MAX_CALL_BYTES: usize = 256 * 1024;
const RUN_MESSAGE: &str = "That run";
const REPORT_DENIED: &str =
    "Your role here can't report runs. Ask a project admin for the runs.report permission.";
const STOP_DENIED: &str =
    "Your role here can't stop runs. Ask a project admin for the runs.interrupt permission.";

/// A run as the run operations return it.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunRecord {
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

impl From<&crate::runs::store::RunRow> for RunRecord {
    fn from(run: &crate::runs::store::RunRow) -> Self {
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

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunRecordValue {
    pub run: RunRecord,
    /// True when this answers a retry of a call that already went through.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

// ---------------------------------------------------------------- run_open

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunCauseArgument {
    /// Why the run exists: task, thread, message, schedule, host_trigger,
    /// automation, request or manual.
    pub kind: String,
    /// The task it works on (key like AWD-12, ID or link). Needed for task.
    pub task_id: Option<String>,
    /// The conversation or thread it belongs to (ID or link). Needed for
    /// thread. People who can't see the conversation can't see the run.
    pub conversation_id: Option<String>,
    /// The message that asked for it (ID or link). Needed for message.
    pub message_id: Option<String>,
    /// The schedule's or trigger's name. Needed for schedule and
    /// host_trigger.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunOpenArguments {
    /// The project the run is in (ID, link or key). Defaults to the one your
    /// connection is in.
    pub project_id: Option<String>,
    /// Your own ID for this run, unique among your runs. Opening again with
    /// the same one returns the run you already opened (created: false), so
    /// a retry never makes a second run.
    pub client_run_id: Option<String>,
    /// What the run is doing, in a line (at most 200 characters).
    pub title: String,
    pub cause: RunCauseArgument,
    /// A run of yours in the same project this one is part of.
    pub parent_run_id: Option<String>,
    /// How often you promise to report, in seconds (30 to 86,400; default
    /// 900). A run silent for one interval shows as quiet, for three as not
    /// responding.
    pub report_interval_seconds: Option<i32>,
    /// What the run was started with (a JSON object of at most 16 KiB).
    /// Fixed once set; secrets are removed.
    pub settings: Option<serde_json::Value>,
    /// working (the default) or accepted, for a run that hasn't started
    /// work yet.
    pub state: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunOpenValue {
    pub run: RunRecord,
    /// False when clientRunId named a run you had already opened.
    pub created: bool,
}

pub(super) async fn run_open(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunOpenArguments,
) -> ToolResult<RunOpenValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if let Some(refused) = rate_limited(state, principal) {
        return refused;
    }
    let result = async {
        if !project_is_chosen(principal) {
            return Ok(ToolResult::invalid(
                "projectId",
                "Name the project the run is in (projectId).",
            ));
        }
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::RunReport).await? {
            return Ok(ToolResult::denied(REPORT_DENIED));
        }
        let initial = match arguments.state.as_deref() {
            None | Some("working") => RunState::Working,
            Some("accepted") => RunState::Accepted,
            Some(_) => {
                return Ok(ToolResult::invalid(
                    "state",
                    "A run opens working (the default) or accepted.",
                ));
            }
        };
        let cause = &arguments.cause;
        let work_item_id = match cause.task_id.as_deref() {
            Some(reference) => match resolve_task(state, principal, reference).await? {
                Some((task_id, task_project)) if task_project == project_id => Some(task_id),
                _ => return Ok(ToolResult::not_found("That task")),
            },
            None => None,
        };
        let conversation_id = match cause.conversation_id.as_deref() {
            Some(value) => match parse_id(value) {
                Some(id) => Some(id),
                None => return Ok(ToolResult::invalid("cause.conversationId", "Give a conversation ID or link.")),
            },
            None => None,
        };
        let message_id = match cause.message_id.as_deref() {
            Some(value) => match parse_message_id(value) {
                Some(id) => Some(id),
                None => return Ok(ToolResult::invalid("cause.messageId", "Give a message ID or link.")),
            },
            None => None,
        };
        let parent_run_id = match arguments.parent_run_id.as_deref() {
            Some(value) => match parse_id(value) {
                Some(id) => Some(id),
                None => return Ok(ToolResult::invalid("parentRunId", "Give a run ID.")),
            },
            None => None,
        };
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let opened = match ops::open(
            &mut transaction,
            Reporter::Account {
                account_id: principal.actor_id,
            },
            RunOpen {
                space_id: project_id,
                client_run_id: arguments.client_run_id.clone(),
                title: arguments.title.clone(),
                cause: Cause {
                    kind: cause.kind.clone(),
                    conversation_id,
                    work_item_id,
                    message_id,
                    reference: cause.reference.clone(),
                },
                parent_run_id,
                report_interval_seconds: arguments.report_interval_seconds,
                settings: arguments.settings.clone(),
                initial,
                requested_by: None,
                via_app_id: None,
                launch_id: None,
            },
        )
        .await
        {
            Ok(opened) => opened,
            Err(error) => return op_error(error, "run_open"),
        };
        if opened.created {
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                "run.open",
                "run",
                opened.run.id,
                serde_json::json!({ "causeKind": opened.run.cause_kind }),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(ToolResult::ok(RunOpenValue {
            run: RunRecord::from(&opened.run),
            created: opened.created,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "run_open"))
}

// -------------------------------------------------------------- run_update

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunUpdateArguments {
    /// The run's ID (one you opened).
    pub run_id: String,
    /// Move the run: accepted, working, waiting, done, failed or cancelled
    /// (along the allowed steps; a finished run stays finished).
    pub state: Option<String>,
    /// One line on what it's doing now. Empty clears it.
    pub status_line: Option<String>,
    pub title: Option<String>,
    /// An https:// address where the run can be followed elsewhere. Empty
    /// clears it.
    pub external_url: Option<String>,
    /// What runs it (a harness or framework name), as you name it.
    pub backend: Option<String>,
    /// Why it failed, for a failed run.
    pub failure_reason: Option<String>,
    /// What it did, for a finished run.
    pub outcome_summary: Option<String>,
    /// Change only if the run is still at this version.
    pub expected_version: Option<i64>,
    /// A stable ID so a retry of this call does the same thing once.
    pub client_request_id: Option<String>,
}

pub(super) async fn run_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunUpdateArguments,
) -> ToolResult<RunRecordValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if let Some(refused) = rate_limited(state, principal) {
        return refused;
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Give the run's ID.");
    };
    let key = match request_key(arguments.client_request_id.as_deref()) {
        Ok(key) => key,
        Err(refused) => return *refused,
    };
    let next = match arguments.state.as_deref().map(str::parse::<RunState>) {
        None => None,
        Some(Ok(next)) => Some(next),
        Some(Err(_)) => {
            return ToolResult::invalid(
                "state",
                "Use accepted, working, waiting, done, failed or cancelled.",
            );
        }
    };
    let result = async {
        if let Some(refused) = reporter_check(state, principal, run_id).await? {
            return Ok(refused);
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let reservation = match reserve_key(&mut transaction, principal, run_id, "update", &key, &arguments).await? {
            Keyed::Replay(value) => {
                transaction.commit().await?;
                return Ok(ToolResult::ok(value));
            }
            Keyed::Refused(refused) => return Ok(refused),
            Keyed::Reserved(reservation) => reservation,
        };
        let run = match ops::update(
            &mut transaction,
            Reporter::Account {
                account_id: principal.actor_id,
            },
            run_id,
            RunUpdate {
                state: next,
                status_line: arguments.status_line.clone(),
                title: arguments.title.clone(),
                external_url: arguments.external_url.clone(),
                backend: arguments.backend.clone(),
                failure_reason: arguments.failure_reason.clone(),
                outcome_summary: arguments.outcome_summary.clone(),
                expected_version: arguments.expected_version,
            },
        )
        .await
        {
            Ok(run) => run,
            Err(error) => return op_error(error, "run_update"),
        };
        if next.is_some() {
            insert_audit(
                &mut transaction,
                principal,
                principal.project_id,
                "run.update",
                "run",
                run.id,
                serde_json::json!({ "state": run.state }),
            )
            .await?;
        }
        let value = RunRecordValue {
            run: RunRecord::from(&run),
            idempotent_replay: false,
        };
        finish_key(&mut transaction, reservation, &value).await?;
        transaction.commit().await?;
        Ok(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "run_update"))
}

// ------------------------------------------------------------ run_activity

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunActivityArgument {
    /// Your own ID for this activity, unique within the run. Sending it
    /// again is counted as a duplicate, never an error, so a retried batch
    /// is safe.
    pub id: String,
    /// run_started, run_finished, run_error, step_started, step_finished,
    /// text_message, reasoning, tool_call, plan, state_snapshot,
    /// state_delta, interrupt, interrupt_resolved, usage, result, gap or
    /// custom.
    pub kind: String,
    /// When it happened (RFC 3339). Defaults to now.
    pub at: Option<String>,
    /// The step it belongs to, your own name for it.
    pub step_ref: Option<String>,
    /// The activity it is under, your own name for it.
    pub parent_ref: Option<String>,
    /// One line to show for it. Made from the payload when left out.
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
    pub payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunActivityArguments {
    /// The run's ID (one you opened).
    pub run_id: String,
    /// 1 to 100 activities, in the order they happened (at most 256 KiB).
    pub activities: Vec<RunActivityArgument>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunActivityValue {
    /// Activities stored by this call.
    pub accepted: usize,
    /// Activities already stored under the same id.
    pub duplicates: usize,
    /// Activities refused because the run reached its limit of 20,000; one
    /// gap activity records them.
    #[serde(skip_serializing_if = "is_zero")]
    pub over_limit: usize,
    /// The sequence number of the run's last activity.
    pub last_seq: i64,
    pub state: String,
    pub version: i64,
    /// The requests this batch's interrupts opened: answer them with the
    /// request tools, or report interrupt_resolved when you no longer wait.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<RaisedRequestValue>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RaisedRequestValue {
    /// The interrupt activity's id.
    pub activity_id: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

const fn is_zero(value: &usize) -> bool {
    *value == 0
}

pub(super) async fn run_activity(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunActivityArguments,
) -> ToolResult<RunActivityValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if let Some(refused) = rate_limited(state, principal) {
        return refused;
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Give the run's ID.");
    };
    if serde_json::to_vec(&arguments.activities).map_or(usize::MAX, |bytes| bytes.len())
        > MAX_CALL_BYTES
    {
        return ToolResult::invalid(
            "activities",
            "Send at most 256 KiB of activities at a time; split the batch.",
        );
    }
    let raises = arguments.activities.iter().any(|activity| {
        activity.kind == "interrupt"
            && activity
                .payload
                .as_ref()
                .and_then(|payload| payload.get("request"))
                .is_some_and(serde_json::Value::is_object)
    });
    let mut batch = Vec::with_capacity(arguments.activities.len());
    for activity in arguments.activities {
        let occurred_at = match activity.at.as_deref() {
            Some(at) => match DateTime::parse_from_rfc3339(at.trim()) {
                Ok(at) => at.with_timezone(&Utc),
                Err(_) => {
                    return ToolResult::invalid(
                        "activities.at",
                        "Give the time as RFC 3339, like 2026-11-14T09:00:00Z.",
                    );
                }
            },
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
    let result = async {
        if let Some(refused) = reporter_check(state, principal, run_id).await? {
            return Ok(refused);
        }
        if raises && !can_write_in(state, principal, principal.project_id, Action::RequestRaise).await? {
            return Ok(ToolResult::denied(
                "Your role here can't ask people things. Ask a project admin for the requests.raise permission.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let appended = match ops::append(
            &mut transaction,
            Reporter::Account {
                account_id: principal.actor_id,
            },
            run_id,
            batch,
        )
        .await
        {
            Ok(appended) => appended,
            Err(error) => return op_error(error, "run_activity"),
        };
        transaction.commit().await?;
        Ok(ToolResult::ok(RunActivityValue {
            accepted: appended.accepted,
            duplicates: appended.duplicates,
            over_limit: appended.over_limit,
            last_seq: appended.run.activity_seq,
            state: appended.run.state.clone(),
            version: appended.run.version,
            requests: appended
                .requests
                .into_iter()
                .map(|raised| RaisedRequestValue {
                    activity_id: raised.activity_id,
                    request_id: raised.request_id.to_string(),
                    client_request_id: raised.client_request_id,
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "run_activity"))
}

// -------------------------------------------------------- run_usage_report

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunUsageReportArguments {
    /// Your own ID for this report, the same on a retry: a report is
    /// recorded once.
    pub report_key: String,
    /// The run it was for (one you opened). Leave out for usage over a time
    /// window (windowStart and windowEnd).
    pub run_id: Option<String>,
    /// Where the window starts (RFC 3339), for usage not tied to one run.
    pub window_start: Option<String>,
    /// Where the window ends (RFC 3339).
    pub window_end: Option<String>,
    /// The project a window report is for (ID, link or key). Defaults to
    /// the one your connection is in.
    pub project_id: Option<String>,
    /// The model the tokens were on, as its provider names it.
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// Input tokens served from a cache.
    pub cached_input_tokens: Option<i64>,
    /// What it cost, if you know.
    pub cost: Option<f64>,
    /// The cost's currency code (default USD).
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunUsageReportValue {
    pub report_id: String,
    /// True when this reportKey was recorded before; nothing was added.
    pub duplicate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

pub(super) async fn run_usage_report(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunUsageReportArguments,
) -> ToolResult<RunUsageReportValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if let Some(refused) = rate_limited(state, principal) {
        return refused;
    }
    let run_id = match arguments.run_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("runId", "Give the run's ID."),
        },
        None => None,
    };
    let time = |value: Option<&str>, field: &'static str| match value {
        Some(value) => DateTime::parse_from_rfc3339(value.trim())
            .map(|at| Some(at.with_timezone(&Utc)))
            .map_err(|_| field),
        None => Ok(None),
    };
    let window = match (
        time(arguments.window_start.as_deref(), "windowStart"),
        time(arguments.window_end.as_deref(), "windowEnd"),
    ) {
        (Ok(Some(start)), Ok(Some(end))) => Some((start, end)),
        (Ok(None), Ok(None)) => None,
        (Err(field), _) | (_, Err(field)) => {
            return ToolResult::invalid(field, "Give the time as RFC 3339, like 2026-11-14T09:00:00Z.");
        }
        _ => {
            return ToolResult::invalid(
                "windowEnd",
                "A window report gives both windowStart and windowEnd.",
            );
        }
    };
    let result = async {
        match run_id {
            Some(run_id) => {
                if let Some(refused) = reporter_check(state, principal, run_id).await? {
                    return Ok(refused);
                }
            }
            None => {
                if !project_is_chosen(principal) {
                    return Ok(ToolResult::invalid(
                        "projectId",
                        "Name the project the usage was for (projectId).",
                    ));
                }
                if !can_write_in(state, principal, principal.project_id, Action::RunReport).await? {
                    return Ok(ToolResult::denied(REPORT_DENIED));
                }
            }
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let recorded = match ops::record_usage(
            &mut transaction,
            Reporter::Account {
                account_id: principal.actor_id,
            },
            UsageInput {
                report_key: arguments.report_key.clone(),
                run_id,
                window,
                space_id: principal.project_id,
                model: arguments.model.clone(),
                input_tokens: arguments.input_tokens.unwrap_or(0),
                output_tokens: arguments.output_tokens.unwrap_or(0),
                cached_input_tokens: arguments.cached_input_tokens.unwrap_or(0),
                cost: arguments.cost,
                currency: arguments.currency.clone(),
            },
        )
        .await
        {
            Ok(recorded) => recorded,
            Err(error) => return op_error(error, "run_usage_report"),
        };
        transaction.commit().await?;
        Ok(ToolResult::ok(RunUsageReportValue {
            report_id: recorded.report_id.to_string(),
            duplicate: recorded.duplicate,
            run_id: recorded.run_id.map(|id| id.to_string()),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "run_usage_report"))
}

// -------------------------------------------------------------- run_result

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunResultArgument {
    /// pull_request, branch, diff, thing, file or link.
    pub kind: String,
    /// Its https:// address. Needed for pull_request and link.
    pub url: Option<String>,
    /// Its ID. Needed for diff, thing and file.
    pub id: Option<String>,
    pub title: Option<String>,
    /// The branch. Needed for branch.
    pub branch: Option<String>,
    /// A pull request's number.
    pub number: Option<i64>,
    /// For a thing: what kind (task, page, ...).
    pub thing_kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunResultArguments {
    /// The run's ID (one you opened).
    pub run_id: String,
    /// 1 to 20 results. Each is kept once (by kind and address), so sending
    /// one again changes nothing.
    pub results: Vec<RunResultArgument>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunResultValue {
    /// Results added by this call.
    pub added: usize,
    pub run: RunRecord,
}

pub(super) async fn run_result(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunResultArguments,
) -> ToolResult<RunResultValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if let Some(refused) = rate_limited(state, principal) {
        return refused;
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Give the run's ID.");
    };
    let result = async {
        if let Some(refused) = reporter_check(state, principal, run_id).await? {
            return Ok(refused);
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let results = arguments
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
        let (run, added) = match ops::add_results(
            &mut transaction,
            Reporter::Account {
                account_id: principal.actor_id,
            },
            run_id,
            results,
        )
        .await
        {
            Ok(done) => done,
            Err(error) => return op_error(error, "run_result"),
        };
        transaction.commit().await?;
        Ok(ToolResult::ok(RunResultValue {
            added,
            run: RunRecord::from(&run),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "run_result"))
}

// -------------------------------------------- run_cancel, mark_not_started

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunCancelArguments {
    /// The run's ID.
    pub run_id: String,
    /// Why, in a line; the agent sees it.
    pub reason: Option<String>,
    /// A stable ID so a retry of this call does the same thing once.
    pub client_request_id: Option<String>,
}

pub(super) async fn run_cancel(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunCancelArguments,
) -> ToolResult<RunRecordValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Give the run's ID.");
    };
    let key = match request_key(arguments.client_request_id.as_deref()) {
        Ok(key) => key,
        Err(refused) => return *refused,
    };
    let result = async {
        if let Some(refused) = stopper_check(state, principal, run_id).await? {
            return Ok(refused);
        }
        let mut transaction = state.pool.begin().await?;
        if ops::is_builtin(&mut transaction, run_id).await? {
            // A run on the built-in host stops the way its Stop button does.
            transaction.rollback().await?;
            let stopped = super::runs::run_stop(
                state,
                principal,
                super::runs::RunControlArguments {
                    run_id: run_id.to_string(),
                    client_request_id: Some(key.as_str().to_owned()),
                },
            )
            .await;
            return Ok(match stopped {
                ToolResult::Ok { .. } => match crate::runs::store::get(&state.pool, run_id).await? {
                    Some(run) => ToolResult::ok(RunRecordValue {
                        run: RunRecord::from(&run),
                        idempotent_replay: false,
                    }),
                    None => ToolResult::not_found(RUN_MESSAGE),
                },
                ToolResult::NotFound { message } => ToolResult::NotFound { message },
                ToolResult::InvalidRequest { field, message } => {
                    ToolResult::InvalidRequest { field, message }
                }
                ToolResult::PermissionDenied {
                    message,
                    retry_category,
                } => ToolResult::PermissionDenied {
                    message,
                    retry_category,
                },
                ToolResult::Conflict { message } => ToolResult::Conflict { message },
                ToolResult::TemporarilyUnavailable { message } => {
                    ToolResult::TemporarilyUnavailable { message }
                }
            });
        }
        mark_via(&mut transaction, principal).await?;
        let reservation = match reserve_key(&mut transaction, principal, run_id, "cancel", &key, &arguments).await? {
            Keyed::Replay(value) => {
                transaction.commit().await?;
                return Ok(ToolResult::ok(value));
            }
            Keyed::Refused(refused) => return Ok(refused),
            Keyed::Reserved(reservation) => reservation,
        };
        let run = match ops::request_cancel(
            &mut transaction,
            principal.actor_id,
            run_id,
            arguments.reason.clone(),
        )
        .await
        {
            Ok(run) => run,
            Err(error) => return op_error(error, "run_cancel"),
        };
        insert_audit(
            &mut transaction,
            principal,
            principal.project_id,
            "run.cancel",
            "run",
            run.id,
            serde_json::json!({ "state": run.state }),
        )
        .await?;
        let value = RunRecordValue {
            run: RunRecord::from(&run),
            idempotent_replay: false,
        };
        finish_key(&mut transaction, reservation, &value).await?;
        transaction.commit().await?;
        Ok(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "run_cancel"))
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunMarkNotStartedArguments {
    /// The run's ID: one whose start is uncertain (start_uncertain).
    pub run_id: String,
}

pub(super) async fn run_mark_not_started(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunMarkNotStartedArguments,
) -> ToolResult<RunRecordValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Give the run's ID.");
    };
    let result = async {
        if let Some(refused) = stopper_check(state, principal, run_id).await? {
            return Ok(refused);
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let run = match ops::mark_not_started(&mut transaction, run_id).await {
            Ok(run) => run,
            Err(error) => return op_error(error, "run_mark_not_started"),
        };
        insert_audit(
            &mut transaction,
            principal,
            principal.project_id,
            "run.mark_not_started",
            "run",
            run.id,
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok(ToolResult::ok(RunRecordValue {
            run: RunRecord::from(&run),
            idempotent_replay: false,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "run_mark_not_started"))
}

// ------------------------------------------------------------------ shared

fn rate_limited<T>(state: &AppState, principal: &McpPrincipal) -> Option<ToolResult<T>> {
    match state.rate_limits.check_run_report(principal.actor_id) {
        Ok(()) => None,
        Err(AppError::RateLimited {
            retry_after_seconds,
        }) => Some(ToolResult::TemporarilyUnavailable {
            message: format!(
                "Too many run reports at once. Batch activities (up to 100 a call) and try again in {retry_after_seconds} seconds."
            ),
        }),
        Err(_) => Some(ToolResult::unavailable()),
    }
}

fn op_error<T>(error: RunOpError, operation: &'static str) -> Result<ToolResult<T>, AppError> {
    Ok(match error {
        RunOpError::Refused { field, message } => ToolResult::invalid(field, message),
        RunOpError::NotFound => ToolResult::not_found(RUN_MESSAGE),
        RunOpError::Conflict(message) => ToolResult::conflict(message),
        RunOpError::VersionConflict { current_version } => ToolResult::conflict(format!(
            "The run changed first (now version {current_version}). Read it again, then retry with expectedVersion {current_version}."
        )),
        RunOpError::App(error) => ToolResult::from_error(error, operation),
    })
}

/// The caller may report on this run: it is theirs, in the project the call
/// is in, and their role there still allows reporting. Missing and not
/// theirs answer the same.
async fn reporter_check<T>(
    state: &AppState,
    principal: &McpPrincipal,
    run_id: Uuid,
) -> Result<Option<ToolResult<T>>, AppError> {
    let space =
        crate::runs::store::space_of_own(&state.pool, principal.actor_id, run_id).await?;
    let Some(space) = space.filter(|space| *space == principal.project_id) else {
        return Ok(Some(ToolResult::not_found(RUN_MESSAGE)));
    };
    if !can_write_in(state, principal, space, Action::RunReport).await? {
        return Ok(Some(ToolResult::denied(REPORT_DENIED)));
    }
    Ok(None)
}

/// The caller may stop this run: they can see it, it is in the project the
/// call is in, and their role there allows stopping runs.
async fn stopper_check<T>(
    state: &AppState,
    principal: &McpPrincipal,
    run_id: Uuid,
) -> Result<Option<ToolResult<T>>, AppError> {
    let space =
        crate::runs::store::space_of_visible(&state.pool, principal.actor_id, run_id).await?;
    let Some(space) = space.filter(|space| *space == principal.project_id) else {
        return Ok(Some(ToolResult::not_found(RUN_MESSAGE)));
    };
    if !can_write_in(state, principal, space, Action::RunInterrupt).await? {
        return Ok(Some(ToolResult::denied(STOP_DENIED)));
    }
    Ok(None)
}

#[allow(clippy::result_large_err)]
fn request_key(value: Option<&str>) -> Result<IdempotencyKey, Box<ToolResult<RunRecordValue>>> {
    match value {
        Some(value) => parse_request_id(value)
            .ok_or_else(|| Box::new(ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE))),
        None => parse_request_id(&Uuid::now_v7().to_string())
            .ok_or_else(|| Box::new(ToolResult::unavailable())),
    }
}

enum Keyed {
    Replay(RunRecordValue),
    Refused(ToolResult<RunRecordValue>),
    Reserved(crate::idempotency::IdempotencyReservation),
}

async fn reserve_key<A: Serialize>(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &McpPrincipal,
    run_id: Uuid,
    action: &str,
    key: &IdempotencyKey,
    arguments: &A,
) -> Result<Keyed, AppError> {
    let scope = resource_bound_scope("runs", run_id, action)?;
    let hash = RequestHash::for_value(arguments)?;
    Ok(match reserve(transaction, principal.actor_id, &scope, key, hash).await {
        Ok(IdempotencyOutcome::Replay(replay)) => {
            let mut value = replay.decode::<RunRecordValue>()?;
            value.idempotent_replay = true;
            Keyed::Replay(value)
        }
        Ok(IdempotencyOutcome::Reserved(reservation)) => Keyed::Reserved(reservation),
        Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
            Keyed::Refused(ToolResult::invalid(
                "clientRequestId",
                "That clientRequestId was already used for a different change. Use a new one.",
            ))
        }
        Err(AppError::Conflict("idempotency_key", "command in progress")) => Keyed::Refused(
            ToolResult::conflict("The same change is still being made. Try again in a moment."),
        ),
        Err(error) => return Err(error),
    })
}

async fn finish_key(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reservation: crate::idempotency::IdempotencyReservation,
    value: &RunRecordValue,
) -> Result<(), AppError> {
    complete(transaction, &reservation, axum::http::StatusCode::OK, value).await
}

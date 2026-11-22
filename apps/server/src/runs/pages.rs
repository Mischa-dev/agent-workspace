//! A1.5: the Runs page and the run view, for every run: on the built-in
//! host, on a linked host, or reported by its own agent. Each reads the run
//! record (`runs`, `run_activities`, `run_usage`) and, for a built-in run,
//! the detail only that host keeps (its harness, branch and pull request).
//!
//! Who sees a run is `run_visible_to` (store::visible) plus, for a run in a
//! project, `actor_can_access_project`, the rule every run read follows.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::state::RunState;
use crate::{
    AppState,
    error::AppError,
    harness_catalog::Harnesses,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

/// The most activities the run view reads at once.
pub const MAX_ACTIVITIES: i64 = 500;
const MAX_LIST: i64 = 100;

/// A run as the Runs page lists it.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunLine {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// queued, starting, start_uncertain, accepted, working, waiting,
    /// cancel_requested, done, failed, cancelled, refused or interrupted.
    pub state: String,
    /// ok, quiet or not_responding.
    pub liveness: String,
    pub title: String,
    /// What started it: task, thread, message, schedule, host_trigger,
    /// automation, request or manual.
    pub cause_kind: String,
    pub cause_ref: Option<String>,
    #[schemars(with = "String")]
    pub organization_id: Uuid,
    #[schemars(with = "Option<String>")]
    pub project_id: Option<Uuid>,
    pub project_key: Option<String>,
    pub project_name: Option<String>,
    #[schemars(with = "Option<String>")]
    pub conversation_id: Option<Uuid>,
    pub thread_name: Option<String>,
    #[schemars(with = "Option<String>")]
    pub task_id: Option<Uuid>,
    pub task_key: Option<String>,
    pub task_title: Option<String>,
    /// The agent doing it.
    #[schemars(with = "String")]
    pub agent_actor_id: Uuid,
    pub agent_name: String,
    /// The host running it; none when the agent reports for itself.
    #[schemars(with = "Option<String>")]
    pub host_id: Option<Uuid>,
    pub host_name: Option<String>,
    /// What the host runs it on, only when the host says.
    pub backend: Option<String>,
    /// Whether this server's own host runs it.
    pub builtin: bool,
    pub harness: Option<String>,
    /// The harness's name for people, filled in after the query.
    #[sqlx(default)]
    pub harness_name: Option<String>,
    pub model: Option<String>,
    #[schemars(with = "Option<String>")]
    pub requested_by_actor_id: Option<Uuid>,
    pub requested_by_name: Option<String>,
    /// The open request the run waits on, if any.
    #[schemars(with = "Option<String>")]
    pub waiting_on_request_id: Option<Uuid>,
    pub waiting_on_title: Option<String>,
    pub status_line: Option<String>,
    pub outcome_summary: Option<String>,
    pub external_url: Option<String>,
    pub failure_reason: Option<String>,
    pub branch: Option<String>,
    pub pull_request_number: Option<i64>,
    pub pull_request_url: Option<String>,
    pub pull_request_state: Option<String>,
    pub gap_count: i32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub last_report_at: Option<DateTime<Utc>>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    /// Null when nobody knows the price.
    pub cost: Option<f64>,
    pub currency: Option<String>,
    /// exact (reported for the run), estimated (shared out from a usage
    /// window), or null with no usage.
    pub cost_confidence: Option<String>,
}

/// `$1` is the viewer.
const LINE_SELECT: &str = r#"
    SELECT run.id, run.state, run.liveness, run.title, run.cause_kind, run.cause_ref,
           run.workspace_id AS organization_id, project.id AS project_id,
           project.project_key, project.name AS project_name,
           run.conversation_id, conversation.name AS thread_name,
           task.id AS task_id,
           CASE WHEN task.id IS NOT NULL THEN project.project_key || '-' || task.number END AS task_key,
           task.title AS task_title,
           run.account_id AS agent_actor_id,
           COALESCE(agent.display_name, 'Agent') AS agent_name,
           run.host_id, host.name AS host_name, run.backend,
           detail.id IS NOT NULL AS builtin,
           detail.config->>'harness' AS harness, detail.config->>'model' AS model,
           run.requested_by_account_id AS requested_by_actor_id,
           requester.display_name AS requested_by_name,
           waiting.id AS waiting_on_request_id, waiting.title AS waiting_on_title,
           run.status_line, run.outcome_summary, run.external_url, run.failure_reason,
           workspace.branch_name AS branch,
           pull_request.pull_request_number, pull_request.html_url AS pull_request_url,
           pull_request.state AS pull_request_state,
           run.gap_count, run.created_at, run.started_at, run.finished_at, run.updated_at,
           run.last_report_at,
           COALESCE(usage.input_tokens, 0)::bigint AS input_tokens,
           COALESCE(usage.output_tokens, 0)::bigint AS output_tokens,
           COALESCE(usage.cached_input_tokens, 0)::bigint AS cached_input_tokens,
           usage.cost, usage.currency, usage.confidence AS cost_confidence
    FROM runs run
    LEFT JOIN projects project ON project.id = run.space_id
    LEFT JOIN conversations conversation ON conversation.id = run.conversation_id
    LEFT JOIN agent_runs detail ON detail.id = run.id
    LEFT JOIN hosts host ON host.id = run.host_id
    LEFT JOIN project_memberships agent
      ON agent.project_id = run.space_id AND agent.actor_id = run.account_id
    LEFT JOIN project_memberships requester
      ON requester.project_id = run.space_id AND requester.actor_id = run.requested_by_account_id
    -- What it waits on: the request it names, else its oldest open one, where
    -- the viewer sees the request's conversation.
    LEFT JOIN LATERAL (
        SELECT request.id, request.title FROM requests request
        WHERE request.run_id = run.id
          AND (request.id = run.waiting_on_request_id
               OR (run.waiting_on_request_id IS NULL AND request.status = 'open'
                   AND run.state = 'waiting'))
          AND (request.conversation_id IS NULL
               OR conversation_visible_to(request.conversation_id, $1))
        ORDER BY request.created_at, request.id
        LIMIT 1
    ) waiting ON true
    LEFT JOIN repository_workspaces workspace ON workspace.id = detail.repository_workspace_id
    LEFT JOIN github_pull_request_requests pull_request_request
      ON pull_request_request.agent_run_id = detail.id
    LEFT JOIN github_pull_requests pull_request
      ON pull_request.id = pull_request_request.github_pull_request_id
    LEFT JOIN LATERAL (
        SELECT link.work_item_id FROM task_threads link
        WHERE link.project_id = run.space_id AND link.conversation_id = run.conversation_id
        ORDER BY link.created_at, link.work_item_id
        LIMIT 1
    ) task_link ON true
    LEFT JOIN task_rows task
      ON task.id = COALESCE(run.work_item_id, task_link.work_item_id) AND task.archived_at IS NULL
    LEFT JOIN LATERAL (
        SELECT sum(row.input_tokens) AS input_tokens,
               sum(row.output_tokens) AS output_tokens,
               sum(row.cached_input_tokens) AS cached_input_tokens,
               sum(COALESCE(row.estimated_cost, row.reported_cost)) AS cost,
               CASE WHEN count(DISTINCT COALESCE(row.currency, row.reported_currency, 'USD')) = 1
                    THEN min(COALESCE(row.currency, row.reported_currency, 'USD'))
                    ELSE 'USD' END AS currency,
               CASE WHEN count(*) = 0 THEN NULL
                    WHEN bool_or(row.confidence = 'estimated') THEN 'estimated'
                    ELSE 'exact' END AS confidence
        FROM run_usage row WHERE row.run_id = run.id
    ) usage ON true
    WHERE run_visible_to(run.id, $1)
      AND (conversation.id IS NULL OR conversation_visible_to(conversation.id, $1))
      AND (run.space_id IS NULL OR actor_can_access_project($1, NULL, run.space_id))
"#;

fn named(harnesses: &Harnesses, mut line: RunLine) -> RunLine {
    line.harness_name = line
        .harness
        .as_deref()
        .map(|harness| harnesses.name(harness));
    line
}

// ------------------------------------------------------------------- list

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunListQuery {
    pub organization_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// active, needs_you (waiting on an answer), not_responding, finished
    /// or failed; every run when left out.
    pub status: Option<String>,
    /// Only runs this agent does.
    pub agent_id: Option<Uuid>,
    /// Only runs on this host.
    pub host_id: Option<Uuid>,
    /// Only built-in runs on this harness.
    pub harness: Option<String>,
    /// Only runs this person asked for.
    pub mine: Option<bool>,
    /// The `nextCursor` of the previous page.
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunList {
    pub runs: Vec<RunLine>,
    pub next_cursor: Option<String>,
}

/// The run states a status filter keeps, and whether it wants runs that
/// stopped reporting.
fn status_filter(status: Option<&str>) -> Result<(Option<Vec<&'static str>>, bool), AppError> {
    Ok(match status {
        None | Some("" | "all") => (None, false),
        Some("active") => (Some(RunState::active_keys()), false),
        Some("needs_you") => (Some(vec![RunState::Waiting.as_str()]), false),
        Some("not_responding") => (Some(RunState::active_keys()), true),
        Some("finished") => (Some(vec![RunState::Done.as_str()]), false),
        Some("failed") => (
            Some(vec![
                RunState::Failed.as_str(),
                RunState::Cancelled.as_str(),
                RunState::Refused.as_str(),
                RunState::Interrupted.as_str(),
            ]),
            false,
        ),
        Some(_) => return Err(AppError::Validation("status")),
    })
}

fn decode_cursor(cursor: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let (time, id) = cursor
        .split_once('_')
        .ok_or(AppError::Validation("cursor"))?;
    let micros = time
        .parse::<i64>()
        .map_err(|_| AppError::Validation("cursor"))?;
    let time = DateTime::from_timestamp_micros(micros).ok_or(AppError::Validation("cursor"))?;
    let id = id.parse().map_err(|_| AppError::Validation("cursor"))?;
    Ok((time, id))
}

fn encode_cursor(run: &RunLine) -> String {
    format!("{}_{}", run.created_at.timestamp_micros(), run.id)
}

/// Runs the viewer can see, newest first, across their projects (or one
/// workspace's or project's).
pub async fn list(
    state: &AppState,
    viewer: Uuid,
    query: &RunListQuery,
) -> Result<RunList, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, MAX_LIST);
    let (states, not_responding) = status_filter(query.status.as_deref())?;
    let cursor = query.cursor.as_deref().map(decode_cursor).transpose()?;
    if query
        .harness
        .as_deref()
        .is_some_and(|harness| harness.len() > 64)
    {
        return Err(AppError::Validation("harness"));
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let rows = sqlx::query_as::<_, RunLine>(&format!(
        r#"{LINE_SELECT}
          AND ($2::uuid IS NULL OR run.workspace_id = $2)
          AND ($3::uuid IS NULL OR run.space_id = $3)
          AND ($4::text[] IS NULL OR run.state = ANY($4))
          AND (NOT $5 OR run.liveness = 'not_responding')
          AND ($6::uuid IS NULL OR run.account_id = $6)
          AND ($7::uuid IS NULL OR run.host_id = $7)
          AND ($8::text IS NULL OR detail.config->>'harness' = $8)
          AND (NOT $9 OR run.requested_by_account_id = $1)
          AND ($10::timestamptz IS NULL OR (run.created_at, run.id) < ($10, $11))
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT $12
        "#
    ))
    .bind(viewer)
    .bind(query.organization_id)
    .bind(query.project_id)
    .bind(states)
    .bind(not_responding)
    .bind(query.agent_id)
    .bind(query.host_id)
    .bind(query.harness.as_deref())
    .bind(query.mine.unwrap_or(false))
    .bind(cursor.map(|(time, _)| time))
    .bind(cursor.map(|(_, id)| id))
    .bind(limit + 1)
    .fetch_all(&mut *transaction)
    .await?;
    let harnesses = Harnesses::visible_on(state, &mut transaction).await?;
    drop(transaction);
    let more = rows.len() > usize::try_from(limit).unwrap_or(0);
    let mut runs = rows
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(0))
        .map(|run| named(&harnesses, run))
        .collect::<Vec<_>>();
    redact(state, viewer, &mut runs).await?;
    let next_cursor = more.then(|| runs.last().map(encode_cursor)).flatten();
    Ok(RunList { runs, next_cursor })
}

/// Leaves out what `viewer` may not see of each run in its project: without
/// "see tasks", the task's title; without "see code", the branch and pull
/// request (as `views::redact` does for the built-in run page).
async fn redact(state: &AppState, viewer: Uuid, runs: &mut [RunLine]) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    redact_on(&mut transaction, viewer, runs).await?;
    transaction.commit().await?;
    Ok(())
}

/// [`redact`] on a connection the caller holds, so a reader holding one
/// takes no second.
async fn redact_on(
    connection: &mut PgConnection,
    viewer: Uuid,
    runs: &mut [RunLine],
) -> Result<(), AppError> {
    let mut projects = runs
        .iter()
        .filter_map(|run| run.project_id)
        .collect::<Vec<_>>();
    projects.sort_unstable();
    projects.dedup();
    let (tasks, code) = if projects.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (
            crate::search::projects_allowing_on(
                &mut *connection,
                viewer,
                &projects,
                crate::policy::Permission::TasksView,
            )
            .await?,
            crate::search::projects_allowing_on(
                &mut *connection,
                viewer,
                &projects,
                crate::policy::Permission::CodeView,
            )
            .await?,
        )
    };
    for run in runs {
        let project = run.project_id;
        if !project.is_some_and(|project| tasks.contains(&project)) {
            run.task_title = None;
        }
        if !project.is_some_and(|project| code.contains(&project)) {
            run.branch = None;
            run.pull_request_number = None;
            run.pull_request_url = None;
            run.pull_request_state = None;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- view

/// One activity as the run view draws it.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLine {
    pub seq: i64,
    pub kind: String,
    pub step_ref: Option<String>,
    pub parent_ref: Option<String>,
    pub summary: String,
    pub payload: Value,
    pub occurred_at: DateTime<Utc>,
}

/// A request the run raised.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestLine {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// approval, question, input or proposal.
    pub kind: String,
    pub title: String,
    /// open, answered, expired, cancelled or superseded.
    pub status: String,
    #[schemars(with = "Option<String>")]
    pub conversation_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
}

/// Tokens and cost for one model.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageLine {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cost: Option<f64>,
    pub currency: Option<String>,
    /// exact or estimated.
    pub confidence: String,
}

/// A run with what it did: the run view.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    #[serde(flatten)]
    pub run: RunLine,
    /// Its latest plan: `[{ title, status }]`.
    pub plan: Option<Value>,
    /// Its activities, oldest first (the last `MAX_ACTIVITIES`).
    pub activities: Vec<ActivityLine>,
    /// Earlier activities not included; read them with run_activity_list.
    pub hidden_activities: i64,
    pub requests: Vec<RequestLine>,
    /// What it produced: links, pull requests, pages, files, commits.
    pub results: Vec<Value>,
    /// The pull requests it opened or reported that the space has, with
    /// their checks and reviews, newest first.
    pub pull_requests: Vec<crate::code_host::pull_requests::PullRequest>,
    pub usage_by_model: Vec<ModelUsageLine>,
    /// The files it changed in its repository, as its host last reported
    /// them (`run_changes_report`), for a viewer who sees code; null before
    /// the first report. The diff is read on its own.
    pub files: Option<Vec<super::changes::ChangedFile>>,
    /// The host couldn't report all of it (too many files, a diff too big
    /// to keep, or changes it couldn't list), so `files` may be missing
    /// some. False with no report.
    pub changes_truncated: bool,
}

pub(crate) async fn visible_line(
    connection: &mut PgConnection,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<Option<RunLine>, AppError> {
    Ok(
        sqlx::query_as::<_, RunLine>(&format!("{LINE_SELECT} AND run.id = $2"))
            .bind(viewer)
            .bind(run_id)
            .fetch_optional(connection)
            .await?,
    )
}

/// Activities after `after_seq`, oldest first.
async fn activities_after(
    connection: &mut PgConnection,
    run_id: Uuid,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<ActivityLine>, AppError> {
    Ok(sqlx::query_as::<_, ActivityLine>(
        r#"
        SELECT seq, kind, step_ref, parent_ref, summary, payload, occurred_at
        FROM run_activities
        WHERE run_id = $1 AND seq > $2
        ORDER BY seq
        LIMIT $3
        "#,
    )
    .bind(run_id)
    .bind(after_seq)
    .bind(limit.clamp(1, MAX_ACTIVITIES))
    .fetch_all(connection)
    .await?)
}

/// The run as `viewer` sees it, with what it did.
pub async fn view(
    state: &AppState,
    connection: &mut PgConnection,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<Option<RunView>, AppError> {
    let Some(line) = visible_line(connection, viewer, run_id).await? else {
        return Ok(None);
    };
    let total =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_activities WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(&mut *connection)
            .await?;
    let mut activities = sqlx::query_as::<_, ActivityLine>(
        r#"
        SELECT seq, kind, step_ref, parent_ref, summary, payload, occurred_at
        FROM run_activities
        WHERE run_id = $1
        ORDER BY seq DESC
        LIMIT $2
        "#,
    )
    .bind(run_id)
    .bind(MAX_ACTIVITIES)
    .fetch_all(&mut *connection)
    .await?;
    activities.reverse();
    let plan = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT payload->'items' FROM run_activities
        WHERE run_id = $1 AND kind = 'plan'
        ORDER BY seq DESC LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut *connection)
    .await?;
    let results = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT payload FROM run_activities
        WHERE run_id = $1 AND kind = 'result'
        ORDER BY seq
        LIMIT 100
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut *connection)
    .await?;
    let reported = results
        .iter()
        .filter(|result| result["kind"] == "pull_request")
        .filter_map(|result| result["url"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let pull_requests = crate::code_host::pull_requests::for_run(
        &mut *connection,
        viewer,
        run_id,
        line.project_id,
        &reported,
    )
    .await?;
    let requests = sqlx::query_as::<_, RequestLine>(
        r#"
        SELECT request.id, request.kind, request.title, request.status,
               request.conversation_id, request.created_at, request.answered_at
        FROM requests request
        WHERE request.run_id = $1
          AND (request.conversation_id IS NULL
               OR conversation_visible_to(request.conversation_id, $2))
        ORDER BY request.created_at, request.id
        LIMIT 100
        "#,
    )
    .bind(run_id)
    .bind(viewer)
    .fetch_all(&mut *connection)
    .await?;
    let usage_by_model = sqlx::query_as::<_, ModelUsageLine>(
        r#"
        SELECT model,
               sum(input_tokens)::bigint AS input_tokens,
               sum(output_tokens)::bigint AS output_tokens,
               sum(cached_input_tokens)::bigint AS cached_input_tokens,
               sum(COALESCE(estimated_cost, reported_cost)) AS cost,
               min(COALESCE(currency, reported_currency)) AS currency,
               CASE WHEN bool_or(confidence = 'estimated') THEN 'estimated' ELSE 'exact' END
                   AS confidence
        FROM run_usage WHERE run_id = $1
        GROUP BY model ORDER BY model
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut *connection)
    .await?;
    let harnesses = Harnesses::visible_on(state, &mut *connection).await?;
    let mut lines = vec![named(&harnesses, line)];
    redact_on(&mut *connection, viewer, &mut lines).await?;
    let run = lines.remove(0);
    let (files, changes_truncated) = if sees_code(&mut *connection, viewer, run.project_id).await? {
        super::changes::get(&mut *connection, run_id)
            .await?
            .map_or((None, false), |changes| {
                (Some(changes.files), changes.truncated)
            })
    } else {
        (None, false)
    };
    Ok(Some(RunView {
        hidden_activities: (total - i64::try_from(activities.len()).unwrap_or(0)).max(0),
        run,
        plan,
        activities,
        requests,
        results,
        pull_requests,
        usage_by_model,
        files,
        changes_truncated,
    }))
}

/// Whether `viewer` sees the code of runs in `project` ("see code"): their
/// branch, pull requests and changes.
pub(crate) async fn sees_code(
    connection: &mut PgConnection,
    viewer: Uuid,
    project: Option<Uuid>,
) -> Result<bool, AppError> {
    let Some(project) = project else {
        return Ok(false);
    };
    Ok(crate::search::projects_allowing_on(
        connection,
        viewer,
        &[project],
        crate::policy::Permission::CodeView,
    )
    .await?
    .contains(&project))
}

// ------------------------------------------------------------- operations

/// `run_detail_get`: one run with what it did, for the run page and for agents.
pub struct RunDetailGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunViewInput {
    /// The run: its id or link.
    pub run_id: IdRef,
}

impl Targeted for RunViewInput {
    fn target(&self) -> Target {
        Target::thing("run", self.run_id.id())
    }
}

impl Operation for RunDetailGet {
    const NAME: &'static str = "run_detail_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "View a run";
    const DESCRIPTION: &'static str = "One run as its page shows it, whoever runs it: state, \
        whether it is still reporting, what it waits on, its plan, its latest activities \
        (oldest first; run_activity_list reads the rest), the requests it raised, its results, \
        the pull requests it opened or reported with their checks, and its tokens and cost \
        per model, each exact or estimated.";

    type Input = RunViewInput;
    type Output = RunView;

    async fn run(call: &mut OpCall<'_>, input: RunViewInput) -> Result<RunView, OpError> {
        let viewer = call.account_id();
        let state = call.state.clone();
        let tx = call.tx().await?;
        view(&state, tx, viewer, input.run_id.id())
            .await?
            .ok_or_else(|| OpError::not_found("That run"))
    }
}

/// `run_activity_list`: a run's activities, a page at a time.
pub struct RunActivityList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunActivityListInput {
    /// The run: its id or link.
    pub run_id: IdRef,
    /// Only activities after this sequence number (0, or left out, from the
    /// start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<i64>,
    /// How many, oldest first (default 200, at most 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl Targeted for RunActivityListInput {
    fn target(&self) -> Target {
        Target::thing("run", self.run_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunActivityListOutput {
    pub activities: Vec<ActivityLine>,
    /// Pass it as afterSeq for the next page; none at the end.
    pub next_after_seq: Option<i64>,
}

impl Operation for RunActivityList {
    const NAME: &'static str = "run_activity_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List a run's activities";
    const DESCRIPTION: &'static str = "A run's activities in the order they happened: steps, \
        tool calls, messages, its plan, requests, results and gaps in its reports. Page with \
        afterSeq.";

    type Input = RunActivityListInput;
    type Output = RunActivityListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RunActivityListInput,
    ) -> Result<RunActivityListOutput, OpError> {
        let viewer = call.account_id();
        let run_id = input.run_id.id();
        let limit = i64::from(input.limit.unwrap_or(200)).clamp(1, MAX_ACTIVITIES);
        let tx = call.tx().await?;
        if visible_line(tx, viewer, run_id).await?.is_none() {
            return Err(OpError::not_found("That run"));
        }
        let activities = activities_after(tx, run_id, input.after_seq.unwrap_or(0), limit).await?;
        let next_after_seq = (i64::try_from(activities.len()).unwrap_or(0) == limit)
            .then(|| activities.last().map(|activity| activity.seq))
            .flatten();
        Ok(RunActivityListOutput {
            activities,
            next_after_seq,
        })
    }
}

// ----------------------------------------------------------- run_diff_get

/// `run_diff_get`: what a run changed, as a diff, for any run.
pub struct RunDiffGet;

impl Operation for RunDiffGet {
    const NAME: &'static str = "run_diff_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Read a run's diff";
    const DESCRIPTION: &'static str = "What a run changed against its base, as its host or agent \
        last reported it: the files and the unified diff (empty, and truncated, when it was too \
        big). unavailable when nothing was reported or you don't see the space's code.";

    type Input = RunViewInput;
    type Output = super::views::RunDiffView;

    async fn run(
        call: &mut OpCall<'_>,
        input: RunViewInput,
    ) -> Result<super::views::RunDiffView, OpError> {
        super::views::run_diff(call.state, call.account_id(), input.run_id.id())
            .await
            .map_err(|error| match error {
                AppError::NotFound(_) => OpError::not_found("That run"),
                error => OpError::from(error),
            })
    }
}

// ---------------------------------------------------------- run_line_list

/// `run_line_list`: the Runs page, every run whoever runs it.
pub struct RunLineList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunLineListInput {
    /// One space (its id or link); left out, every space you can open in
    /// this workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// active, needs_you (waiting on an answer), not_responding, finished
    /// or failed; every run when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Only runs this agent does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<IdRef>,
    /// Only runs on this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<IdRef>,
    /// Only built-in runs on this harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Only runs you asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mine: Option<bool>,
    /// The nextCursor of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// How many, newest first (default 50, at most 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl Targeted for RunLineListInput {
    fn target(&self) -> Target {
        match &self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

impl Operation for RunLineList {
    const NAME: &'static str = "run_line_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List all runs";
    const DESCRIPTION: &'static str = "Every run you can see in this workspace, newest first, \
        whoever runs it (this server's host, a linked host, or an agent reporting its own): \
        state, whether it is still reporting, its space, thread and task, agent, tokens and \
        cost. Filter by space, status, agent, host, harness or runs you asked for; page with \
        cursor. run_detail_get opens one.";

    type Input = RunLineListInput;
    type Output = RunList;

    async fn run(call: &mut OpCall<'_>, input: RunLineListInput) -> Result<RunList, OpError> {
        let query = RunListQuery {
            organization_id: Some(call.ctx.workspace_id),
            project_id: call.ctx.space_id.filter(|_| input.space_id.is_some()),
            status: input.status,
            agent_id: input.agent_id.map(|agent| agent.id()),
            host_id: input.host_id.map(|host| host.id()),
            harness: input.harness,
            mine: input.mine,
            cursor: input.cursor,
            limit: input.limit.map(i64::from),
        };
        list(call.state, call.account_id(), &query)
            .await
            .map_err(OpError::from)
    }
}

// ------------------------------------------------------- agent_usage_list

/// A1.15: each agent's (or person's) usage in the workspace, for the usage
/// page: from `run_usage`, so window reports split onto runs are counted
/// and marked estimated, and windows that found no run are shown apart.
pub struct AgentUsageList;

/// Days counted when the call names none.
const USAGE_DAYS: u16 = 30;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentUsageListInput {
    /// How many days back, today included (default 30, at most 400).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u16>,
}

impl Targeted for AgentUsageListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageLine {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub name: Option<String>,
    pub handle: Option<String>,
    /// Runs with usage in the period.
    pub runs: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cost: Option<f64>,
    pub currency: Option<String>,
    /// exact (every amount reported per run) or estimated (some split from a
    /// time window).
    pub confidence: String,
}

#[derive(Debug, Serialize, JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UnattributedUsageLine {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub name: Option<String>,
    /// Window reports that overlapped none of its runs.
    pub reports: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageListOutput {
    pub days: u16,
    /// Most tokens first.
    pub accounts: Vec<AgentUsageLine>,
    /// Usage reported for a time window that no run was going in ("not
    /// tied to a run").
    pub not_tied_to_a_run: Vec<UnattributedUsageLine>,
}

impl Operation for AgentUsageList {
    const NAME: &'static str = "agent_usage_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List usage by agent";
    const DESCRIPTION: &'static str = "Tokens and cost per agent (or person) in this workspace \
        over the last days, from the runs in projects you can open. Usage a host reported for \
        a time window is split across the runs going then and marked estimated; a window no \
        run was going in is listed as not tied to a run.";

    type Input = AgentUsageListInput;
    type Output = AgentUsageListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentUsageListInput,
    ) -> Result<AgentUsageListOutput, OpError> {
        let viewer = call.account_id();
        let workspace = call.ctx.workspace_id;
        let days = input.days.unwrap_or(USAGE_DAYS).clamp(1, 400);
        let tx = call.tx().await?;
        let accounts = sqlx::query_as::<_, AgentUsageLine>(
            r#"
            SELECT usage.account_id, named.display_name AS name, named.handle,
                   count(DISTINCT usage.run_id) AS runs,
                   sum(usage.input_tokens)::bigint AS input_tokens,
                   sum(usage.output_tokens)::bigint AS output_tokens,
                   sum(usage.cached_input_tokens)::bigint AS cached_input_tokens,
                   sum(COALESCE(usage.reported_cost, usage.estimated_cost)) AS cost,
                   min(COALESCE(usage.reported_currency, usage.currency)) AS currency,
                   CASE WHEN bool_or(usage.confidence = 'estimated') THEN 'estimated'
                        ELSE 'exact' END AS confidence
            FROM run_usage usage
            LEFT JOIN LATERAL (
                SELECT membership.display_name, membership.handle
                FROM project_memberships membership
                WHERE membership.project_id = usage.space_id
                  AND membership.actor_id = usage.account_id
                LIMIT 1
            ) named ON true
            -- visibility: only runs in projects the viewer can open count.
            WHERE usage.workspace_id = $1 AND usage.space_id IS NOT NULL
              AND actor_can_access_project($2, NULL, usage.space_id)
              AND usage.usage_date >= (now() AT TIME ZONE 'UTC')::date - ($3::int - 1)
            GROUP BY usage.account_id, named.display_name, named.handle
            ORDER BY sum(usage.input_tokens + usage.output_tokens) DESC, usage.account_id
            LIMIT 200
            "#,
        )
        .bind(workspace)
        .bind(viewer)
        .bind(i32::from(days))
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        let not_tied_to_a_run = sqlx::query_as::<_, UnattributedUsageLine>(
            r#"
            SELECT report.account_id, min(named.display_name) AS name, count(*) AS reports,
                   sum(report.input_tokens)::bigint AS input_tokens,
                   sum(report.output_tokens)::bigint AS output_tokens,
                   sum(report.reported_cost) AS cost,
                   min(report.reported_currency) AS currency
            FROM usage_reports report
            -- visibility: an account shown is in a project the viewer can open.
            JOIN LATERAL (
                SELECT membership.display_name
                FROM project_memberships membership
                JOIN projects project ON project.id = membership.project_id
                WHERE membership.actor_id = report.account_id AND membership.enabled
                  AND project.organization_id = $1
                  AND actor_can_access_project($2, NULL, project.id)
                LIMIT 1
            ) named ON true
            WHERE report.workspace_id = $1 AND report.run_id IS NULL
              AND report.attributed_at IS NULL
              AND report.window_end >= now() - make_interval(days => $3::int)
            GROUP BY report.account_id
            ORDER BY sum(report.input_tokens + report.output_tokens) DESC, report.account_id
            LIMIT 200
            "#,
        )
        .bind(workspace)
        .bind(viewer)
        .bind(i32::from(days))
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        Ok(AgentUsageListOutput {
            days,
            accounts,
            not_tied_to_a_run,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_filters_name_run_states() {
        let (active, silent) = status_filter(Some("active")).expect("active");
        assert_eq!(active, Some(RunState::active_keys()));
        assert!(!silent);
        let (_, silent) = status_filter(Some("not_responding")).expect("silent");
        assert!(silent);
        assert_eq!(
            status_filter(Some("needs_you")).expect("needs you").0,
            Some(vec!["waiting"])
        );
        assert!(status_filter(Some("later")).is_err());
        assert_eq!(status_filter(None).expect("all").0, None);
    }
}

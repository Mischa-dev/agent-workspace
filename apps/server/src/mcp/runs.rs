//! Codex runs for people (docs/plans/mcp-tools.md, Phase 4): start one in a
//! thread on your own Codex account, follow it, and see its pull request's
//! checks. Agents keep `run_request`, which asks a person to approve.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id, public_url, snippet},
};
use crate::{
    AppState,
    api::{StartRunRequest, interrupt_run_in, retry_run_in, start_run_in},
    error::AppError,
    idempotency::IdempotencyKey,
    policy::ActorKind,
    runs::RunCaller,
};

const NOT_FOR_AGENTS: &str = "Agents ask a person to start Codex with run_request; run_start spends a person's own Codex account.";

/// AWD-75: the projects the caller belongs to where `module` is off for
/// them, for lists that reach projects through membership rather than a
/// list of projects; they leave these out before counting or limiting.
pub(super) async fn projects_with_module_off(
    state: &AppState,
    actor_id: Uuid,
    module: &str,
) -> Result<Vec<Uuid>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let projects = sqlx::query_scalar::<_, Uuid>(
        "SELECT project_id FROM project_memberships WHERE actor_id = $1 AND enabled",
    )
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let on = crate::modules::state::ProjectModules::load(&mut connection, actor_id, &projects)
        .await?
        .keep(&projects, |set, _| set.contains(module));
    Ok(projects
        .into_iter()
        .filter(|project| !on.contains(project))
        .collect())
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunStartArguments {
    /// The thread the run works in. To work on a task: start a thread
    /// (thread_start), link it to the task (task_link), then start the run
    /// there, so the run's pull request is linked to the task.
    pub thread_id: String,
    /// What Codex should do, written out in full: Codex sees only this and
    /// the repository.
    pub prompt: String,
    /// The repository, if the project has more than one ready.
    pub repository_id: Option<String>,
    /// A stable ID so a retry never starts a second run.
    pub client_request_id: Option<String>,
    /// The harness to run on (for example `codex_app_server`), if the caller
    /// has accounts on more than one. Defaults to their preferred account.
    #[serde(default)]
    pub harness: Option<String>,
    /// A model from the harness's catalog. Defaults to the account's choice.
    #[serde(default)]
    pub model: Option<String>,
    /// A reasoning level the model offers.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// A speed the model offers.
    #[serde(default)]
    pub speed: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunGetArguments {
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunControlArguments {
    /// The run's ID.
    pub run_id: String,
    /// A stable ID so a retry of this call does the same thing once.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunListArguments {
    /// Only this project (ID, link or key).
    pub project_id: Option<String>,
    /// active (queued or working, the default), finished, failed, or all.
    pub state: Option<String>,
    /// Only runs you started.
    pub mine: Option<bool>,
    /// How many, newest first (default 20, at most 100).
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct PullRequestLine {
    pub number: i64,
    pub url: String,
    pub title: String,
    /// open, closed or merged.
    pub state: String,
    /// pending, passing, failing or unknown.
    pub checks: Option<String>,
    pub review: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunValue {
    pub id: String,
    /// queued, working, waiting_for_approval, finished, failed, interrupted,
    /// published, opening_pull_request, checks_pending, checks_passed, or
    /// checks_failed.
    pub stage: String,
    /// The run's own state, as the web app shows it.
    pub state: String,
    pub prompt: String,
    pub thread_id: String,
    pub project_id: String,
    pub started_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequestLine>,
    /// Approvals Codex is waiting on in this run.
    pub approvals_waiting: i64,
    /// What to do next, in words.
    pub next: String,
    /// When polling, wait this long before asking again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_after_seconds: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunListValue {
    pub runs: Vec<RunValue>,
}

#[derive(Debug, FromRow)]
struct RunRow {
    id: Uuid,
    state: String,
    prompt: String,
    conversation_id: Uuid,
    project_id: Uuid,
    started_by: String,
    queue_position: Option<i64>,
    branch: Option<String>,
    published: bool,
    failure_reason: Option<String>,
    pull_request_request_state: Option<String>,
    pr_number: Option<i64>,
    pr_url: Option<String>,
    pr_title: Option<String>,
    pr_state: Option<String>,
    pr_checks: Option<String>,
    pr_review: Option<String>,
    approvals_waiting: i64,
    created_at: String,
    updated_at: String,
    conversation_kind: String,
    parent_conversation_id: Option<Uuid>,
    parent_kind: Option<String>,
}

/// Runs the viewer can see: in a project they can reach, in a thread they can
/// see. `$1` is the viewer, `$2` their agent connection.
const RUN_SELECT: &str = r#"
    SELECT run.id, run.state, run.prompt, run.conversation_id, thread.project_id,
           COALESCE(starter.display_name, 'Someone') AS started_by,
           CASE WHEN run.state = 'queued' THEN (
               SELECT count(*) FROM agent_runs queued
               WHERE queued.state = 'queued'
                 AND (queued.created_at, queued.id) <= (run.created_at, run.id)
           ) END AS queue_position,
           workspace.branch_name AS branch,
           workspace.published_at IS NOT NULL AS published,
           run.failure_reason,
           request.state AS pull_request_request_state,
           pr.pull_request_number AS pr_number, pr.html_url AS pr_url, pr.title AS pr_title,
           pr.state AS pr_state, pr.checks_state AS pr_checks, pr.review_state AS pr_review,
           (SELECT count(*) FROM agent_approvals approval
             WHERE approval.agent_run_id = run.id AND approval.status = 'pending') AS approvals_waiting,
           run.created_at::text AS created_at, run.updated_at::text AS updated_at,
           thread.kind AS conversation_kind,
           thread.parent_channel_id AS parent_conversation_id,
           parent.kind AS parent_kind
    FROM agent_runs run
    JOIN conversations thread ON thread.id = run.conversation_id
    LEFT JOIN conversations parent ON parent.id = thread.parent_channel_id
    LEFT JOIN project_memberships starter
      ON starter.project_id = thread.project_id AND starter.actor_id = run.requesting_actor_id
    LEFT JOIN repository_workspaces workspace ON workspace.id = run.repository_workspace_id
    LEFT JOIN github_pull_request_requests request ON request.agent_run_id = run.id
    LEFT JOIN github_pull_requests pr ON pr.id = request.github_pull_request_id
    WHERE conversation_visible_to(thread.id, $1)
      AND actor_can_access_project($1, $2, thread.project_id)
"#;

fn stage(row: &RunRow) -> (&'static str, String, Option<u32>) {
    if let Some(checks) = row.pr_checks.as_deref().filter(|_| row.pr_number.is_some()) {
        return match checks {
            "passing" | "success" | "passed" => (
                "checks_passed",
                "The pull request's checks passed.".to_owned(),
                None,
            ),
            "failing" | "failure" | "failed" => (
                "checks_failed",
                "The pull request's checks failed; open it to see which.".to_owned(),
                None,
            ),
            _ => (
                "checks_pending",
                "Checks are running on the pull request; ask again shortly.".to_owned(),
                Some(60),
            ),
        };
    }
    if row.pr_number.is_some() {
        return (
            "checks_pending",
            "The pull request is open; its checks haven't reported yet.".to_owned(),
            Some(60),
        );
    }
    if row.pull_request_request_state.is_some() {
        return (
            "opening_pull_request",
            "The pull request is being opened.".to_owned(),
            Some(30),
        );
    }
    match row.state.as_str() {
        "created" | "queued" => (
            "queued",
            "Waiting for Codex to pick it up.".to_owned(),
            Some(60),
        ),
        "blocked" if row.approvals_waiting > 0 => (
            "waiting_for_approval",
            "Codex is waiting on an approval; answer it in the thread (approval_list shows it).".to_owned(),
            Some(60),
        ),
        "starting" | "working" | "blocked" | "recovering" => (
            "working",
            "Codex is working.".to_owned(),
            Some(60),
        ),
        "failed" => (
            "failed",
            "The run failed; see failure. Fix the cause and start a new run.".to_owned(),
            None,
        ),
        "interrupted" | "canceled" => (
            "interrupted",
            "The run was stopped.".to_owned(),
            None,
        ),
        _ if row.published => (
            "published",
            "Codex's branch is published. Open a pull request from the run in the web app."
                .to_owned(),
            None,
        ),
        _ => (
            "finished",
            "Codex finished. Publish its branch and open a pull request from the run in the web app."
                .to_owned(),
            None,
        ),
    }
}

fn run_value(state: &AppState, row: RunRow) -> RunValue {
    let url = public_url(state, &run_conversation_path(&row));
    let (stage, next, poll_after_seconds) = stage(&row);
    let pull_request = match (row.pr_number, row.pr_url.clone()) {
        (Some(number), Some(url)) => Some(PullRequestLine {
            number,
            url,
            title: row.pr_title.clone().unwrap_or_default(),
            state: row.pr_state.clone().unwrap_or_default(),
            checks: row.pr_checks.clone(),
            review: row.pr_review.clone(),
        }),
        _ => None,
    };
    RunValue {
        id: row.id.to_string(),
        stage: stage.to_owned(),
        state: row.state,
        prompt: snippet(&row.prompt, 600),
        thread_id: row.conversation_id.to_string(),
        project_id: row.project_id.to_string(),
        started_by: row.started_by,
        queue_position: row.queue_position,
        branch: row.branch,
        failure: row.failure_reason,
        pull_request,
        approvals_waiting: row.approvals_waiting,
        next,
        poll_after_seconds,
        created_at: row.created_at,
        updated_at: row.updated_at,
        url,
    }
}

/// Where a run's conversation opens: DMs, groups and their threads live under
/// Messages, everything else under its project.
fn run_conversation_path(row: &RunRow) -> String {
    match (
        row.conversation_kind.as_str(),
        row.parent_kind.as_deref(),
        row.parent_conversation_id,
    ) {
        ("dm" | "group", _, _) => format!("/messages/{}", row.conversation_id),
        ("thread", Some("dm" | "group"), Some(parent)) => {
            format!("/messages/{parent}?thread={}", row.conversation_id)
        }
        ("channel", _, _) => format!(
            "/projects/{}/channels/{}",
            row.project_id, row.conversation_id
        ),
        _ => format!(
            "/projects/{}/threads/{}",
            row.project_id, row.conversation_id
        ),
    }
}

pub(super) async fn load_run(
    state: &AppState,
    principal: &McpPrincipal,
    run_id: Uuid,
) -> Result<Option<RunValue>, AppError> {
    let row = sqlx::query_as::<_, RunRow>(&format!("{RUN_SELECT} AND run.id = $3"))
        .bind(principal.actor_id)
        .bind(principal.external_agent_connection_id)
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?;
    let rows =
        without_hidden_pull_requests(state, principal.actor_id, row.into_iter().collect()).await?;
    Ok(rows.into_iter().next().map(|row| run_value(state, row)))
}

/// Leaves out a run's code (its branch, and its pull request and request:
/// number, link, title, state, checks, review) where the viewer's role, or an
/// app's agent limit, leaves out "see code" in the run's project, as the
/// run pages do (run_views::redact). The rest of the run stays.
async fn without_hidden_pull_requests(
    state: &AppState,
    viewer: Uuid,
    mut rows: Vec<RunRow>,
) -> Result<Vec<RunRow>, AppError> {
    let mut projects = rows
        .iter()
        .filter(|row| {
            row.pr_number.is_some()
                || row.branch.is_some()
                || row.pull_request_request_state.is_some()
        })
        .map(|row| row.project_id)
        .collect::<Vec<_>>();
    projects.sort_unstable();
    projects.dedup();
    if projects.is_empty() {
        return Ok(rows);
    }
    let sees_code = crate::search::projects_allowing(
        &state.pool,
        viewer,
        &projects,
        crate::policy::Permission::CodeView,
    )
    .await?;
    for row in &mut rows {
        if !sees_code.contains(&row.project_id) {
            row.branch = None;
            row.pull_request_request_state = None;
            row.pr_number = None;
            row.pr_url = None;
            row.pr_title = None;
            row.pr_state = None;
            row.pr_checks = None;
            row.pr_review = None;
        }
    }
    Ok(rows)
}

pub(super) async fn run_start(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunStartArguments,
) -> ToolResult<RunValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(NOT_FOR_AGENTS);
    }
    let Some(thread_id) = parse_id(&arguments.thread_id) else {
        return ToolResult::invalid("threadId", "Pass the thread's ID or link.");
    };
    let repository_id = match arguments.repository_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("repositoryId", "Pass a repository ID."),
        },
        None => None,
    };
    let request_id = arguments
        .client_request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let Ok(key) = IdempotencyKey::parse(&request_id) else {
        return ToolResult::invalid("clientRequestId", "Use up to 128 plain characters.");
    };
    if arguments.prompt.trim().is_empty() {
        return ToolResult::invalid("prompt", "Say what Codex should do.");
    }
    let caller = RunCaller::mcp(principal, request_id);
    let result = async {
        let (_, started) = start_run_in(
            state,
            &caller,
            principal.via_grant_id(),
            thread_id,
            StartRunRequest {
                prompt: arguments.prompt.clone(),
                repository_id,
                config: Some(crate::provider::run_config::RunConfig {
                    harness: arguments.harness.clone(),
                    model: arguments.model.clone(),
                    reasoning: arguments.reasoning.clone(),
                    speed: arguments.speed.clone(),
                    ..Default::default()
                })
                .filter(|config| *config != Default::default()),
                provider_account_id: None,
                agent_profile_id: None,
            },
            Ok(key),
        )
        .await?;
        load_run(state, principal, started.run_id)
            .await?
            .map(ToolResult::ok)
            .ok_or(AppError::NotFound("agent_run"))
    }
    .await;
    result.unwrap_or_else(|error| match error {
        AppError::NotFound(_) => ToolResult::not_found("That thread"),
        AppError::Invalid(message) => ToolResult::invalid("threadId", message),
        AppError::Forbidden => ToolResult::denied(
            "You can't start Codex in that project (it needs a member or above).",
        ),
        AppError::Conflict(_, message) => ToolResult::conflict(message),
        AppError::RepositoryBindingConflict => ToolResult::conflict(
            "That thread already works in a different repository. Start a new thread for this one.",
        ),
        AppError::ExecutionProfileRequired => ToolResult::conflict(
            "You have no signed-in provider account for that harness in this project.",
        ),
        error => ToolResult::from_error(error, "run_start"),
    })
}

fn control_key(
    arguments: &RunControlArguments,
) -> Result<(Uuid, String, IdempotencyKey), Box<ToolResult<RunValue>>> {
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return Err(Box::new(ToolResult::invalid("runId", "Pass the run's ID.")));
    };
    let request_id = arguments
        .client_request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let Ok(key) = IdempotencyKey::parse(&request_id) else {
        return Err(Box::new(ToolResult::invalid(
            "clientRequestId",
            "Use up to 128 plain characters.",
        )));
    };
    Ok((run_id, request_id, key))
}

/// What a refused stop or retry says, the same for a missing run and one
/// the caller can't see.
fn control_error(error: AppError, tool: &'static str) -> ToolResult<RunValue> {
    match error {
        AppError::NotFound(_) => ToolResult::not_found("That run"),
        AppError::Forbidden => ToolResult::denied(if tool == "run_stop" {
            "You can't stop runs in that project."
        } else {
            "You can't start runs in that project (it needs a member or above)."
        }),
        AppError::ProjectArchived => ToolResult::conflict("That project is archived."),
        AppError::Conflict(_, message) => ToolResult::conflict(message),
        AppError::RepositoryBindingConflict => ToolResult::conflict(
            "That thread now works in a different repository. Start a new run instead.",
        ),
        AppError::ExecutionProfileRequired => ToolResult::conflict(
            "You have no signed-in provider account for that harness in this project.",
        ),
        error => ToolResult::from_error(error, tool),
    }
}

/// Stop a run that's queued, working or waiting for an answer, as the run
/// page's Stop button does. Its workspace is kept and its open questions
/// are withdrawn.
pub(super) async fn run_stop(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunControlArguments,
) -> ToolResult<RunValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let (run_id, request_id, key) = match control_key(&arguments) {
        Ok(parts) => parts,
        Err(refused) => return *refused,
    };
    let caller = RunCaller::mcp(principal, request_id);
    let result = async {
        interrupt_run_in(state, &caller, run_id, Ok(key)).await?;
        load_run(state, principal, run_id)
            .await?
            .ok_or(AppError::NotFound("agent_run"))
    }
    .await;
    match result {
        Ok(run) => ToolResult::ok(run),
        Err(error) => control_error(error, "run_stop"),
    }
}

/// Start a finished, failed or stopped run again: a new run in the same
/// thread with the same prompt, repository and settings. People only, as
/// it spends their own account.
pub(super) async fn run_retry(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunControlArguments,
) -> ToolResult<RunValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(NOT_FOR_AGENTS);
    }
    let (run_id, request_id, key) = match control_key(&arguments) {
        Ok(parts) => parts,
        Err(refused) => return *refused,
    };
    let caller = RunCaller::mcp(principal, request_id);
    let result = async {
        let (_, started) = retry_run_in(state, &caller, run_id, Ok(key)).await?;
        load_run(state, principal, started.run_id)
            .await?
            .ok_or(AppError::NotFound("agent_run"))
    }
    .await;
    match result {
        Ok(run) => ToolResult::ok(run),
        Err(error) => control_error(error, "run_retry"),
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunDismissFailureArguments {
    /// The failed run's ID.
    pub run_id: String,
}

/// Dismiss a failed run's notice for the caller, as the notice's Dismiss
/// button does: it stops showing as needing attention for them. Read state
/// only, like mark_read: the run and its failure stay.
pub(super) async fn run_dismiss_failure(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunDismissFailureArguments,
) -> ToolResult<RunValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Pass the run's ID.");
    };
    let result = async {
        // Only a run the caller can see: its thread is visible to them and
        // its project open to this connection.
        if load_run(state, principal, run_id).await?.is_none() {
            return Err(AppError::NotFound("agent_run"));
        }
        let mut transaction = state.pool.begin().await?;
        super::common::mark_via(&mut transaction, principal).await?;
        let failure = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
            r#"
            SELECT event.id, run.project_id, run.conversation_id
              FROM agent_runs run
              JOIN agent_run_events event ON event.agent_run_id = run.id
              JOIN conversation_entries entry
                ON entry.agent_run_event_id = event.id
               AND entry.conversation_id = run.conversation_id
             WHERE run.id = $1
               AND run.state = 'failed'
               AND event.kind = 'run_failed'
             ORDER BY event.id DESC
             LIMIT 1
            "#,
        )
        .bind(run_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((event_id, project_id, conversation_id)) = failure else {
            return Err(AppError::Conflict(
                "agent_run",
                "That run hasn't failed, so there's no notice to dismiss.",
            ));
        };
        let inserted = sqlx::query(
            r#"
            INSERT INTO agent_run_acknowledgements (
                id, project_id, agent_run_id, actor_id, acknowledged_event_id
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (agent_run_id, actor_id) DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(run_id)
        .bind(principal.actor_id)
        .bind(event_id)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() == 1 {
            crate::outbox::enqueue_agent_run_updated(
                &mut transaction,
                project_id,
                conversation_id,
                run_id,
            )
            .await?;
        }
        transaction.commit().await?;
        load_run(state, principal, run_id)
            .await?
            .ok_or(AppError::NotFound("agent_run"))
    }
    .await;
    match result {
        Ok(run) => ToolResult::ok(run),
        Err(error) => control_error(error, "run_dismiss_failure"),
    }
}

pub(super) async fn run_get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunGetArguments,
) -> ToolResult<RunValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Pass the run's ID.");
    };
    match load_run(state, principal, run_id).await {
        Ok(Some(run)) => ToolResult::ok(run),
        Ok(None) => ToolResult::not_found("That run"),
        Err(error) => ToolResult::from_error(error, "run_get"),
    }
}

pub(super) async fn run_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunListArguments,
) -> ToolResult<RunListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let project = match arguments.project_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::not_found("That project"),
        },
        None => None,
    };
    let states: &[&str] = match arguments.state.as_deref().unwrap_or("active") {
        "active" => &crate::runs::ACTIVE_RUN_STATES,
        "finished" => &["completed"],
        "failed" => &["failed"],
        "all" => &[],
        _ => {
            return ToolResult::invalid("state", "Use active, finished, failed, or all.");
        }
    };
    let limit = i64::from(arguments.limit.unwrap_or(20).clamp(1, 100));
    let off = match projects_with_module_off(state, principal.actor_id, "runs").await {
        Ok(off) => off,
        Err(error) => return ToolResult::from_error(error, "run_list"),
    };
    let result = sqlx::query_as::<_, RunRow>(&format!(
        r#"{RUN_SELECT}
          AND ($3::uuid IS NULL OR thread.project_id = $3)
          AND NOT (thread.project_id = ANY($7))
          AND (cardinality($4::text[]) = 0 OR run.state = ANY($4))
          AND (NOT $5 OR run.requesting_actor_id = $1)
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT $6"#
    ))
    .bind(principal.actor_id)
    .bind(principal.external_agent_connection_id)
    .bind(project)
    .bind(states)
    .bind(arguments.mine.unwrap_or(false))
    .bind(limit)
    .bind(&off)
    .fetch_all(&state.pool)
    .await;
    let rows = match result {
        Ok(rows) => rows,
        Err(error) => return ToolResult::from_error(error.into(), "run_list"),
    };
    match without_hidden_pull_requests(state, principal.actor_id, rows).await {
        Ok(rows) => ToolResult::ok(RunListValue {
            runs: rows.into_iter().map(|row| run_value(state, row)).collect(),
        }),
        Err(error) => ToolResult::from_error(error, "run_list"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: &str, parent: Option<(Uuid, &str)>) -> RunRow {
        RunRow {
            id: Uuid::nil(),
            state: "running".to_owned(),
            prompt: String::new(),
            conversation_id: Uuid::from_u128(2),
            project_id: Uuid::from_u128(1),
            started_by: String::new(),
            queue_position: None,
            branch: None,
            published: false,
            failure_reason: None,
            pull_request_request_state: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            pr_checks: None,
            pr_review: None,
            approvals_waiting: 0,
            created_at: String::new(),
            updated_at: String::new(),
            conversation_kind: kind.to_owned(),
            parent_conversation_id: parent.map(|(id, _)| id),
            parent_kind: parent.map(|(_, kind)| kind.to_owned()),
        }
    }

    #[test]
    fn runs_in_messages_link_to_messages() {
        let project = Uuid::from_u128(1);
        let conversation = Uuid::from_u128(2);
        let parent = Uuid::from_u128(3);
        assert_eq!(
            run_conversation_path(&row("thread", Some((parent, "channel")))),
            format!("/projects/{project}/threads/{conversation}")
        );
        assert_eq!(
            run_conversation_path(&row("thread", Some((parent, "dm")))),
            format!("/messages/{parent}?thread={conversation}")
        );
        assert_eq!(
            run_conversation_path(&row("thread", Some((parent, "group")))),
            format!("/messages/{parent}?thread={conversation}")
        );
        assert_eq!(
            run_conversation_path(&row("dm", None)),
            format!("/messages/{conversation}")
        );
        assert_eq!(
            run_conversation_path(&row("channel", None)),
            format!("/projects/{project}/channels/{conversation}")
        );
    }
}

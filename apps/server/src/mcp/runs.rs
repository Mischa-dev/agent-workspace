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
    api::{StartRunRequest, start_run_in},
    error::AppError,
    idempotency::IdempotencyKey,
    policy::ActorKind,
    runs::RunCaller,
};

const NOT_FOR_AGENTS: &str = "Agents ask a person to start Codex with run_request; run_start spends a person's own Codex account.";

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
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunGetArguments {
    pub run_id: String,
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
           run.created_at::text AS created_at, run.updated_at::text AS updated_at
    FROM agent_runs run
    JOIN conversations thread ON thread.id = run.conversation_id
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
        url: public_url(
            state,
            &format!(
                "/projects/{}/threads/{}",
                row.project_id, row.conversation_id
            ),
        ),
    }
}

async fn load_run(
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
    Ok(row.map(|row| run_value(state, row)))
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
        error => ToolResult::from_error(error, "run_start"),
    })
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
    let result = sqlx::query_as::<_, RunRow>(&format!(
        r#"{RUN_SELECT}
          AND ($3::uuid IS NULL OR thread.project_id = $3)
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
    .fetch_all(&state.pool)
    .await;
    match result {
        Ok(rows) => ToolResult::ok(RunListValue {
            runs: rows.into_iter().map(|row| run_value(state, row)).collect(),
        }),
        Err(error) => ToolResult::from_error(error.into(), "run_list"),
    }
}

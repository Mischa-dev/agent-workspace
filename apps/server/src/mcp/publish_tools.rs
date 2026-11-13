//! AWD-111: a finished run's work sent to GitHub through MCP, as the run
//! page's buttons do: run_publish pushes its branch, pull_request_create
//! opens a pull request from it, and pull_request_refresh reads a pull
//! request's checks and review again. Each queues a job the GitHub worker
//! runs; everything goes through `crate::github`, for whoever the project's
//! roles let do it.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, accessible_project_ids, insert_audit, mark_via, parse_id},
    runs::load_run,
    tasks::resolve_tasks,
};
use crate::{AppState, error::AppError, github, policy::Action};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunPublishArguments {
    /// The finished run whose branch to push to GitHub.
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PullRequestCreateArguments {
    /// The finished run whose published branch to open it from
    /// (run_publish first).
    pub run_id: String,
    /// Up to 256 characters, on one line.
    pub title: String,
    /// Up to 10,000 characters.
    #[serde(default)]
    pub body: Option<String>,
    /// Tasks it delivers, by key (such as AW-6) or ID, up to 20: they're
    /// linked to it, and its automation moves them.
    #[serde(default)]
    pub task_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PullRequestRefreshArguments {
    /// The pull request (pull_request_list, pull_request_get).
    pub pull_request_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueuedValue {
    /// true when this call queued it; false when it was already queued or
    /// done, and this is that one.
    pub queued: bool,
    /// The job: its state (queued, running, failed or completed), and what
    /// it works on. run_get and pull_request_get show the result.
    #[schemars(with = "Map<String, Value>")]
    pub job: Value,
}

/// Refused where GitHub isn't set up on this server: checked after the run
/// or pull request, so one the caller can't see is "not found" either way.
fn not_configured(state: &AppState) -> Result<(), AppError> {
    if state.config.github.is_none() {
        return Err(AppError::Conflict("github_integration", "not_configured"));
    }
    Ok(())
}

fn job_value<T: Serialize>(view: &T) -> Result<Value, AppError> {
    serde_json::to_value(view).map_err(|_| AppError::Internal("serialize GitHub job"))
}

/// The run's project, when the caller can see the run.
async fn run_project(
    state: &AppState,
    principal: &McpPrincipal,
    run_id: Uuid,
) -> Result<Uuid, AppError> {
    load_run(state, principal, run_id)
        .await?
        .and_then(|run| parse_id(&run.project_id))
        .ok_or(AppError::NotFound("agent_run"))
}

pub(super) async fn run_publish(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunPublishArguments,
) -> ToolResult<QueuedValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Pass the run's ID.");
    };
    let result = async {
        let project_id = run_project(state, principal, run_id).await?;
        not_configured(state)?;
        let mut transaction = state.pool.begin().await?;
        github::require_project_access_as(
            &mut transaction,
            principal.actor_id,
            principal.actor_kind,
            project_id,
            Action::RepositoryPublish,
        )
        .await?;
        let queued = github::queue_publish_in(
            state,
            &mut transaction,
            principal.actor_id,
            project_id,
            run_id,
        )
        .await?;
        if queued.new {
            mark_via(&mut transaction, principal).await?;
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                "github.workspace_publish_queued",
                "github_job",
                queued.view.job_id,
                serde_json::json!({
                    "agent_run_id": queued.view.agent_run_id,
                    "repository_workspace_id": queued.view.repository_workspace_id,
                }),
            )
            .await?;
        }
        transaction.commit().await?;
        if queued.new {
            github::kick_jobs(state.clone());
        }
        Ok(QueuedValue {
            queued: queued.new,
            job: job_value(&queued.view)?,
        })
    }
    .await;
    outcome(result, "run_publish")
}

pub(super) async fn pull_request_create(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PullRequestCreateArguments,
) -> ToolResult<QueuedValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Pass the run's ID.");
    };
    let body = arguments.body.unwrap_or_default();
    let references = arguments.task_ids.unwrap_or_default();
    if references.len() > 20 {
        return ToolResult::invalid("taskIds", "Name up to 20 tasks.");
    }
    let result = async {
        let project_id = run_project(state, principal, run_id).await?;
        not_configured(state)?;
        let mut work_item_ids = Vec::with_capacity(references.len());
        for found in resolve_tasks(state, principal, &references).await? {
            match found {
                Some((task, project)) if project == project_id => work_item_ids.push(task),
                _ => return Err(AppError::NotFound("work_item")),
            }
        }
        work_item_ids.dedup();
        let title = github::checked_pull_request_input(&arguments.title, &body, &work_item_ids)?;
        let mut transaction = state.pool.begin().await?;
        github::require_project_access_as(
            &mut transaction,
            principal.actor_id,
            principal.actor_kind,
            project_id,
            Action::PullRequestCreate,
        )
        .await?;
        let queued = github::queue_pull_request_in(
            &mut transaction,
            principal.actor_id,
            project_id,
            run_id,
            &title,
            &body,
            &work_item_ids,
        )
        .await?;
        if queued.new {
            mark_via(&mut transaction, principal).await?;
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                "github.pull_request_queued",
                "github_pull_request_request",
                queued.view.request_id,
                serde_json::json!({
                    "agent_run_id": run_id,
                    "repository_workspace_id": queued.view.repository_workspace_id,
                }),
            )
            .await?;
        }
        transaction.commit().await?;
        if queued.new {
            github::kick_jobs(state.clone());
        }
        Ok(QueuedValue {
            queued: queued.new,
            job: job_value(&queued.view)?,
        })
    }
    .await;
    outcome(result, "pull_request_create")
}

pub(super) async fn pull_request_refresh(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PullRequestRefreshArguments,
) -> ToolResult<QueuedValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(pull_request_id) = parse_id(&arguments.pull_request_id) else {
        return ToolResult::invalid("pullRequestId", "Pass the pull request's ID.");
    };
    let result = async {
        let projects = accessible_project_ids(state, principal).await?;
        let project_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT project_id FROM github_pull_requests WHERE id = $1 AND project_id = ANY($2)",
        )
        .bind(pull_request_id)
        .bind(&projects)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound("github_pull_request"))?;
        not_configured(state)?;
        let mut transaction = state.pool.begin().await?;
        github::require_project_access_as(
            &mut transaction,
            principal.actor_id,
            principal.actor_kind,
            project_id,
            Action::PullRequestRefresh,
        )
        .await?;
        let queued = github::queue_refresh_in(
            &mut transaction,
            principal.actor_id,
            project_id,
            pull_request_id,
        )
        .await?;
        if queued.new {
            mark_via(&mut transaction, principal).await?;
        }
        transaction.commit().await?;
        if queued.new {
            github::kick_jobs(state.clone());
        }
        Ok(QueuedValue {
            queued: queued.new,
            job: job_value(&queued.view)?,
        })
    }
    .await;
    outcome(result, "pull_request_refresh")
}

fn outcome(result: Result<QueuedValue, AppError>, tool: &'static str) -> ToolResult<QueuedValue> {
    match result {
        Ok(value) => ToolResult::ok(value),
        Err(AppError::NotFound("agent_run")) => ToolResult::not_found("That run"),
        Err(AppError::NotFound("github_pull_request")) => {
            ToolResult::not_found("That pull request")
        }
        Err(AppError::NotFound("work_item")) => {
            ToolResult::not_found("One of those tasks in the run's project")
        }
        Err(AppError::NotFound("project")) => ToolResult::not_found("That run's project"),
        Err(AppError::Forbidden) => {
            ToolResult::denied("Your role in that project doesn't let you send code to GitHub.")
        }
        Err(AppError::Invalid("pull request input")) => ToolResult::invalid(
            "title",
            "A title is 1 to 256 characters on one line, a body up to 10,000, with up to 20 different tasks.",
        ),
        Err(AppError::Conflict(what, why)) => ToolResult::conflict(match (what, why) {
            ("github_integration", "not_configured") => "GitHub isn't set up on this server yet.",
            ("agent_run", "not_terminal") => "That run hasn't finished yet.",
            ("agent_run", "branch_not_published") => {
                "That run's branch isn't published yet (run_publish, then wait for it), or the run isn't finished, or its repository can't reach GitHub."
            }
            ("repository_workspace", "active_run") => {
                "Another run is working in that run's workspace. Wait for it to finish."
            }
            ("github_installation", "repository_unavailable") => {
                "The project's GitHub installation doesn't reach that repository (github_integration_write)."
            }
            ("repository_workspace", _) => "That run's workspace isn't where it should be.",
            _ => why,
        }),
        Err(error) => ToolResult::from_error(error, tool),
    }
}

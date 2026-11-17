//! Runs through MCP (docs/plans/mcp-tools.md, Phase 4): start one in a
//! thread on your own provider account, follow any run you can see (on the
//! built-in host or reported by its own agent, read through
//! `runs::brief`), and see its pull request's checks. Agents keep
//! `run_request`, which asks a person to approve.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
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
    runs::{RunCaller, brief},
};

const NOT_FOR_AGENTS: &str = "Agents ask a person to start a run with run_request; run_start spends a person's own provider account.";

/// AWD-75: the projects the caller belongs to where `module` is off for
/// them, for lists that reach projects through membership rather than a
/// list of projects; they leave these out before counting or limiting.
pub(super) async fn projects_with_module_off(
    state: &AppState,
    actor_id: Uuid,
    module: &str,
) -> Result<Vec<Uuid>, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
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
    /// What the run should do, written out in full: it sees only this and
    /// the repository.
    pub prompt: String,
    /// The repository, if the project has more than one ready.
    pub repository_id: Option<String>,
    /// A stable ID so a retry never starts a second run.
    pub client_request_id: Option<String>,
    /// The harness to run on (its key, as the caller's provider accounts
    /// list it), if the caller has accounts on more than one. Defaults to their preferred account.
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

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
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
    /// queued, working, not_responding, waiting_for_approval,
    /// waiting_for_answer, stopping, finished, failed, interrupted,
    /// published, opening_pull_request, checks_pending, checks_passed, or
    /// checks_failed.
    pub stage: String,
    /// The run's state: queued, starting, start_uncertain, accepted,
    /// working, waiting, cancel_requested, done, failed, cancelled, refused
    /// or interrupted.
    pub state: String,
    /// Whether whoever does the run is still reporting: ok, quiet or
    /// not_responding.
    pub liveness: String,
    /// What the run was asked to do (its prompt, or the title its agent
    /// gave it).
    pub prompt: String,
    /// What the run last said it was doing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub started_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequestLine>,
    /// Approvals or other requests this run is waiting on.
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

fn run_value(state: &AppState, row: brief::BriefRow) -> RunValue {
    let url = public_url(state, &run_path(&row));
    let brief::Stage {
        key: stage,
        next,
        poll_after_seconds,
    } = brief::stage(&row);
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
        liveness: row.liveness,
        prompt: snippet(&row.prompt, 600),
        status_line: row.status_line,
        thread_id: row.conversation_id.map(|id| id.to_string()),
        project_id: row.project_id.map(|id| id.to_string()),
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

/// Where a run opens: its conversation (DMs, groups and their threads live
/// under Messages, everything else under its project), or the run page for
/// a run in no conversation.
fn run_path(row: &brief::BriefRow) -> String {
    let (Some(conversation_id), Some(project_id)) = (row.conversation_id, row.project_id) else {
        return format!("/runs/{}", row.id);
    };
    match (
        row.conversation_kind.as_deref().unwrap_or("thread"),
        row.parent_kind.as_deref(),
        row.parent_conversation_id,
    ) {
        ("dm" | "group", _, _) => format!("/messages/{conversation_id}"),
        ("thread", Some("dm" | "group"), Some(parent)) => {
            format!("/messages/{parent}?thread={conversation_id}")
        }
        ("channel", _, _) => format!("/projects/{project_id}/channels/{conversation_id}"),
        _ => format!("/projects/{project_id}/threads/{conversation_id}"),
    }
}

pub(super) async fn load_run(
    state: &AppState,
    principal: &McpPrincipal,
    run_id: Uuid,
) -> Result<Option<RunValue>, AppError> {
    let row = crate::db::scoped_one(&state.pool, async |connection| {
        brief::get(
            connection,
            principal.actor_id,
            principal.external_agent_connection_id,
            run_id,
        )
        .await
    })
    .await?;
    let rows =
        without_hidden_pull_requests(state, principal.actor_id, row.into_iter().collect()).await?;
    Ok(rows.into_iter().next().map(|row| run_value(state, row)))
}

/// Leaves out a run's code (its branch, and its pull request and request:
/// number, link, title, state, checks, review) where the viewer's role, or an
/// app's agent limit, leaves out "see code" in the run's project, as the
/// run pages do (runs::views::redact). The rest of the run stays.
async fn without_hidden_pull_requests(
    state: &AppState,
    viewer: Uuid,
    mut rows: Vec<brief::BriefRow>,
) -> Result<Vec<brief::BriefRow>, AppError> {
    let mut projects = rows
        .iter()
        .filter(|row| {
            row.pr_number.is_some()
                || row.branch.is_some()
                || row.pull_request_request_state.is_some()
        })
        .filter_map(|row| row.project_id)
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
        if !row
            .project_id
            .is_some_and(|project| sees_code.contains(&project))
        {
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
        return ToolResult::invalid("prompt", "Say what the run should do.");
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
            "You can't start a run in that project (it needs a member or above).",
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
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        super::common::mark_via(&mut transaction, principal).await?;
        let failure = brief::failure_notice(&mut transaction, run_id).await?;
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
    let Some(states) = brief::states_named(arguments.state.as_deref().unwrap_or("active")) else {
        return ToolResult::invalid("state", "Use active, finished, failed, or all.");
    };
    let limit = i64::from(arguments.limit.unwrap_or(20).clamp(1, 100));
    let off = match projects_with_module_off(state, principal.actor_id, "runs").await {
        Ok(off) => off,
        Err(error) => return ToolResult::from_error(error, "run_list"),
    };
    let filter = brief::ListFilter {
        project_id: project,
        states,
        mine: arguments.mine.unwrap_or(false),
        without_projects: off,
        limit,
    };
    let result = crate::db::scoped_one(&state.pool, async |connection| {
        brief::list(
            connection,
            principal.actor_id,
            principal.external_agent_connection_id,
            &filter,
        )
        .await
    })
    .await;
    let rows = match result {
        Ok(rows) => rows,
        Err(error) => return ToolResult::from_error(error, "run_list"),
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

    fn row(kind: &str, parent: Option<(Uuid, &str)>) -> brief::BriefRow {
        brief::BriefRow {
            id: Uuid::nil(),
            state: "working".to_owned(),
            detail_state: None,
            liveness: "ok".to_owned(),
            prompt: String::new(),
            status_line: None,
            outcome_summary: None,
            conversation_id: Some(Uuid::from_u128(2)),
            project_id: Some(Uuid::from_u128(1)),
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
            conversation_kind: Some(kind.to_owned()),
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
            run_path(&row("thread", Some((parent, "channel")))),
            format!("/projects/{project}/threads/{conversation}")
        );
        assert_eq!(
            run_path(&row("thread", Some((parent, "dm")))),
            format!("/messages/{parent}?thread={conversation}")
        );
        assert_eq!(
            run_path(&row("thread", Some((parent, "group")))),
            format!("/messages/{parent}?thread={conversation}")
        );
        assert_eq!(
            run_path(&row("dm", None)),
            format!("/messages/{conversation}")
        );
        assert_eq!(
            run_path(&row("channel", None)),
            format!("/projects/{project}/channels/{conversation}")
        );
        let mut alone = row("thread", None);
        alone.conversation_id = None;
        assert_eq!(run_path(&alone), format!("/runs/{}", Uuid::nil()));
    }
}

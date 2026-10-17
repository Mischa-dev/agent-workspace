//! Launch through MCP (C4, C5, C1, A3): `task_launch` starts or queues runs
//! for tasks with the project's defaults, `launch_list` shows the queue,
//! `environment_list` where runs execute, and `session_bind` lets an agent's
//! runner record its own session in a conversation so the thread links to it
//! and later wakeups name it.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, conversation_path, parse_id, project_is_chosen, public_url},
    tasks::resolve_task,
};
use crate::{
    AppState,
    error::AppError,
    idempotency::IdempotencyKey,
    launch::{self, LaunchCaller, LaunchOverrides, LaunchRequest, LaunchView, StartWhen},
    policy::ActorKind,
};

const NOT_FOR_AGENTS: &str = "Agents ask a person to launch with run_request; task_launch spends a person's own provider account.";
const CHOOSE_PROJECT: &str = "Name the project (projectId: an ID, link or key such as \"AWD\").";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskLaunchArguments {
    /// The tasks to launch: IDs, links or keys such as "AWD-12", all in one
    /// project. One task starts at once; several go through the launch
    /// queue, which starts them as account slots free up, blocked ones last.
    pub task_ids: Vec<String>,
    /// The environment to run in (environment_list). Defaults to the
    /// project's.
    #[serde(default)]
    pub environment_id: Option<String>,
    /// A harness such as `codex_cloud`, if not the environment's.
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    /// How many attempts, where the harness can make several.
    #[serde(default)]
    pub attempts: Option<u8>,
    /// The prompt, for a single task. Left out, the project's launch
    /// template builds it from the task.
    #[serde(default)]
    pub prompt: Option<String>,
    /// now (the default), at (with startAt), window_reset (when your
    /// account's usage window resets) or blockers_done.
    #[serde(default)]
    pub start_when: Option<String>,
    /// When to start, for startWhen "at" (RFC 3339).
    #[serde(default)]
    pub start_at: Option<String>,
    /// A stable ID so a retry never launches twice.
    #[serde(default)]
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project (ID, link or key such as "AWD").
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LaunchValue {
    pub id: String,
    /// The task's key, such as AWD-12.
    pub task: Option<String>,
    pub task_title: Option<String>,
    /// queued, started, cancelled or failed.
    pub state: String,
    /// Why it waits, or why it was cancelled or failed.
    pub status_reason: Option<String>,
    pub start_when: String,
    pub start_at: Option<String>,
    pub environment: Option<String>,
    pub harness: Option<String>,
    pub requested_by: String,
    /// The run it started; follow it with run_get.
    pub run_id: Option<String>,
    pub run_state: Option<String>,
    pub thread_url: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskLaunchValue {
    pub launches: Vec<LaunchValue>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LaunchListValue {
    /// Waiting to start, in queue order.
    pub queued: Vec<LaunchValue>,
    /// The latest that started, failed or were cancelled.
    pub recent: Vec<LaunchValue>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct EnvironmentValue {
    pub id: String,
    pub name: String,
    pub harness: String,
    pub harness_name: String,
    /// local_host or cloud.
    pub runs_on: Option<String>,
    pub provider_environment_id: Option<String>,
    pub repository_id: Option<String>,
    pub branch: Option<String>,
    pub setup_notes: String,
    pub is_default: bool,
    pub last_check_ok: Option<bool>,
    pub last_check_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct EnvironmentListValue {
    pub environments: Vec<EnvironmentValue>,
    pub default_environment_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SessionBindArguments {
    /// The thread or conversation you were asked to reply in.
    pub conversation_id: String,
    /// Your runner's own reference for the session, such as a Claude Code
    /// session ID or a Codex thread ID; wakeups name it as sessionRef.
    pub external_ref: String,
    /// What runs it, such as claude_code or codex.
    pub harness: String,
    /// An https page where people can watch the session.
    #[serde(default)]
    pub url: Option<String>,
    /// active (the default) while you work, idle when you stop.
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionBindValue {
    pub session_id: String,
    pub conversation_id: String,
    pub external_ref: String,
    pub state: String,
}

fn launch_value(state: &AppState, project_id: Uuid, view: LaunchView) -> LaunchValue {
    LaunchValue {
        id: view.id.to_string(),
        task: view.work_item.as_ref().map(|task| task.key.clone()),
        task_title: view.work_item.map(|task| task.title),
        state: view.state,
        status_reason: view.status_reason,
        start_when: view.start_when,
        start_at: view.start_at.map(|at| at.to_rfc3339()),
        environment: view.environment_name,
        harness: view.harness_name,
        requested_by: view.requested_by_name,
        run_id: view.agent_run_id.map(|id| id.to_string()),
        run_state: view.run_state,
        thread_url: view
            .conversation_id
            .map(|id| public_url(state, &conversation_path(project_id, "thread", id, false))),
        created_at: view.created_at.to_rfc3339(),
    }
}

fn caller(principal: &McpPrincipal, request_id: Option<String>) -> LaunchCaller {
    LaunchCaller {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind,
        via_grant_id: principal.via_grant_id(),
        mcp: true,
        session_id: None,
        request_id,
    }
}

fn start_when(value: Option<&str>) -> Option<StartWhen> {
    Some(match value.map(str::trim) {
        None | Some("" | "now") => StartWhen::Now,
        Some("at") => StartWhen::At,
        Some("window_reset") => StartWhen::WindowReset,
        Some("blockers_done") => StartWhen::BlockersDone,
        Some(_) => return None,
    })
}

pub(super) async fn task_launch(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskLaunchArguments,
) -> ToolResult<TaskLaunchValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(NOT_FOR_AGENTS);
    }
    if arguments.task_ids.is_empty() || arguments.task_ids.len() > launch::MAX_TASKS_PER_LAUNCH {
        return ToolResult::invalid("taskIds", "Pass 1 to 50 tasks.");
    }
    let Some(start_when) = start_when(arguments.start_when.as_deref()) else {
        return ToolResult::invalid("startWhen", "Use now, at, window_reset or blockers_done.");
    };
    let start_at = match arguments.start_at.as_deref() {
        Some(value) => match chrono::DateTime::parse_from_rfc3339(value.trim()) {
            Ok(at) => Some(at.with_timezone(&chrono::Utc)),
            Err(_) => return ToolResult::invalid("startAt", "Use an RFC 3339 time."),
        },
        None => None,
    };
    let environment_id = match arguments.environment_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("environmentId", "Pass an environment ID."),
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
    let result = async {
        let mut project_id = None;
        let mut work_item_ids = Vec::new();
        for identifier in &arguments.task_ids {
            let Some((task_id, task_project)) = resolve_task(state, principal, identifier).await?
            else {
                return Ok(ToolResult::not_found(&format!("Task {identifier}")));
            };
            if project_id.is_some_and(|project| project != task_project) {
                return Ok(ToolResult::invalid(
                    "taskIds",
                    "Launch tasks from one project at a time.",
                ));
            }
            project_id = Some(task_project);
            work_item_ids.push(task_id);
        }
        let Some(project_id) = project_id else {
            return Ok(ToolResult::invalid("taskIds", "Pass at least one task."));
        };
        let caller = caller(principal, Some(request_id.chars().take(128).collect()));
        let launched = launch::launch_tasks(
            state,
            &caller,
            project_id,
            LaunchRequest {
                work_item_ids,
                environment_id,
                overrides: LaunchOverrides {
                    harness: arguments.harness.clone(),
                    model: arguments.model.clone(),
                    reasoning: arguments.reasoning.clone(),
                    speed: arguments.speed.clone(),
                    attempts: arguments.attempts,
                },
                provider_account_id: None,
                prompt: arguments.prompt.clone(),
                start_when,
                start_at,
                queue: false,
                dialog: false,
            },
            "mcp",
            Ok(key),
        )
        .await?;
        Ok(ToolResult::ok(TaskLaunchValue {
            launches: launched
                .launches
                .into_iter()
                .map(|view| launch_value(state, project_id, view))
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error: AppError| match error {
        AppError::NotFound("project") | AppError::Forbidden => {
            ToolResult::denied("You can't start runs in that project (it needs a member or above).")
        }
        AppError::Invalid(message) => ToolResult::invalid("taskIds", message),
        AppError::ExecutionProfileRequired => ToolResult::conflict(
            "You have no signed-in provider account for that harness in this project.",
        ),
        error => ToolResult::from_error(error, "task_launch"),
    })
}

pub(super) async fn launch_list(
    state: &AppState,
    principal: &McpPrincipal,
    _arguments: ProjectArguments,
) -> ToolResult<LaunchListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if !project_is_chosen(principal) {
        return ToolResult::invalid("projectId", CHOOSE_PROJECT);
    }
    let project_id = principal.project_id;
    match launch::list_for(state, &caller(principal, None), project_id).await {
        Ok(list) => ToolResult::ok(LaunchListValue {
            queued: list
                .queued
                .into_iter()
                .map(|view| launch_value(state, project_id, view))
                .collect(),
            recent: list
                .recent
                .into_iter()
                .map(|view| launch_value(state, project_id, view))
                .collect(),
        }),
        Err(AppError::NotFound(_) | AppError::Forbidden) => ToolResult::not_found("That project"),
        Err(error) => ToolResult::from_error(error, "launch_list"),
    }
}

pub(super) async fn environment_list(
    state: &AppState,
    principal: &McpPrincipal,
    _arguments: ProjectArguments,
) -> ToolResult<EnvironmentListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if !project_is_chosen(principal) {
        return ToolResult::invalid("projectId", CHOOSE_PROJECT);
    }
    match crate::environments::list_for(
        state,
        principal.actor_id,
        principal.actor_kind,
        principal.project_id,
    )
    .await
    {
        Ok(list) => ToolResult::ok(EnvironmentListValue {
            default_environment_id: list.default_environment_id.map(|id| id.to_string()),
            environments: list
                .environments
                .into_iter()
                .map(|environment| EnvironmentValue {
                    id: environment.id.to_string(),
                    name: environment.name,
                    harness: environment.harness,
                    harness_name: environment.harness_name,
                    runs_on: environment.runs_on.map(|runs_on| {
                        serde_json::to_value(runs_on)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    }),
                    provider_environment_id: environment.provider_environment_id,
                    repository_id: environment.repository_id.map(|id| id.to_string()),
                    branch: environment.branch,
                    setup_notes: environment.setup_notes,
                    is_default: environment.is_default,
                    last_check_ok: environment.last_check_ok,
                    last_check_message: environment.last_check_message,
                })
                .collect(),
        }),
        Err(AppError::NotFound(_) | AppError::Forbidden) => ToolResult::not_found("That project"),
        Err(error) => ToolResult::from_error(error, "environment_list"),
    }
}

pub(super) async fn session_bind(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SessionBindArguments,
) -> ToolResult<SessionBindValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::ExternalAgent {
        return ToolResult::denied("Only agents bind their own sessions.");
    }
    let Some(conversation_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass the conversation's ID or link.");
    };
    let harness = arguments.harness.trim();
    if harness.is_empty()
        || harness.len() > 64
        || !harness
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return ToolResult::invalid("harness", "Use a short name such as claude_code.");
    }
    let session_state = match arguments.state.as_deref().map(str::trim) {
        None | Some("" | "active") => "active",
        Some("idle") => "idle",
        Some("ended") => "ended",
        Some(_) => return ToolResult::invalid("state", "Use active, idle or ended."),
    };
    let external_ref = arguments.external_ref.trim();
    let url = arguments
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty());
    if crate::agent_sessions::valid_reference(external_ref, None).is_err() {
        return ToolResult::invalid("externalRef", "Use up to 200 printable characters.");
    }
    if crate::agent_sessions::valid_reference(external_ref, url).is_err() {
        return ToolResult::invalid("url", "Use an https link.");
    }
    let result = async {
        // Only where the agent was asked to work, can still see it, and its
        // connection still reaches the project.
        let Some(project_id) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT conversation.project_id
            FROM conversations conversation
            WHERE conversation.id = $1
              AND conversation_visible_to(conversation.id, $2)
              AND actor_can_access_project($2, $3, conversation.project_id)
              AND EXISTS (
                  SELECT 1 FROM reply_requests request
                  WHERE request.agent_actor_id = $2
                    AND COALESCE(request.thread_id, request.conversation_id) = conversation.id
                    AND (request.status IN ('pending', 'claimed')
                         OR request.updated_at > now() - interval '1 day')
              )
            "#,
        )
        .bind(conversation_id)
        .bind(principal.actor_id)
        .bind(principal.external_agent_connection_id)
        .fetch_optional(&state.pool)
        .await?
        else {
            return Ok(ToolResult::not_found(
                "A conversation you were asked to reply in",
            ));
        };
        let mut transaction = state.pool.begin().await?;
        let session_id = crate::agent_sessions::bind(
            &mut transaction,
            crate::agent_sessions::Binding {
                project_id,
                conversation_id,
                agent_actor_id: principal.actor_id,
                source: "runner",
                harness,
                external_ref,
                url,
                state: session_state,
                agent_run_id: None,
                reply_request_id: None,
            },
        )
        .await?;
        crate::replies::notify(&mut transaction, project_id, conversation_id).await?;
        transaction.commit().await?;
        Ok(ToolResult::ok(SessionBindValue {
            session_id: session_id.to_string(),
            conversation_id: conversation_id.to_string(),
            external_ref: external_ref.to_owned(),
            state: session_state.to_owned(),
        }))
    }
    .await;
    result.unwrap_or_else(|error: AppError| ToolResult::from_error(error, "session_bind"))
}

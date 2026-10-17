//! Use it before it resets (U4) through MCP: `usage_batch` shows the batch a
//! provider account could run before its window resets, and
//! `usage_batch_launch` launches it, or the tasks you pick, each in its own
//! project's environment on that account. Accounts belong to people, so
//! agents have none.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
};
use crate::{
    AppState,
    error::AppError,
    idempotency::IdempotencyKey,
    launch::LaunchCaller,
    policy::ActorKind,
    usage::{
        self,
        batch::{self, BatchLaunchRequest, BatchPick},
    },
};

const NOT_FOR_AGENTS: &str =
    "Provider accounts belong to people; ask a person to launch with run_request.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UsageBatchArguments {
    /// The provider account (usage_windows lists yours).
    pub account_id: String,
    /// The window to size the batch for; defaults to the soonest to reset
    /// with capacity left.
    #[serde(default)]
    pub window_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct BatchTaskLine {
    pub task_id: String,
    /// Such as AWD-12.
    pub key: String,
    pub title: String,
    pub project_id: String,
    pub project: String,
    pub priority: String,
    pub unblocks: usize,
    /// Why it ranks where it does.
    pub reasons: Vec<String>,
    /// The project's default environment it would run in.
    pub environment: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkippedLine {
    pub project: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageBatchValue {
    pub account: String,
    pub harness: String,
    /// How many tasks fit, and why.
    pub size: usize,
    pub sizing: String,
    /// The suggested batch, most important first.
    pub tasks: Vec<BatchTaskLine>,
    /// Other ready tasks you could add.
    pub more: Vec<BatchTaskLine>,
    /// Projects whose tasks can't run on this account, and what to change.
    pub skipped: Vec<SkippedLine>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UsageBatchLaunchArguments {
    pub account_id: String,
    /// Task IDs to launch (from usage_batch). Left out, the suggested batch
    /// as it stands now.
    #[serde(default)]
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub window_key: Option<String>,
    /// A stable ID so a retry never launches twice.
    #[serde(default)]
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LaunchedLine {
    pub project: String,
    pub environment: String,
    /// Task keys queued; follow them with launch_list.
    pub tasks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct FailedLine {
    pub project: String,
    pub tasks: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageBatchLaunchValue {
    pub launched: usize,
    pub projects: Vec<LaunchedLine>,
    pub failed: Vec<FailedLine>,
}

fn line(task: batch::BatchTask) -> BatchTaskLine {
    BatchTaskLine {
        task_id: task.id.to_string(),
        key: task.key,
        title: task.title,
        project_id: task.project_id.to_string(),
        project: task.project_name,
        priority: task.priority,
        unblocks: task.unblocks,
        reasons: task.reasons,
        environment: task.environment_name,
    }
}

/// The person's own account, in the workspace of the connection's project.
async fn account(
    state: &AppState,
    principal: &McpPrincipal,
    account_id: Uuid,
) -> Result<Option<usage::UsageAccount>, AppError> {
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(principal.project_id)
            .fetch_one(&state.pool)
            .await?;
    match usage::owned_account(&state.pool, principal.actor_id, account_id).await {
        Ok(account) if account.organization_id == organization_id => Ok(Some(account)),
        Ok(_) | Err(AppError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) async fn usage_batch(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: UsageBatchArguments,
) -> ToolResult<UsageBatchValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(NOT_FOR_AGENTS);
    }
    let Some(account_id) = parse_id(&arguments.account_id) else {
        return ToolResult::invalid("accountId", "Use a provider account ID.");
    };
    let result = async {
        let Some(account) = account(state, principal, account_id).await? else {
            return Ok(ToolResult::not_found("provider account"));
        };
        let suggestion = batch::suggest(
            state,
            principal.actor_id,
            &account,
            arguments.window_key.as_deref(),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(UsageBatchValue {
            account: suggestion.account_name,
            harness: suggestion.harness_name,
            size: suggestion.capacity.size,
            sizing: suggestion.capacity.summary,
            tasks: suggestion.tasks.into_iter().map(line).collect(),
            more: suggestion.more.into_iter().map(line).collect(),
            skipped: suggestion
                .skipped
                .into_iter()
                .map(|skipped| SkippedLine {
                    project: skipped.project_name,
                    reason: skipped.reason,
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "usage_batch"))
}

pub(super) async fn usage_batch_launch(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: UsageBatchLaunchArguments,
) -> ToolResult<UsageBatchLaunchValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(NOT_FOR_AGENTS);
    }
    let Some(account_id) = parse_id(&arguments.account_id) else {
        return ToolResult::invalid("accountId", "Use a provider account ID.");
    };
    let request_id = arguments
        .client_request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let Ok(key) = IdempotencyKey::parse(&request_id) else {
        return ToolResult::invalid("clientRequestId", "Use up to 128 plain characters.");
    };
    let result = async {
        let Some(account) = account(state, principal, account_id).await? else {
            return Ok(ToolResult::not_found("provider account"));
        };
        let picks = match &arguments.task_ids {
            None => None,
            Some(ids) => {
                if ids.is_empty() || ids.len() > batch::MAX_BATCH {
                    return Ok(ToolResult::invalid("taskIds", "Pass 1 to 20 tasks."));
                }
                let mut picks = Vec::new();
                for id in ids {
                    let Some(work_item_id) = parse_id(id) else {
                        return Ok(ToolResult::invalid(
                            "taskIds",
                            "Use task IDs from usage_batch.",
                        ));
                    };
                    let Some(project_id) = sqlx::query_scalar::<_, Uuid>(
                        r#"
                        SELECT project_id FROM work_items
                        WHERE id = $1 AND actor_can_access_project($2, $3, project_id)
                        "#,
                    )
                    .bind(work_item_id)
                    .bind(principal.actor_id)
                    .bind(principal.external_agent_connection_id)
                    .fetch_optional(&state.pool)
                    .await?
                    else {
                        return Ok(ToolResult::not_found(&format!("Task {id}")));
                    };
                    picks.push(BatchPick {
                        project_id,
                        work_item_id,
                    });
                }
                Some(picks)
            }
        };
        let caller = LaunchCaller {
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind,
            via_grant_id: principal.via_grant_id(),
            mcp: true,
            session_id: None,
            request_id: Some(request_id.clone()),
        };
        let launched = batch::launch_batch(
            state,
            &caller,
            &account,
            BatchLaunchRequest {
                tasks: picks,
                window_key: arguments.window_key.clone(),
            },
            &key,
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(UsageBatchLaunchValue {
            launched: launched.launched,
            projects: launched
                .projects
                .into_iter()
                .map(|project| LaunchedLine {
                    project: project.project_name,
                    environment: project.environment_name,
                    tasks: project
                        .launches
                        .into_iter()
                        .filter_map(|launch| launch.work_item.map(|task| task.key))
                        .collect(),
                })
                .collect(),
            failed: launched
                .failed
                .into_iter()
                .map(|failed| FailedLine {
                    project: failed
                        .project_name
                        .unwrap_or_else(|| "A project".to_owned()),
                    tasks: failed.tasks,
                    reason: failed.reason,
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "usage_batch_launch"))
}

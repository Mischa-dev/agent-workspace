//! What a run did and what runs cost, and a project's agents, through MCP
//! (H5, U2, A9, A10): `run_steps` reads a run's step timeline, tests,
//! files changed and tokens; `run_usage` totals tokens and cost;
//! `agent_profile_list` lists a project's agents with what each is doing
//! and the harnesses a new one could run on. Read only: agents and
//! harnesses are made in the app.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id, project_is_chosen},
};
use crate::{AppState, error::AppError};

const CHOOSE_PROJECT: &str = "Name the project (projectId: an ID, link or key such as \"AWD\").";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunStepsArguments {
    /// The run's ID (run_list, run_get).
    pub run_id: String,
    /// Also return the unified diff against the run's base (it can be long).
    #[serde(default)]
    pub include_diff: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunStepsValue {
    /// The run: state, agent, harness and model, task, branch, pull request,
    /// tokens and cost, then its steps (commands with exit code and the end
    /// of their output, file changes, notes, tools, approvals, test results),
    /// test totals, tokens per model and the files it changed. Secrets were
    /// removed before steps were stored.
    #[schemars(with = "serde_json::Map<String, Value>")]
    pub run: Value,
    /// The diff, when includeDiff: { state, files, patch, truncated }.
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub diff: Option<Value>,
}

pub(super) async fn run_steps(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunStepsArguments,
) -> ToolResult<RunStepsValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(run_id) = parse_id(&arguments.run_id) else {
        return ToolResult::invalid("runId", "Pass the run's ID.");
    };
    let detail = match crate::run_views::run_detail(state, principal.actor_id, run_id).await {
        // The middleware scoped the call to the run's project and checked
        // the caller's access there; a run elsewhere is not found.
        Ok(detail) if detail.run.project_id == principal.project_id => detail,
        Ok(_) | Err(AppError::NotFound(_)) => return ToolResult::not_found("That run"),
        Err(error) => return ToolResult::from_error(error, "run_steps"),
    };
    let diff = if arguments.include_diff {
        match crate::run_views::run_diff(state, principal.actor_id, run_id).await {
            Ok(diff) => serde_json::to_value(diff).ok(),
            Err(error) => return ToolResult::from_error(error, "run_steps"),
        }
    } else {
        None
    };
    match serde_json::to_value(detail) {
        Ok(run) => ToolResult::ok(RunStepsValue { run, diff }),
        Err(_) => ToolResult::from_error(AppError::Internal("serialize run"), "run_steps"),
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunUsageArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// One project (ID, link or key such as "AWD"); left out, every project
    /// you can reach.
    #[serde(default)]
    pub project_id: Option<String>,
    /// day (the default), project, account or model.
    #[serde(default)]
    pub group_by: Option<String>,
    /// How many days back, today included (30 by default, at most 400).
    #[serde(default)]
    pub days: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunUsageValue {
    /// { groupBy, since, rows: [{ key, label, runs, inputTokens,
    /// outputTokens, cachedInputTokens, cost, currency }], totals, runs }.
    /// Cost is estimated from the model catalog's prices, or what the
    /// harness reported; null when neither knows.
    #[schemars(with = "serde_json::Map<String, Value>")]
    pub usage: Value,
}

pub(super) async fn run_usage(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RunUsageArguments,
) -> ToolResult<RunUsageValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if !matches!(
        arguments.group_by.as_deref(),
        None | Some("day" | "project" | "account" | "model")
    ) {
        return ToolResult::invalid("groupBy", "Use day, project, account or model.");
    }
    // AWD-75: not projects where Runs is off for the caller.
    let without_projects =
        match super::runs::projects_with_module_off(state, principal.actor_id, "runs").await {
            Ok(off) => off,
            Err(error) => return ToolResult::from_error(error, "run_usage"),
        };
    let query = crate::run_views::UsageQuery {
        organization_id: None,
        project_id: project_is_chosen(principal).then_some(principal.project_id),
        group_by: arguments.group_by,
        days: arguments.days,
        without_projects,
    };
    match crate::run_views::usage_report(state, principal.actor_id, &query).await {
        Ok(report) => match serde_json::to_value(report) {
            Ok(usage) => ToolResult::ok(RunUsageValue { usage }),
            Err(_) => ToolResult::from_error(AppError::Internal("serialize usage"), "run_usage"),
        },
        Err(error) => ToolResult::from_error(error, "run_usage"),
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AgentProfileListArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project (ID, link or key such as "AWD").
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentProfileListValue {
    /// The agents people made here (name, @handle, template, harness, model
    /// settings, instructions, allowed tools, environment, on or off).
    #[schemars(with = "Vec<serde_json::Map<String, Value>>")]
    pub profiles: Value,
    /// Every agent in the project and what it's doing now: idle, working
    /// (on a task key, in a thread you can see) or waiting for an answer.
    #[schemars(with = "Vec<serde_json::Map<String, Value>>")]
    pub agents: Value,
    /// The harnesses an agent here can run on.
    #[schemars(with = "Vec<serde_json::Map<String, Value>>")]
    pub harnesses: Value,
    /// The templates a new agent can start from.
    #[schemars(with = "Vec<serde_json::Map<String, Value>>")]
    pub templates: Value,
}

pub(super) async fn agent_profile_list(
    state: &AppState,
    principal: &McpPrincipal,
    _arguments: AgentProfileListArguments,
) -> ToolResult<AgentProfileListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if !project_is_chosen(principal) {
        return ToolResult::invalid("projectId", CHOOSE_PROJECT);
    }
    let project_id = principal.project_id;
    let listed = async {
        let list = crate::agent_profiles::list_profiles(
            state,
            principal.actor_id,
            principal.actor_kind,
            project_id,
        )
        .await?;
        let presence = crate::agent_profiles::presence(
            state,
            principal.actor_id,
            principal.actor_kind,
            project_id,
        )
        .await?;
        Ok::<_, AppError>(AgentProfileListValue {
            profiles: json(&list.profiles),
            agents: json(&presence.agents),
            harnesses: json(&list.harnesses),
            templates: json(&list.templates),
        })
    }
    .await;
    match listed {
        Ok(value) => ToolResult::ok(value),
        Err(AppError::NotFound(_) | AppError::Forbidden) => ToolResult::not_found("That project"),
        Err(error) => ToolResult::from_error(error, "agent_profile_list"),
    }
}

fn json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

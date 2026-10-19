//! MCP tools for AI planning (roadmap W10): the review queue, readiness and
//! what to do next, the dependency graph, summaries and plan imports. They
//! call the same functions as the browser routes in `ai/http.rs`.

use rmcp::schemars;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
    planning::reader,
    tasks::command_actor,
    writes::{WriteAccess, check_write_access},
};
use crate::{
    AppState,
    ai::{
        detect::{self, Mode},
        gateway::{self, AiError, Scope, Tier},
        http::{draft_criteria, parse_kinds},
        plans::{self, Source},
        ready::{self, GraphFilter, RankFilter, RankScope},
        suggestions::{self, Filter},
        summaries::{self, SummaryRefusal},
    },
    error::AppError,
    policy::Action,
};

fn ai_refusal<T>(error: AiError) -> ToolResult<T> {
    match error {
        AiError::NotConfigured(message) => ToolResult::conflict(message),
        AiError::App(error) => ToolResult::from_error(error, "ai"),
        error => ToolResult::conflict(error.message()),
    }
}

fn access_refusal<T>(access: WriteAccess) -> Option<ToolResult<T>> {
    match access {
        WriteAccess::Allowed => None,
        WriteAccess::ReadOnly => Some(ToolResult::read_only()),
        WriteAccess::NotFound => Some(ToolResult::not_found("That project")),
        WriteAccess::ProjectArchived => Some(ToolResult::conflict("That project is archived.")),
        WriteAccess::Denied => Some(ToolResult::denied(
            "You can't change tasks in that project.",
        )),
    }
}

/// The caller may read tasks in the named project, a standard one. Returns
/// its workspace.
async fn readable(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: &str,
) -> Result<Result<Uuid, ToolResult<Value>>, AppError> {
    if !principal.can_read() {
        return Ok(Err(ToolResult::denied("This connection can't read.")));
    }
    if parse_id(project_id) != Some(principal.project_id) {
        return Ok(Err(ToolResult::not_found("That project")));
    }
    crate::memory::project_access(
        &state.pool,
        principal.actor_id,
        principal.actor_kind,
        principal.project_id,
        Action::WorkItemView,
    )
    .await?;
    match sqlx::query_scalar::<_, Uuid>(
        "SELECT organization_id FROM projects WHERE id = $1 AND kind = 'standard'",
    )
    .bind(principal.project_id)
    .fetch_optional(&state.pool)
    .await?
    {
        Some(organization_id) => Ok(Ok(organization_id)),
        None => Ok(Err(ToolResult::not_found("That project"))),
    }
}

fn scope(principal: &McpPrincipal, organization_id: Uuid) -> Scope {
    Scope {
        organization_id,
        project_id: Some(principal.project_id),
        actor_id: Some(principal.actor_id),
    }
}

fn value(serialized: impl serde::Serialize) -> ToolResult<Value> {
    match serde_json::to_value(serialized) {
        Ok(value) => ToolResult::ok(value),
        Err(_) => ToolResult::unavailable(),
    }
}

/// A task in the caller's project, by key or ID.
async fn task_in_project(
    state: &AppState,
    principal: &McpPrincipal,
    reference: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(
        match super::tasks::resolve_task(state, principal, reference).await? {
            Some((task_id, project_id)) if project_id == principal.project_id => Some(task_id),
            _ => None,
        },
    )
}

// ---------------------------------------------------------------------------
// ai_suggestions_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AiSuggestionsListArguments {
    pub project_id: String,
    /// pending (the default), accepted, dismissed, applied, or all.
    pub status: Option<String>,
    /// Only suggestions about this task (key such as AWD-12, or ID).
    pub task_id: Option<String>,
    /// Comma-separated kinds: blocks, relates_to, duplicates, parent,
    /// blocked_reason, label, priority, type, criteria.
    pub kind: Option<String>,
    /// At most 100; default 50.
    pub limit: Option<i64>,
    /// nextCursor from the previous page.
    pub cursor: Option<String>,
}

pub(super) async fn ai_suggestions_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AiSuggestionsListArguments,
) -> ToolResult<Value> {
    let result = async {
        if let Err(refusal) = readable(state, principal, &arguments.project_id).await? {
            return Ok(refusal);
        }
        let work_item_id = match &arguments.task_id {
            Some(reference) => match task_in_project(state, principal, reference).await? {
                Some(id) => Some(id),
                None => return Ok(ToolResult::not_found("That task")),
            },
            None => None,
        };
        let kinds = match parse_kinds(arguments.kind.as_deref()) {
            Ok(kinds) => kinds,
            Err(_) => {
                return Ok(ToolResult::invalid(
                    "kind",
                    "Use the kinds listed in kind's description.",
                ));
            }
        };
        let mut connection = state.pool.acquire().await?;
        let page = suggestions::list(
            &mut connection,
            reader(principal),
            principal.project_id,
            &Filter {
                status: arguments.status,
                work_item_id,
                kinds,
                limit: arguments.limit.unwrap_or(50),
                cursor: arguments.cursor,
            },
        )
        .await?;
        Ok::<_, AppError>(value(page))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "ai_suggestions_list"))
}

// ---------------------------------------------------------------------------
// ai_suggestion_decide

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AiSuggestionDecideArguments {
    pub project_id: String,
    pub suggestion_id: String,
    /// accept (apply it, as you) or dismiss (never suggest it again).
    pub decision: String,
}

pub(super) async fn ai_suggestion_decide(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AiSuggestionDecideArguments,
) -> ToolResult<Value> {
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let Some(suggestion_id) = parse_id(&arguments.suggestion_id) else {
        return ToolResult::not_found("That suggestion");
    };
    let accept = match arguments.decision.as_str() {
        "accept" => true,
        "dismiss" => false,
        _ => return ToolResult::invalid("decision", "Pass accept or dismiss."),
    };
    let result = async {
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) =
            access_refusal(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(refusal);
        }
        let actor = command_actor(principal, principal.project_id);
        let outcome = if accept {
            suggestions::accept(
                &mut transaction,
                &actor,
                reader(principal),
                suggestion_id,
                "accepted",
            )
            .await?
        } else {
            suggestions::dismiss(&mut transaction, &actor, reader(principal), suggestion_id).await?
        };
        if let Err(refusal) = outcome {
            transaction.rollback().await?;
            return Ok(match refusal {
                suggestions::Refusal::NotFound => ToolResult::not_found("That suggestion"),
                suggestions::Refusal::Forbidden => ToolResult::denied(refusal.message()),
                _ => ToolResult::conflict(refusal.message()),
            });
        }
        transaction.commit().await?;
        let mut connection = state.pool.acquire().await?;
        let view = suggestions::get(
            &mut connection,
            reader(principal),
            principal.project_id,
            suggestion_id,
        )
        .await?;
        Ok::<_, AppError>(value(json!({"suggestion": view})))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "ai_suggestion_decide"))
}

// ---------------------------------------------------------------------------
// ai_analyze

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AiAnalyzeArguments {
    pub project_id: String,
    /// Triage this one task now (labels, priority, type, parent,
    /// duplicates), instead of looking at the whole project.
    pub task_id: Option<String>,
    /// For taskId: draft acceptance criteria now, as a suggestion.
    pub criteria: Option<bool>,
}

pub(super) async fn ai_analyze(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AiAnalyzeArguments,
) -> ToolResult<Value> {
    let result = async {
        let organization_id = match readable(state, principal, &arguments.project_id).await? {
            Ok(organization_id) => organization_id,
            Err(refusal) => return Ok(refusal),
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) = access_refusal(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(refusal);
        }
        transaction.commit().await?;
        if let Err(error) = gateway::resolve(state, scope(principal, organization_id), Tier::Quick).await
        {
            return Ok(ai_refusal(error));
        }
        let task_id = match &arguments.task_id {
            Some(reference) => match task_in_project(state, principal, reference).await? {
                Some(id) => Some(id),
                None => return Ok(ToolResult::not_found("That task")),
            },
            None => None,
        };
        if arguments.criteria == Some(true) {
            let Some(task_id) = task_id else {
                return Ok(ToolResult::invalid("taskId", "Name the task to draft criteria for."));
            };
            return Ok(
                match draft_criteria(
                    state,
                    organization_id,
                    principal.project_id,
                    principal.actor_id,
                    task_id,
                )
                .await?
                {
                    Ok(id) => {
                        let mut connection = state.pool.acquire().await?;
                        let view = suggestions::get(
                            &mut connection,
                            reader(principal),
                            principal.project_id,
                            id,
                        )
                        .await?;
                        value(json!({"suggestion": view}))
                    }
                    Err(_) => ToolResult::conflict(
                        "The criteria couldn't be drafted. Check Runs and models settings, then try again.",
                    ),
                },
            );
        }
        let run_id = detect::request(
            &state.pool,
            organization_id,
            principal.project_id,
            if task_id.is_some() { Mode::Triage } else { Mode::Analyze },
            Some(principal.actor_id),
            task_id.into_iter().collect(),
        )
        .await?;
        Ok::<_, AppError>(value(json!({
            "runId": run_id,
            "message": "Queued. Suggestions appear in ai_suggestions_list when the run finishes.",
        })))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "ai_analyze"))
}

// ---------------------------------------------------------------------------
// task_ready_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskReadyListArguments {
    pub project_id: String,
    /// ready (the default: not started, blockers done, not marked blocked),
    /// agents (ready, with criteria, a description and a repository, and
    /// unclaimed), or all (every open task, ranked).
    pub scope: Option<String>,
    /// Only tasks in this milestone (ID).
    pub milestone_id: Option<String>,
    /// One task's readiness checks instead of the list.
    pub task_id: Option<String>,
    /// At most 200; default 50.
    pub limit: Option<usize>,
    /// nextOffset from the previous page.
    pub offset: Option<usize>,
}

pub(super) async fn task_ready_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskReadyListArguments,
) -> ToolResult<Value> {
    let result = async {
        if let Err(refusal) = readable(state, principal, &arguments.project_id).await? {
            return Ok(refusal);
        }
        if let Some(reference) = &arguments.task_id {
            let Some(task_id) = task_in_project(state, principal, reference).await? else {
                return Ok(ToolResult::not_found("That task"));
            };
            let readiness = ready::readiness_of(
                &state.pool,
                reader(principal),
                principal.project_id,
                task_id,
            )
            .await?;
            return Ok(match readiness {
                Some(readiness) => value(json!({"readiness": readiness})),
                None => ToolResult::not_found("That task"),
            });
        }
        let scope = match arguments.scope.as_deref() {
            None | Some("ready") => RankScope::Ready,
            Some("agents") => RankScope::Agents,
            Some("all") => RankScope::All,
            Some(_) => return Ok(ToolResult::invalid("scope", "Pass ready, agents or all.")),
        };
        let milestone_id = match &arguments.milestone_id {
            Some(id) => match parse_id(id) {
                Some(id) => Some(id),
                None => return Ok(ToolResult::not_found("That milestone")),
            },
            None => None,
        };
        let ranking = ready::rank(
            &state.pool,
            reader(principal),
            principal.project_id,
            &RankFilter {
                scope,
                milestone_id,
                limit: arguments.limit.unwrap_or(50),
                offset: arguments.offset.unwrap_or(0),
            },
        )
        .await?;
        Ok::<_, AppError>(value(ranking))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_ready_list"))
}

// ---------------------------------------------------------------------------
// dependency_graph

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DependencyGraphArguments {
    pub project_id: String,
    /// Only tasks in this milestone (ID), with what links to them.
    pub milestone_id: Option<String>,
    /// Also finished tasks that block nothing open.
    pub include_finished: Option<bool>,
}

pub(super) async fn dependency_graph(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: DependencyGraphArguments,
) -> ToolResult<Value> {
    let result = async {
        if let Err(refusal) = readable(state, principal, &arguments.project_id).await? {
            return Ok(refusal);
        }
        let milestone_id = match &arguments.milestone_id {
            Some(id) => match parse_id(id) {
                Some(id) => Some(id),
                None => return Ok(ToolResult::not_found("That milestone")),
            },
            None => None,
        };
        let graph = ready::graph(
            &state.pool,
            reader(principal),
            principal.project_id,
            &GraphFilter {
                milestone_id,
                include_finished: arguments.include_finished.unwrap_or(false),
            },
        )
        .await?;
        Ok::<_, AppError>(value(graph))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "dependency_graph"))
}

// ---------------------------------------------------------------------------
// ai_summarize

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AiSummarizeArguments {
    pub project_id: String,
    /// A task to catch up on (key such as AWD-12, or ID).
    pub task_id: Option<String>,
    /// Or a channel or thread to catch up on (ID).
    pub conversation_id: Option<String>,
}

pub(super) async fn ai_summarize(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AiSummarizeArguments,
) -> ToolResult<Value> {
    let result = async {
        let organization_id = match readable(state, principal, &arguments.project_id).await? {
            Ok(organization_id) => organization_id,
            Err(refusal) => return Ok(refusal),
        };
        let outcome = match (&arguments.task_id, &arguments.conversation_id) {
            (Some(reference), None) => {
                let Some(task_id) = task_in_project(state, principal, reference).await? else {
                    return Ok(ToolResult::not_found("That task"));
                };
                summaries::task(
                    state,
                    organization_id,
                    principal.project_id,
                    principal.actor_id,
                    principal.external_agent_connection_id,
                    task_id,
                )
                .await
            }
            (None, Some(conversation)) => {
                let Some(conversation_id) = parse_id(conversation) else {
                    return Ok(ToolResult::not_found("That conversation"));
                };
                summaries::thread(
                    state,
                    organization_id,
                    principal.project_id,
                    principal.actor_id,
                    principal.external_agent_connection_id,
                    conversation_id,
                    None,
                )
                .await
            }
            _ => {
                return Ok(ToolResult::invalid(
                    "taskId",
                    "Pass taskId or conversationId, one of them.",
                ));
            }
        };
        Ok::<_, AppError>(match outcome {
            Ok(summary) => value(json!({"summary": summary})),
            Err(SummaryRefusal::NotFound) => ToolResult::not_found("That task or conversation"),
            Err(SummaryRefusal::Empty) => {
                value(json!({"summary": {"lines": ["Nothing has happened here yet."], "outcome": null, "status": null}}))
            }
            Err(SummaryRefusal::Ai(error)) => ai_refusal(error),
        })
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "ai_summarize"))
}

// ---------------------------------------------------------------------------
// plan_import

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PlanImportArguments {
    pub project_id: String,
    /// Start a new import: the plan's title.
    pub title: Option<String>,
    /// Start a new import: the plan as Markdown or plain text (the model
    /// drafts tasks from it), or as JSON in the draft shape
    /// {"milestones": [...], "tasks": [...]} or a roadmap with workstreams
    /// and items (read without a model).
    pub plan: Option<String>,
    /// Or start from a project page (ID).
    pub page_id: Option<String>,
    /// An import already started, to read or commit.
    pub import_id: Option<String>,
    /// Create the draft's milestones, tasks and dependencies. Pass the
    /// import's version as expectedVersion.
    pub commit: Option<bool>,
    pub expected_version: Option<i64>,
}

pub(super) async fn plan_import(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PlanImportArguments,
) -> ToolResult<Value> {
    let result = async {
        let organization_id = match readable(state, principal, &arguments.project_id).await? {
            Ok(organization_id) => organization_id,
            Err(refusal) => return Ok(refusal),
        };
        if let Some(import) = &arguments.import_id {
            let Some(import_id) = parse_id(import) else {
                return Ok(ToolResult::not_found("That plan"));
            };
            if arguments.commit == Some(true) {
                let Some(expected_version) = arguments.expected_version else {
                    return Ok(ToolResult::invalid(
                        "expectedVersion",
                        "Pass the version you read, so a changed plan isn't created by mistake.",
                    ));
                };
                let mut transaction = state.pool.begin().await?;
                if let Some(refusal) =
                    access_refusal(check_write_access(&mut transaction, principal).await?)
                {
                    return Ok(refusal);
                }
                transaction.commit().await?;
                if let Err(refusal) = plans::commit(
                    &state.pool,
                    organization_id,
                    principal.project_id,
                    import_id,
                    expected_version,
                    principal.actor_id,
                )
                .await?
                {
                    return Ok(match refusal {
                        plans::EditRefusal::NotFound => ToolResult::not_found("That plan"),
                        refusal => ToolResult::conflict(refusal.message()),
                    });
                }
            }
            return Ok(
                match plans::get(&state.pool, principal.project_id, import_id).await? {
                    Some(plan) => value(json!({"plan": plan})),
                    None => ToolResult::not_found("That plan"),
                },
            );
        }
        let Some(title) = arguments.title.as_deref() else {
            return Ok(ToolResult::invalid("title", "Give the plan a title."));
        };
        let source = match (&arguments.plan, &arguments.page_id) {
            (Some(plan), None) => Source::Text { text: plan.clone() },
            (None, Some(page)) => match parse_id(page) {
                Some(page_id) => Source::Page { page_id },
                None => return Ok(ToolResult::not_found("That page")),
            },
            _ => return Ok(ToolResult::invalid("plan", "Pass plan or pageId, one of them.")),
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(refusal) = access_refusal(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(refusal);
        }
        transaction.commit().await?;
        let actor = command_actor(principal, principal.project_id);
        Ok::<_, AppError>(
            match plans::create(state, organization_id, &actor, title, source).await? {
                Ok(plan) => value(json!({
                    "plan": plan,
                    "next": "Review the draft, then call plan_import with importId, commit: true and expectedVersion.",
                })),
                Err(plans::CreateRefusal::Invalid(message)) => ToolResult::invalid("plan", message),
                Err(plans::CreateRefusal::NotFound(message)) => ToolResult::NotFound {
                    message: message.to_owned(),
                },
                Err(plans::CreateRefusal::Ai(error)) => ai_refusal(error),
            },
        )
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "plan_import"))
}

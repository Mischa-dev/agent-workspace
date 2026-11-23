//! Answers with sources (C2.27): a question answered from what search finds
//! for the person asking, with each statement citing the results it came
//! from. Only what the asker can see right now goes to the model: the
//! results are the ordinary search's, read with the asker's access, kept to
//! the workspace whose model (and bill) answers. DMs and groups belong to no
//! workspace, so they stay out. Nothing is stored.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, confined_project, public_url, scope_projects},
    memory_tools::SearchResult,
};
use crate::{
    AppState,
    ai::{
        gateway::{self, AiError, Scope, Tier},
        prompts,
    },
    error::AppError,
    search::{self, SearchFilter},
};

/// The most results an answer reads.
const MAX_RESULTS: i64 = 8;
/// The most characters of one result sent to the model.
const RESULT_CHARS: usize = 600;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AnswerArguments {
    /// The question, in plain words (1 to 200 characters). Search filters
    /// such as in:#channel or kind:page narrow what it reads.
    pub question: String,
    // Read by the dispatcher, which scopes the call to this space.
    #[allow(dead_code)]
    /// Only this space.
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnswerValue {
    /// The answer, citing sources by number in square brackets: [1] is the
    /// first of `sources`.
    pub answer: String,
    /// The results the answer cites, in the order of their numbers.
    pub sources: Vec<SearchResult>,
    /// How many results it read.
    pub read: usize,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Deserialize)]
struct Reply {
    answer: String,
    sources: Vec<i64>,
}

fn refusal<T>(error: AiError) -> ToolResult<T> {
    match error {
        AiError::App(error) => ToolResult::from_error(error, "search_answer"),
        error => ToolResult::conflict(error.message()),
    }
}

pub(crate) async fn answer_tool(
    state: &AppState,
    principal: &McpPrincipal,
    workspace: Uuid,
    arguments: AnswerArguments,
) -> ToolResult<AnswerValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let question = arguments.question.trim().to_owned();
    if question.is_empty() || question.chars().count() > search::MAX_QUERY_CHARACTERS {
        return ToolResult::invalid("question", "Ask in 1 to 200 characters.");
    }
    let parsed = search::grammar::parse(&question);
    if !parsed.errors.is_empty() {
        return ToolResult::invalid(
            "question",
            format!("{} takes other values.", parsed.errors.join(", ")),
        );
    }
    let result = async {
        // The spaces the caller reaches, in this workspace only.
        let reachable = scope_projects(state, principal).await?;
        let projects = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM projects WHERE id = ANY($1) AND organization_id = $2",
        )
        .bind(&reachable)
        .bind(workspace)
        .fetch_all(crate::db::scoped(&state.pool))
        .await?;
        let Some(resolved) = search::resolve(
            &state.pool,
            principal.actor_id,
            &projects,
            &parsed,
            &[],
            SearchFilter {
                confined: confined_project(principal).is_some(),
                ..SearchFilter::default()
            },
        )
        .await?
        else {
            return Ok(nothing());
        };
        if resolved.text.is_empty() {
            return Ok(ToolResult::invalid(
                "question",
                "Ask with some words, not only filters.",
            ));
        }
        let hits = search::search(
            &state.pool,
            crate::addresses::Addresses::of(state),
            principal.actor_id,
            &projects,
            &resolved.text,
            &resolved.kinds,
            &resolved.filter,
            MAX_RESULTS,
        )
        .await?
        .into_iter()
        // A workspace's own boards are in no space: kept to this workspace too.
        .filter(|hit| hit.workspace_id == workspace)
        .collect::<Vec<_>>();
        if hits.is_empty() {
            return Ok(nothing());
        }
        let sent = hits
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                json!({
                    "n": index + 1,
                    "kind": hit.kind,
                    "title": hit.title,
                    "snippet": hit.snippet.chars().take(RESULT_CHARS).collect::<String>(),
                    "author": hit.author,
                    "at": hit.at,
                })
            })
            .collect::<Vec<_>>();
        let scope = Scope {
            organization_id: workspace,
            project_id: principal
                .project_explicit
                .then_some(principal.project_id)
                .filter(|project| projects.contains(project)),
            actor_id: Some(principal.actor_id),
        };
        let answer = match gateway::run::<Reply>(
            state,
            scope,
            &prompts::SEARCH_ANSWER,
            &json!({"question": question, "results": sent}),
            Tier::Quick,
        )
        .await
        {
            Ok(answer) => answer,
            Err(error) => return Ok(refusal(error)),
        };
        let read = hits.len();
        let mut cited = Vec::new();
        for number in answer.value.sources {
            let Some(index) = usize::try_from(number)
                .ok()
                .and_then(|number| number.checked_sub(1))
                .filter(|index| *index < read && !cited.contains(index))
            else {
                continue;
            };
            cited.push(index);
        }
        let sources = cited
            .into_iter()
            .map(|index| {
                let hit = hits[index].clone();
                SearchResult {
                    kind: hit.kind,
                    id: hit.id.to_string(),
                    title: hit.title,
                    snippet: hit.snippet,
                    project: hit.project_name,
                    project_id: hit.project_id.map(|id| id.to_string()),
                    conversation_id: hit.conversation_id.map(|id| id.to_string()),
                    conversation: hit.conversation_name,
                    author: hit.author,
                    at: hit.at,
                    url: public_url(state, &hit.path),
                }
            })
            .collect();
        Ok::<_, AppError>(ToolResult::ok(AnswerValue {
            answer: answer.value.answer,
            sources,
            read,
            provider: answer.provider.to_owned(),
            model: answer.model,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "search_answer"))
}

/// Nothing the asker can see matches: said without asking a model.
fn nothing() -> ToolResult<AnswerValue> {
    ToolResult::ok(AnswerValue {
        answer: "Nothing you can see here answers that. Try other words.".to_owned(),
        sources: Vec::new(),
        read: 0,
        provider: String::new(),
        model: String::new(),
    })
}

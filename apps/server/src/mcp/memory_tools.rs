//! Memory and search tools: find anything by text, and read or write the
//! project's pages and decisions. Pages keep every version, so edits can be
//! undone; decisions from private conversations stay private.

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, insert_audit, parse_id, project_is_chosen, public_url, scope_projects,
        truncate_text,
    },
    talk::can_write_in,
};
use crate::{
    AppState,
    error::AppError,
    memory::{self, DecisionInput, PageInput, Writer},
    policy::Action,
    search::{self, SearchFilter, SearchKind},
};

const PAGE_BODY_CHARACTERS: usize = 60_000;

fn writer(principal: &McpPrincipal) -> Writer {
    Writer {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind,
        via_grant_id: principal.via_grant_id(),
    }
}

// ---------------------------------------------------------------------------
// search

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SearchArguments {
    /// What to look for. Quotes match a phrase; -word excludes a word.
    pub query: String,
    /// Any of: messages, threads, tasks, pages, decisions. Default: all.
    pub kinds: Option<Vec<String>>,
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// Only this project.
    pub project_id: Option<String>,
    /// Only this conversation (and its threads).
    pub conversation_id: Option<String>,
    /// Only things written by this person: actor ID or @handle.
    pub from: Option<String>,
    /// Only after this date (YYYY-MM-DD or a timestamp).
    pub since: Option<String>,
    /// Only before this date (YYYY-MM-DD or a timestamp).
    pub until: Option<String>,
    /// How many results (default 20, at most 100).
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchResult {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub at: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchValue {
    pub results: Vec<SearchResult>,
    pub hint: &'static str,
}

fn parse_time(value: &str, end_of_day: bool) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        return Some(time.with_timezone(&Utc));
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let time = if end_of_day {
        date.and_hms_opt(23, 59, 59)?
    } else {
        date.and_hms_opt(0, 0, 0)?
    };
    Some(time.and_utc())
}

pub(super) async fn search_tool(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SearchArguments,
) -> ToolResult<SearchValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let query = arguments.query.trim().to_owned();
    if query.is_empty() || query.chars().count() > search::MAX_QUERY_CHARACTERS {
        return ToolResult::invalid("query", "Search for 1 to 200 characters.");
    }
    let mut kinds = Vec::new();
    for kind in arguments.kinds.unwrap_or_default() {
        match SearchKind::parse(&kind) {
            Some(kind) => kinds.push(kind),
            None => {
                return ToolResult::invalid(
                    "kinds",
                    "Use messages, threads, tasks, pages, or decisions.",
                );
            }
        }
    }
    let since = match arguments
        .since
        .as_deref()
        .map(|value| parse_time(value, false))
    {
        Some(None) => return ToolResult::invalid("since", "Use YYYY-MM-DD."),
        Some(Some(time)) => Some(time),
        None => None,
    };
    let until = match arguments
        .until
        .as_deref()
        .map(|value| parse_time(value, true))
    {
        Some(None) => return ToolResult::invalid("until", "Use YYYY-MM-DD."),
        Some(Some(time)) => Some(time),
        None => None,
    };
    let conversation_id = match arguments.conversation_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("conversationId", "Pass a conversation ID."),
        },
        None => None,
    };
    let limit = i64::from(arguments.limit.unwrap_or(20));
    if !(1..=100).contains(&limit) {
        return ToolResult::invalid("limit", "Use a limit from 1 to 100.");
    }
    let result = async {
        let projects = scope_projects(state, principal).await?;
        let author_id = match arguments.from.as_deref().map(str::trim) {
            Some(person) => match person.parse::<Uuid>() {
                Ok(id) => Some(id),
                Err(_) => match sqlx::query_scalar::<_, Uuid>(
                    "SELECT actor_id FROM project_memberships WHERE project_id = ANY($1) AND lower(handle) = lower($2) LIMIT 1",
                )
                .bind(&projects)
                .bind(person.trim_start_matches('@'))
                .fetch_optional(&state.pool)
                .await?
                {
                    Some(id) => Some(id),
                    None => {
                        return Ok(ToolResult::invalid(
                            "from",
                            format!("No one called {person} was found."),
                        ));
                    }
                },
            },
            None => None,
        };
        let hits = search::search(
            &state.pool,
            principal.actor_id,
            &projects,
            &query,
            &kinds,
            &SearchFilter {
                conversation_id,
                author_id,
                since,
                until,
            },
            limit,
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(SearchValue {
            results: hits
                .into_iter()
                .map(|hit| SearchResult {
                    kind: hit.kind,
                    id: hit.id.to_string(),
                    title: hit.title,
                    snippet: hit.snippet,
                    project: hit.project_name,
                    conversation_id: hit.conversation_id.map(|id| id.to_string()),
                    conversation: hit.conversation_name,
                    author: hit.author,
                    at: hit.at,
                    url: public_url(state, &hit.path),
                })
                .collect(),
            hint: "Open a message's conversation around it with conversation_read (aroundMessageId). Decisions and pages are the settled version; prefer them over old messages.",
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "search"))
}

// ---------------------------------------------------------------------------
// Pages

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageListArguments {
    pub project_id: String,
    /// Include archived pages.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageLineValue {
    pub id: String,
    pub title: String,
    pub is_guide: bool,
    pub version: i64,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    /// Not touched in months: check it's still right before relying on it.
    pub stale: bool,
    pub excerpt: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageListValue {
    pub pages: Vec<PageLineValue>,
}

pub(super) async fn page_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageListArguments,
) -> ToolResult<PageListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        memory::project_access(
            &state.pool,
            principal.actor_id,
            principal.actor_kind,
            principal.project_id,
            Action::McpRead,
        )
        .await?;
        let pages = memory::list_pages(
            &state.pool,
            principal.project_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(PageListValue {
            pages: pages
                .into_iter()
                .map(|page| PageLineValue {
                    id: page.id.to_string(),
                    title: page.title,
                    is_guide: page.is_guide,
                    version: page.version,
                    updated_at: page.updated_at,
                    updated_by: page.updated_by,
                    stale: page.stale,
                    excerpt: page.excerpt,
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_list"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageGetArguments {
    pub page_id: String,
    /// Read an earlier version instead of the current one.
    pub version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageValue {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub is_guide: bool,
    /// Pass this as expectedVersion when you save.
    pub version: i64,
    pub updated_at: String,
    pub stale: bool,
    pub archived: bool,
    /// Recent versions, newest first. Restore one with page_write.
    pub history: Vec<RevisionLine>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RevisionLine {
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: String,
}

pub(super) async fn page_get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageGetArguments,
) -> ToolResult<PageValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    let result = async {
        let Some(page) = memory::get_page(&state.pool, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        if page.project_id != principal.project_id
            || memory::project_access(
                &state.pool,
                principal.actor_id,
                principal.actor_kind,
                page.project_id,
                Action::McpRead,
            )
            .await
            .is_err()
        {
            return Ok(ToolResult::not_found("That page"));
        }
        let (title, body) = match arguments.version {
            Some(version) if version != page.version => {
                match memory::get_revision(&state.pool, page_id, version).await? {
                    Some(revision) => (revision.title, revision.body),
                    None => return Ok(ToolResult::not_found("That version")),
                }
            }
            _ => (page.title.clone(), page.body.clone()),
        };
        let (body, truncated) = truncate_text(&body, PAGE_BODY_CHARACTERS);
        Ok::<_, AppError>(ToolResult::ok(PageValue {
            url: public_url(
                state,
                &format!("/projects/{}/pages/{}", page.project_id, page.id),
            ),
            id: page.id.to_string(),
            project_id: page.project_id.to_string(),
            title,
            body,
            truncated,
            is_guide: page.is_guide,
            version: page.version,
            updated_at: page.updated_at,
            stale: page.stale,
            archived: page.archived,
            history: page
                .revisions
                .into_iter()
                .take(20)
                .map(|revision| RevisionLine {
                    version: revision.version,
                    author: revision.author,
                    via: revision.via,
                    note: revision.note,
                    at: revision.created_at,
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_get"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageWriteArguments {
    /// The page to change. Leave it out to create a page (then projectId and
    /// title are needed).
    pub page_id: Option<String>,
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    pub project_id: Option<String>,
    pub title: Option<String>,
    /// The whole page, in Markdown. It replaces the current text.
    pub body: Option<String>,
    /// Make this the project's guide page, the one everyone reads first.
    pub is_guide: Option<bool>,
    /// The version you last read. The save is refused if someone changed the
    /// page since.
    pub expected_version: Option<i64>,
    /// Put back an earlier version.
    pub restore_version: Option<i64>,
    /// Archive (true) or bring back (false).
    pub archived: Option<bool>,
    /// A short note on what changed.
    pub note: Option<String>,
    /// Also pin the page to this conversation.
    pub pin_to_conversation_id: Option<String>,
}

pub(super) async fn page_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageWriteArguments,
) -> ToolResult<PageValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let page_id = match arguments.page_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("pageId", "Pass a page ID or link."),
        },
        None => None,
    };
    if page_id.is_none() && !project_is_chosen(principal) {
        return ToolResult::invalid(
            "projectId",
            "Say which project the new page belongs to (projectId).",
        );
    }
    let pin_to = match arguments.pin_to_conversation_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("pinToConversationId", "Pass a conversation ID."),
        },
        None => None,
    };
    let input = PageInput {
        title: arguments.title,
        body: arguments.body,
        is_guide: arguments.is_guide,
        expected_version: arguments.expected_version,
        restore_version: arguments.restore_version,
        archived: arguments.archived,
        note: arguments.note,
    };
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::WorkItemEdit).await? {
            return Ok(ToolResult::denied("You can't edit pages in that project."));
        }
        let mut transaction = state.pool.begin().await?;
        let direct = memory::project_access(
            &mut *transaction,
            principal.actor_id,
            principal.actor_kind,
            project_id,
            Action::WorkItemEdit,
        )
        .await?;
        if direct {
            return Ok(ToolResult::invalid(
                "projectId",
                "Pages live in projects, not in Messages. Pick a project.",
            ));
        }
        let page_id = match page_id {
            Some(page_id) => {
                let owner = sqlx::query_scalar::<_, Uuid>(
                    "SELECT project_id FROM project_pages WHERE id = $1",
                )
                .bind(page_id)
                .fetch_optional(&mut *transaction)
                .await?;
                if owner != Some(project_id) {
                    return Ok(ToolResult::not_found("That page"));
                }
                memory::update_page(&mut transaction, writer(principal), page_id, &input).await?;
                page_id
            }
            None => {
                memory::create_page(&mut transaction, writer(principal), project_id, &input).await?
            }
        };
        if let Some(conversation_id) = pin_to {
            memory::set_pin(
                &mut transaction,
                writer(principal),
                conversation_id,
                "page",
                page_id,
                true,
            )
            .await?;
        }
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "page.mcp_written",
            "page",
            page_id,
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(
            page_get(
                state,
                principal,
                PageGetArguments {
                    page_id: page_id.to_string(),
                    version: None,
                },
            )
            .await,
        )
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_write"))
}

// ---------------------------------------------------------------------------
// Decisions

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DecisionRecordArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project. Not needed when you pass fromMessageId or conversationId.
    pub project_id: Option<String>,
    /// What was decided, in one line.
    pub title: String,
    /// Why, the options considered, and anything a newcomer should know.
    pub detail: Option<String>,
    /// The message where it was decided.
    pub from_message_id: Option<String>,
    /// The conversation or thread where it was decided.
    pub conversation_id: Option<String>,
    /// An earlier decision this one replaces.
    pub supersedes: Option<String>,
    /// Also pin the decision to its conversation.
    pub pin: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct DecisionValue {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub detail: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_conversation: Option<String>,
    pub decided_by: String,
    pub decided_at: String,
}

pub(super) async fn decision_record(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: DecisionRecordArguments,
) -> ToolResult<DecisionValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let parse = |value: Option<&str>| -> Result<Option<Uuid>, ()> {
        match value {
            Some(value) => parse_id(value).map(Some).ok_or(()),
            None => Ok(None),
        }
    };
    let Ok(source_message_id) = parse(arguments.from_message_id.as_deref()) else {
        return ToolResult::invalid("fromMessageId", "Pass a message ID or link.");
    };
    let Ok(source_conversation_id) = parse(arguments.conversation_id.as_deref()) else {
        return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
    };
    let Ok(supersedes) = parse(arguments.supersedes.as_deref()) else {
        return ToolResult::invalid("supersedes", "Pass the earlier decision's ID.");
    };
    if !project_is_chosen(principal) {
        return ToolResult::invalid(
            "projectId",
            "Say which project (projectId), or pass the message or conversation it came from.",
        );
    }
    let input = DecisionInput {
        title: arguments.title,
        detail: arguments.detail,
        source_message_id,
        source_conversation_id,
        supersedes,
    };
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::MessageSend).await? {
            return Ok(ToolResult::denied(
                "You can't record decisions in that project.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        memory::project_access(
            &mut *transaction,
            principal.actor_id,
            principal.actor_kind,
            project_id,
            Action::MessageSend,
        )
        .await?;
        let decision_id =
            memory::record_decision(&mut transaction, writer(principal), project_id, &input)
                .await?;
        if arguments.pin.unwrap_or(false) {
            let conversation = match (input.source_conversation_id, input.source_message_id) {
                (Some(id), _) => Some(id),
                (None, Some(message_id)) => {
                    sqlx::query_scalar::<_, Uuid>(
                        "SELECT conversation_id FROM messages WHERE id = $1",
                    )
                    .bind(message_id)
                    .fetch_optional(&mut *transaction)
                    .await?
                }
                _ => None,
            };
            if let Some(conversation_id) = conversation {
                memory::set_pin(
                    &mut transaction,
                    writer(principal),
                    conversation_id,
                    "decision",
                    decision_id,
                    true,
                )
                .await?;
            }
        }
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "decision.mcp_recorded",
            "decision",
            decision_id,
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        let decision = memory::get_decision(&state.pool, decision_id, principal.actor_id)
            .await?
            .ok_or(AppError::NotFound("decision"))?;
        Ok::<_, AppError>(ToolResult::ok(DecisionValue {
            id: decision.id.to_string(),
            project_id: decision.project_id.to_string(),
            title: decision.title,
            detail: decision.detail,
            status: decision.status,
            source_conversation: decision.source_conversation_name,
            decided_by: decision.decided_by,
            decided_at: decision.created_at,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "decision_record"))
}

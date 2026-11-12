//! Memory and search tools: find anything by text, and read or write the
//! project's pages and decisions. Pages keep every version, so edits can be
//! undone; decisions from private conversations stay private.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, insert_audit, parse_id, parse_message_id, parse_time, project_is_chosen,
        public_url, scope_projects, string_or_number, truncate_text,
    },
    talk::can_write_in,
};
use crate::{
    AppState,
    error::AppError,
    memory::{self, DecisionInput, PageInput, Writer},
    page_docs::{
        edits::{self, Edit, EditError, EditOp, Target},
        store, suggestions,
        tree::{Block, Node, plain_text},
        ydoc,
    },
    policy::Action,
    search::{self, SearchFilter, SearchKind},
};

const PAGE_BODY_CHARACTERS: usize = 60_000;

pub(super) fn writer(principal: &McpPrincipal) -> Writer {
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
                    "Use messages, threads, tasks, pages, decisions, or files.",
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
                    // Agents read the status in the title, as "Name (status)".
                    title: match hit.status {
                        Some(status) => format!("{} ({status})", hit.title),
                        None => hit.title,
                    },
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
    /// Set when the page is a skill: when to use it. skill_get loads it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_description: Option<String>,
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
                    skill_description: page.skill_description,
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
    /// Set when the page is a skill: the line that says when to use it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_description: Option<String>,
    /// Pass this as expectedVersion when you save.
    pub version: i64,
    pub updated_at: String,
    pub stale: bool,
    pub archived: bool,
    /// Recent versions, newest first. Restore one with page_write.
    pub history: Vec<RevisionLine>,
    pub url: String,
    /// Open comment threads on the page (page_comments reads them).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub open_comments: i64,
    /// Suggested edits waiting on someone (page_suggestions reads them).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_suggestions: i64,
    /// Set when your change to the text was kept as a suggestion instead
    /// (the page is someone else's); the page above doesn't have it yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<SuggestionMade>,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
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
        let (open_comments, pending_suggestions) = sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT
                (SELECT count(*) FROM project_page_comments
                 WHERE page_id = $1 AND thread_id = id AND resolved_at IS NULL
                   AND EXISTS (SELECT 1 FROM project_page_comments reply
                               WHERE reply.thread_id = project_page_comments.id
                                 AND reply.deleted_at IS NULL)),
                (SELECT count(*) FROM project_page_suggestions
                 WHERE page_id = $1 AND status = 'pending')
            "#,
        )
        .bind(page_id)
        .fetch_one(&state.pool)
        .await?;
        Ok::<_, AppError>(ToolResult::ok(PageValue {
            open_comments,
            pending_suggestions,
            suggestion: None,
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
            skill_description: page.skill_description,
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
    /// Make the page a skill: one line saying when an agent should use it
    /// ("Use when deploying the web app"). An empty string makes it a plain
    /// page again.
    pub skill_description: Option<String>,
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
    let mut input = PageInput {
        title: arguments.title,
        body: arguments.body,
        is_guide: arguments.is_guide,
        skill_description: arguments.skill_description,
        expected_version: arguments.expected_version,
        restore_version: arguments.restore_version,
        archived: arguments.archived,
        note: arguments.note,
    };
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::PageEdit).await? {
            return Ok(ToolResult::denied("You can't edit pages in that project."));
        }
        let mut transaction = state.pool.begin().await?;
        let direct = memory::project_access(
            &mut *transaction,
            principal.actor_id,
            principal.actor_kind,
            project_id,
            Action::PageEdit,
        )
        .await?;
        if direct {
            return Ok(ToolResult::invalid(
                "projectId",
                "Pages live in projects, not in Messages. Pick a project.",
            ));
        }
        let mut suggestion = None;
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
                // An agent's new text for someone else's page is kept as a
                // suggestion; anything else it changes applies.
                let changes_text = input.body.is_some() || input.restore_version.is_some();
                if changes_text
                    && suggestions::must_suggest(
                        &writer(principal),
                        suggestions::page_creator(&mut transaction, page_id).await?,
                    )
                {
                    match suggest_text(&mut transaction, principal, page_id, &input).await? {
                        Ok(made) => suggestion = made,
                        Err(refused) => return Ok(refused),
                    }
                    input.body = None;
                    input.restore_version = None;
                }
                let more = input.title.is_some()
                    || input.is_guide.is_some()
                    || input.skill_description.is_some()
                    || input.archived.is_some()
                    || input.body.is_some()
                    || input.restore_version.is_some();
                if more {
                    memory::update_page(&mut transaction, writer(principal), page_id, &input)
                        .await?;
                }
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
        let mut value = page_get(
            state,
            principal,
            PageGetArguments {
                page_id: page_id.to_string(),
                version: None,
            },
        )
        .await;
        if let (Some(made), ToolResult::Ok { value: page }) = (suggestion, &mut value) {
            page.suggestion = Some(made);
        }
        Ok::<_, AppError>(value)
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_write"))
}

/// Keeps an agent's new text for someone else's page as a suggestion: the
/// block edits that turn the page into it. `None` when the text is what the
/// page already says.
async fn suggest_text(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &McpPrincipal,
    page_id: Uuid,
    input: &PageInput,
) -> Result<Result<Option<SuggestionMade>, ToolResult<PageValue>>, AppError> {
    let Some(page) = store::lock_page(transaction, page_id).await? else {
        return Ok(Err(ToolResult::not_found("That page")));
    };
    if let Some(expected) = input.expected_version
        && expected != page.version
    {
        return Err(AppError::VersionConflict {
            resource: "page",
            current_version: page.version,
        });
    }
    if page.archived {
        return Ok(Err(ToolResult::invalid(
            "pageId",
            "That page is archived. Bring it back with page_write (archived: false) to edit it.",
        )));
    }
    let markdown = match (input.restore_version, input.body.as_deref()) {
        (_, Some(body)) => memory::normalize_body(body, memory::MAX_PAGE_BODY)?,
        (Some(restore), None) => sqlx::query_scalar::<_, String>(
            "SELECT body FROM project_page_revisions WHERE page_id = $1 AND version = $2",
        )
        .bind(page_id)
        .bind(restore)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("page version"))?,
        (None, None) => return Ok(Ok(None)),
    };
    let note = memory::normalize_note(input.note.as_deref())?.or_else(|| {
        input
            .restore_version
            .map(|restore| format!("Restore version {restore}"))
    });
    let stored = store::document(transaction, page_id, &page).await?;
    let current = store::read_blocks(&stored.state)?;
    let Some(edits) = suggestions::diff(&current, &markdown) else {
        return Ok(Ok(None));
    };
    if edits.len() > edits::MAX_EDITS {
        return Ok(Err(ToolResult::invalid(
            "body",
            format!(
                "That changes more than {} blocks of someone else's page at once. Suggest it in parts with page_edit.",
                edits::MAX_EDITS
            ),
        )));
    }
    let stored_edits = match suggestions::resolve(&current, &edits) {
        Ok(stored) => stored,
        Err(error) => return Ok(Err(ToolResult::invalid("body", error.to_string()))),
    };
    let id = suggestions::create(
        transaction,
        writer(principal),
        page_id,
        &page,
        &stored_edits,
        note.as_deref(),
    )
    .await?;
    insert_audit(
        transaction,
        principal,
        page.project_id,
        "page.suggestion_created",
        "page",
        page_id,
        serde_json::json!({ "edits": stored_edits.len(), "fromVersion": page.version, "suggestionId": id }),
    )
    .await?;
    Ok(Ok(Some(SuggestionMade::pending(id))))
}

// ---------------------------------------------------------------------------
// Pages as blocks (AWD-53): read a page's outline and blocks, and change it
// block by block instead of rewriting the whole page. Every block has a
// lasting id, kept in the page's stored document, so an edit that names
// blocks by id needs no version: it lands even while people are typing
// elsewhere on the page, and a block's hash stops it undoing a change to
// that block the agent hasn't seen.

/// How much of a block page_blocks shows without includeMarkdown.
const BLOCK_PREVIEW_CHARACTERS: usize = 120;

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageBlocksArguments {
    pub page_id: String,
    /// The version positions came from: needed when section, from or to is
    /// a position, and the read is then refused if the page has changed
    /// since. Not needed with block ids.
    pub version: Option<i64>,
    /// Read the section under a heading: its id (or position). That's the
    /// heading and every block up to the next heading at its level or above.
    #[serde(default, deserialize_with = "string_or_number")]
    #[schemars(with = "Option<String>")]
    pub section: Option<String>,
    /// Read the blocks from this one (an id, or a position)…
    #[serde(default, deserialize_with = "string_or_number")]
    #[schemars(with = "Option<String>")]
    pub from: Option<String>,
    /// …to this one, both included (defaults to from).
    #[serde(default, deserialize_with = "string_or_number")]
    #[schemars(with = "Option<String>")]
    pub to: Option<String>,
    /// With no section or range: give each block's whole Markdown instead
    /// of a short preview.
    pub include_markdown: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageBlocksValue {
    pub page_id: String,
    pub project_id: String,
    pub title: String,
    /// The page's version. Block ids last across versions; positions hold
    /// only within this one.
    pub version: i64,
    /// The headings, each with the last block of its section.
    pub outline: Vec<OutlineLine>,
    /// Every block, or the ones you selected.
    pub blocks: Vec<BlockLine>,
    /// Exactly the page's Markdown for a section or range you asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Some Markdown was left out to keep the answer small; read a range.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub url: String,
    /// Set when your edits were kept as a suggestion instead of changing
    /// the page (it's someone else's): the page above is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<SuggestionMade>,
}

/// Edits kept as a suggestion for the page's owner to accept or reject.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SuggestionMade {
    pub suggestion_id: String,
    /// pending until someone decides; see page_suggestions.
    pub status: String,
    pub message: String,
}

impl SuggestionMade {
    pub(super) fn pending(id: Uuid) -> Self {
        Self {
            suggestion_id: id.to_string(),
            status: "pending".to_owned(),
            message: "This page is someone else's, so your edits were kept as a suggestion for them to accept or reject. The page hasn't changed yet; page_suggestions shows what happens to it. Withdraw it with page_suggestion_decide.".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct OutlineLine {
    pub id: String,
    pub position: usize,
    pub level: u8,
    pub text: String,
    pub hash: String,
    /// The position of the last block in the heading's section.
    pub section_end: usize,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct BlockLine {
    /// The block's lasting id: it stays the same while the block is edited
    /// or moved. Pass it to page_edit as blockId.
    pub id: String,
    /// Where the block is in this version.
    pub position: usize,
    /// heading, paragraph, code, quote, list, hr, table, or embed (a line
    /// `[label](/path "embed")` that shows something of the app live, such
    /// as a collection's view).
    #[serde(rename = "type")]
    pub kind: String,
    /// A heading's level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// A digest of the block's contents. Pass it to page_edit to replace or
    /// delete the block; it proves you read the block as it is.
    pub hash: String,
    /// The block's start, when its whole Markdown isn't given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

/// The page, with access checked the way page_get checks it.
async fn readable_page(
    state: &AppState,
    principal: &McpPrincipal,
    page_id: Uuid,
) -> Result<Option<memory::PageView>, AppError> {
    let Some(page) = memory::get_page(&state.pool, page_id).await? else {
        return Ok(None);
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
        return Ok(None);
    }
    Ok(Some(page))
}

/// One version of a page as blocks: its document's blocks and the page's
/// Markdown for each.
struct PageAsBlocks {
    page_id: Uuid,
    project_id: Uuid,
    title: String,
    version: i64,
    blocks: Vec<Block>,
    derived: store::Derived,
}

impl PageAsBlocks {
    fn heading_level(&self, position: usize) -> Option<u8> {
        match &self.blocks[position].node {
            Node::Heading { level, .. } => Some(*level),
            _ => None,
        }
    }

    /// The last block in the section under the heading at `position`.
    fn section_end(&self, position: usize, level: u8) -> usize {
        (position + 1..self.blocks.len())
            .find(|&next| self.heading_level(next).is_some_and(|other| other <= level))
            .map_or(self.blocks.len() - 1, |next| next - 1)
    }

    /// The page's Markdown for blocks `from` to `to`.
    fn markdown_of(&self, from: usize, to: usize) -> &str {
        let ranges = &self.derived.ranges[from..=to];
        match (
            ranges.iter().flatten().next(),
            ranges.iter().flatten().next_back(),
        ) {
            (Some(first), Some(last)) => &self.derived.markdown[first.start..last.end],
            _ => "",
        }
    }

    fn markdown_of_block(&self, position: usize) -> &str {
        self.derived.ranges[position]
            .clone()
            .map_or("", |range| &self.derived.markdown[range])
    }

    fn id(&self, position: usize) -> String {
        self.blocks[position].id.clone().unwrap_or_default()
    }

    /// A block named by id, or by position (digits) in this version.
    fn find(&self, reference: &str, field: &str) -> Result<(usize, bool), String> {
        if !reference.is_empty() && reference.bytes().all(|byte| byte.is_ascii_digit()) {
            let position = reference
                .parse::<usize>()
                .ok()
                .filter(|&position| position < self.blocks.len())
                .ok_or_else(|| {
                    format!(
                        "{field}: there's no block {reference}; the page has {}",
                        self.blocks.len()
                    )
                })?;
            return Ok((position, true));
        }
        self.blocks
            .iter()
            .position(|block| block.id.as_deref() == Some(reference))
            .map(|position| (position, false))
            .ok_or_else(|| format!("{field}: there's no block {reference} on the page now"))
    }
}

fn block_type(node: &Node) -> &'static str {
    match node {
        Node::Paragraph(_) => "paragraph",
        Node::Heading { .. } => "heading",
        Node::CodeBlock { .. } => "code",
        Node::Blockquote(_) => "quote",
        Node::BulletList(_) | Node::OrderedList { .. } => "list",
        Node::HorizontalRule => "hr",
        Node::Table(_) => "table",
        Node::Embed { .. } => "embed",
    }
}

async fn page_as_blocks(
    state: &AppState,
    principal: &McpPrincipal,
    page_id: Uuid,
) -> Result<Option<PageAsBlocks>, AppError> {
    let Some(page) = readable_page(state, principal, page_id).await? else {
        return Ok(None);
    };
    let snapshot = store::open(&state.pool, page_id).await?;
    let blocks = store::read_blocks(&snapshot.state)?;
    let derived = store::derive_markdown(&snapshot.body, &blocks);
    Ok(Some(PageAsBlocks {
        page_id,
        project_id: page.project_id,
        title: page.title,
        version: snapshot.version,
        blocks,
        derived,
    }))
}

fn blocks_value(
    state: &AppState,
    page: &PageAsBlocks,
    selection: Option<(usize, usize)>,
    include_markdown: bool,
) -> PageBlocksValue {
    let mut budget = PAGE_BODY_CHARACTERS;
    let mut truncated = false;
    let (first, last) = selection.unwrap_or((0, page.blocks.len().saturating_sub(1)));
    let whole = selection.is_some() || include_markdown;
    let lines = page
        .blocks
        .iter()
        .enumerate()
        .filter(|(position, _)| *position >= first && *position <= last)
        .map(|(position, block)| {
            let text = page.markdown_of_block(position);
            let (preview, markdown) = if whole {
                let length = text.chars().count();
                if length <= budget {
                    budget -= length;
                    (None, Some(text.to_owned()))
                } else {
                    truncated = true;
                    let (preview, _) = truncate_text(text, BLOCK_PREVIEW_CHARACTERS);
                    (Some(preview), None)
                }
            } else {
                let first_line = text.lines().next().unwrap_or_default();
                let (preview, _) = truncate_text(first_line, BLOCK_PREVIEW_CHARACTERS);
                (Some(preview), None)
            };
            BlockLine {
                id: page.id(position),
                position,
                kind: block_type(&block.node).to_owned(),
                level: page.heading_level(position),
                hash: edits::content_hash(block),
                preview,
                markdown,
            }
        })
        .collect();
    let markdown = selection.map(|(from, to)| {
        let (text, cut) = truncate_text(page.markdown_of(from, to), PAGE_BODY_CHARACTERS);
        truncated |= cut;
        text
    });
    let outline = page
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(position, block)| match &block.node {
            Node::Heading { level, content } => Some(OutlineLine {
                id: page.id(position),
                position,
                level: *level,
                text: plain_text(content),
                hash: edits::content_hash(block),
                section_end: page.section_end(position, *level),
            }),
            _ => None,
        })
        .collect();
    PageBlocksValue {
        url: public_url(
            state,
            &format!("/projects/{}/pages/{}", page.project_id, page.page_id),
        ),
        page_id: page.page_id.to_string(),
        project_id: page.project_id.to_string(),
        title: page.title.clone(),
        version: page.version,
        outline,
        blocks: lines,
        markdown,
        truncated,
        suggestion: None,
    }
}

pub(super) async fn page_blocks(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageBlocksArguments,
) -> ToolResult<PageBlocksValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    if arguments.section.is_some() && (arguments.from.is_some() || arguments.to.is_some()) {
        return ToolResult::invalid("section", "Pass a section or a range, not both.");
    }
    if arguments.to.is_some() && arguments.from.is_none() {
        return ToolResult::invalid("from", "Pass from with to.");
    }
    let result = async {
        let Some(page) = page_as_blocks(state, principal, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        let mut by_position = false;
        let mut find = |reference: &str, field: &str| {
            page.find(reference, field).map(|(position, positional)| {
                by_position |= positional;
                position
            })
        };
        let selection = match (&arguments.section, &arguments.from, &arguments.to) {
            (Some(section), _, _) => match find(section, "section") {
                Ok(position) => match page.heading_level(position) {
                    Some(level) => Some((position, page.section_end(position, level))),
                    None => {
                        return Ok(ToolResult::invalid(
                            "section",
                            format!("block {section} isn't a heading"),
                        ));
                    }
                },
                Err(message) => return Ok(missing_block(message, section)),
            },
            (None, Some(from), to) => {
                let from_position = match find(from, "from") {
                    Ok(position) => position,
                    Err(message) => return Ok(missing_block(message, from)),
                };
                let to_position = match to.as_deref().map(|to| (to, find(to, "to"))) {
                    None => from_position,
                    Some((_, Ok(position))) => position,
                    Some((to, Err(message))) => return Ok(missing_block(message, to)),
                };
                if to_position < from_position {
                    return Ok(ToolResult::invalid(
                        "from",
                        format!("block {from} comes after block {}", to.as_deref().unwrap_or(from)),
                    ));
                }
                Some((from_position, to_position))
            }
            _ => None,
        };
        if by_position {
            match arguments.version {
                None => {
                    return Ok(ToolResult::invalid(
                        "version",
                        "Pass the version your positions came from (page_blocks returns it), or use block ids.",
                    ));
                }
                Some(version) if version != page.version => {
                    return Ok(ToolResult::conflict(format!(
                        "block positions are from version {version}, but the page is at version {}; read it again, or use block ids",
                        page.version
                    )));
                }
                Some(_) => {}
            }
        }
        Ok::<_, AppError>(ToolResult::ok(blocks_value(
            state,
            &page,
            selection,
            arguments.include_markdown.unwrap_or(false),
        )))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_blocks"))
}

/// A block that isn't there: gone since it was read (an id), or never there.
fn missing_block<T>(message: String, reference: &str) -> ToolResult<T> {
    if reference.bytes().all(|byte| byte.is_ascii_digit()) {
        ToolResult::invalid("from", message)
    } else {
        ToolResult::conflict(format!("{message}; read the page again"))
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageEditArguments {
    pub page_id: String,
    /// The version you read. Needed when an edit names a block by position,
    /// and then the edit is refused if the page has changed since; with
    /// block ids it's optional.
    pub expected_version: Option<i64>,
    /// Up to 100 edits, applied together or not at all. Each names a block
    /// as the page is now, never as an earlier edit in the same call left it.
    pub edits: Vec<PageEditInput>,
    /// A short note on what changed, kept with the version.
    pub note: Option<String>,
    /// Keep the edits as a suggestion for someone to accept, even on a page
    /// you may change directly. On a page someone else made, an agent's
    /// edits are always suggestions.
    pub suggest: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageEditInput {
    /// replace, insertBefore, insertAfter, delete, or append (to the end of
    /// the page; names no block).
    pub op: String,
    /// The block's id, from page_blocks.
    pub block_id: Option<String>,
    /// Or the block's position in expectedVersion.
    pub position: Option<usize>,
    /// The block's hash, from page_blocks: needed to replace or delete, and
    /// the edit is refused if the block has changed since you read it.
    pub hash: Option<String>,
    /// The new Markdown, one or more blocks (not for delete). A replaced
    /// block keeps its id as the first block of its replacement.
    pub markdown: Option<String>,
}

pub(super) async fn page_edit(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageEditArguments,
) -> ToolResult<PageBlocksValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    let note = match memory::normalize_note(arguments.note.as_deref()) {
        Ok(note) => note,
        Err(_) => return ToolResult::invalid("note", "Keep the note under 500 characters."),
    };
    let mut edits = Vec::with_capacity(arguments.edits.len());
    let mut by_position = false;
    for (index, edit) in arguments.edits.into_iter().enumerate() {
        let op = match edit.op.as_str() {
            "replace" => EditOp::Replace,
            "insertBefore" => EditOp::InsertBefore,
            "insertAfter" => EditOp::InsertAfter,
            "delete" => EditOp::Delete,
            "append" => EditOp::Append,
            _ => {
                return ToolResult::invalid(
                    "edits",
                    format!(
                        "edit {index}: op is replace, insertBefore, insertAfter, delete or append"
                    ),
                );
            }
        };
        let target = match (edit.block_id, edit.position) {
            (Some(_), Some(_)) => {
                return ToolResult::invalid(
                    "edits",
                    format!("edit {index}: pass blockId or position, not both"),
                );
            }
            (Some(id), None) => Some(Target::Id(id)),
            (None, Some(position)) => {
                by_position = true;
                Some(Target::Position(position))
            }
            (None, None) => None,
        };
        edits.push(Edit {
            op,
            target,
            hash: edit.hash,
            markdown: edit.markdown,
        });
    }
    if by_position && arguments.expected_version.is_none() {
        return ToolResult::invalid(
            "expectedVersion",
            "Pass the version your positions came from (page_blocks returns it), or name blocks by blockId.",
        );
    }
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::PageEdit).await? {
            return Ok(ToolResult::denied("You can't edit pages in that project."));
        }
        let mut transaction = state.pool.begin().await?;
        let Some(page) = store::lock_page(&mut transaction, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        if page.project_id != project_id {
            return Ok(ToolResult::not_found("That page"));
        }
        if page.archived {
            return Ok(ToolResult::invalid(
                "pageId",
                "That page is archived. Bring it back with page_write (archived: false) to edit it.",
            ));
        }
        if let Some(expected) = arguments.expected_version
            && expected != page.version
        {
            return Ok(ToolResult::conflict(format!(
                "the edits are for version {expected}, but the page is at version {}; read it again{}",
                page.version,
                if by_position { "" } else { ", or leave out expectedVersion to edit by block id" }
            )));
        }
        let stored = store::document(&mut transaction, page_id, &page).await?;
        let current = store::read_blocks(&stored.state)?;
        let blocks = match edits::apply(&current, &edits) {
            Ok(blocks) => blocks,
            Err(
                error @ (EditError::HashMismatch { .. }
                | EditError::NoSuchBlock {
                    target: Target::Id(_),
                    ..
                }),
            ) => return Ok(ToolResult::conflict(error.to_string())),
            Err(error) => return Ok(ToolResult::invalid("edits", error.to_string())),
        };
        let writer = writer(principal);
        let creator = suggestions::page_creator(&mut transaction, page_id).await?;
        if arguments.suggest == Some(true) || suggestions::must_suggest(&writer, creator) {
            let stored_edits = match suggestions::resolve(&current, &edits) {
                Ok(stored) => stored,
                Err(error) => return Ok(ToolResult::invalid("edits", error.to_string())),
            };
            let id = suggestions::create(
                &mut transaction,
                writer,
                page_id,
                &page,
                &stored_edits,
                note.as_deref(),
            )
            .await?;
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                "page.suggestion_created",
                "page",
                page_id,
                serde_json::json!({ "edits": edits.len(), "fromVersion": page.version, "suggestionId": id }),
            )
            .await?;
            transaction.commit().await?;
            let derived = store::derive_markdown(&page.body, &current);
            let now = PageAsBlocks {
                page_id,
                project_id,
                title: page.title,
                version: page.version,
                blocks: current,
                derived,
            };
            let mut value = blocks_value(state, &now, None, false);
            value.suggestion = Some(SuggestionMade::pending(id));
            return Ok(ToolResult::ok(value));
        }
        // Keep the words of the Markdown sent, for the blocks it says.
        let sent: Vec<&str> = edits
            .iter()
            .filter_map(|edit| edit.markdown.as_deref())
            .collect();
        let change = store::change(&stored.state, &page.body, &sent, |doc, _| {
            ydoc::replace(doc, &blocks);
            Ok(())
        })?;
        let (version, _) = store::save(
            &mut transaction,
            writer,
            page_id,
            &page,
            &change,
            note.as_deref(),
            store::History::Version,
        )
        .await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "page.mcp_block_edited",
            "page",
            page_id,
            serde_json::json!({ "edits": edits.len(), "fromVersion": page.version }),
        )
        .await?;
        transaction.commit().await?;
        let after = PageAsBlocks {
            page_id,
            project_id,
            title: page.title,
            version,
            blocks: change.blocks,
            derived: change.derived,
        };
        Ok::<_, AppError>(ToolResult::ok(blocks_value(state, &after, None, false)))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_edit"))
}

// ---------------------------------------------------------------------------
// Skills
//
// A skill is a page with a one-line description of when to use it. Agents
// read the list (cheap: names and descriptions) and load a skill's page only
// when its description fits the work, as Claude Code and Codex do with skill
// folders. skill_get also returns the skill as a SKILL.md, so an agent can
// keep a copy where its own harness looks for skills.

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SkillListArguments {
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkillLine {
    /// Pass this to skill_get.
    pub name: String,
    /// When to use it.
    pub description: String,
    pub page_id: String,
    pub title: String,
    pub version: i64,
    pub updated_at: String,
    /// Not touched in months: check it's still right before relying on it.
    pub stale: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkillListValue {
    pub skills: Vec<SkillLine>,
}

pub(super) fn skill_lines(pages: Vec<memory::PageSummary>) -> Vec<SkillLine> {
    pages
        .into_iter()
        .filter_map(|page| {
            Some(SkillLine {
                name: memory::skill_name(&page.title),
                description: page.skill_description?,
                page_id: page.id.to_string(),
                title: page.title,
                version: page.version,
                updated_at: page.updated_at,
                stale: page.stale,
            })
        })
        .collect()
}

pub(super) async fn skill_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SkillListArguments,
) -> ToolResult<SkillListValue> {
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
        let skills = memory::list_skills(&state.pool, principal.project_id).await?;
        Ok::<_, AppError>(ToolResult::ok(SkillListValue {
            skills: skill_lines(skills),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "skill_list"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SkillGetArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project. Needed with name; not with pageId.
    pub project_id: Option<String>,
    /// The skill's name, as skill_list shows it.
    pub name: Option<String>,
    /// Or the skill's page.
    pub page_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkillValue {
    pub name: String,
    pub description: String,
    /// The instructions, in Markdown.
    pub body: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub page_id: String,
    pub project_id: String,
    pub version: i64,
    pub updated_at: String,
    pub stale: bool,
    /// The same skill as a SKILL.md file, to save as
    /// `<skills folder>/<name>/SKILL.md` for a harness that loads skills from
    /// disk (Claude Code reads .claude/skills).
    pub skill_md: String,
    pub url: String,
}

/// A skill in the SKILL.md format: YAML front matter with the name and
/// description, then the instructions.
pub(super) fn skill_md(name: &str, description: &str, body: &str) -> String {
    let quoted = serde_json::to_string(description).unwrap_or_default();
    format!(
        "---\nname: {name}\ndescription: {quoted}\n---\n\n{}\n",
        body.trim_end()
    )
}

pub(super) async fn skill_get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SkillGetArguments,
) -> ToolResult<SkillValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let page_id = match (arguments.page_id.as_deref(), arguments.name.as_deref()) {
        (Some(value), None) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("pageId", "Pass a page ID or link."),
        },
        (None, Some(_)) => {
            if !project_is_chosen(principal) {
                return ToolResult::invalid(
                    "projectId",
                    "Say which project the skill is in (projectId).",
                );
            }
            None
        }
        _ => return ToolResult::invalid("name", "Pass name or pageId, one of them."),
    };
    let result = async {
        memory::project_access(
            &state.pool,
            principal.actor_id,
            principal.actor_kind,
            principal.project_id,
            Action::McpRead,
        )
        .await?;
        let page_id = match (page_id, arguments.name.as_deref()) {
            (Some(page_id), _) => Some(page_id),
            (None, Some(name)) => {
                let wanted = memory::skill_name(name);
                memory::list_skills(&state.pool, principal.project_id)
                    .await?
                    .into_iter()
                    .find(|page| memory::skill_name(&page.title) == wanted)
                    .map(|page| page.id)
            }
            (None, None) => None,
        };
        let page = match page_id {
            Some(page_id) => memory::get_page(&state.pool, page_id).await?,
            None => None,
        };
        let Some(page) = page.filter(|page| {
            page.project_id == principal.project_id
                && !page.archived
                && page.skill_description.is_some()
        }) else {
            return Ok(ToolResult::not_found("That skill"));
        };
        let name = memory::skill_name(&page.title);
        let description = page.skill_description.clone().unwrap_or_default();
        let (body, truncated) = truncate_text(&page.body, PAGE_BODY_CHARACTERS);
        Ok::<_, AppError>(ToolResult::ok(SkillValue {
            skill_md: skill_md(&name, &description, &body),
            url: public_url(
                state,
                &format!("/projects/{}/pages/{}", page.project_id, page.id),
            ),
            name,
            description,
            body,
            truncated,
            page_id: page.id.to_string(),
            project_id: page.project_id.to_string(),
            version: page.version,
            updated_at: page.updated_at,
            stale: page.stale,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "skill_get"))
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
    /// The message where it was decided: its ID, or a link to it.
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
    /// The decision that replaced this one, if it was superseded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_conversation: Option<String>,
    pub decided_by: String,
    pub decided_at: String,
}

impl From<memory::DecisionView> for DecisionValue {
    fn from(decision: memory::DecisionView) -> Self {
        Self {
            id: decision.id.to_string(),
            project_id: decision.project_id.to_string(),
            title: decision.title,
            detail: decision.detail,
            status: decision.status,
            superseded_by: decision.superseded_by_decision_id.map(|id| id.to_string()),
            source_conversation: decision.source_conversation_name,
            decided_by: decision.decided_by,
            decided_at: decision.created_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DecisionListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Include decisions that were later replaced.
    pub include_superseded: Option<bool>,
    /// How many, newest first (default 50, at most 200).
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct DecisionListValue {
    pub decisions: Vec<DecisionValue>,
}

pub(super) async fn decision_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: DecisionListArguments,
) -> ToolResult<DecisionListValue> {
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
        let decisions = memory::list_decisions(
            &state.pool,
            principal.project_id,
            principal.actor_id,
            arguments.include_superseded.unwrap_or(false),
            i64::from(arguments.limit.unwrap_or(50).clamp(1, 200)),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(DecisionListValue {
            decisions: decisions.into_iter().map(DecisionValue::from).collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "decision_list"))
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
    let Ok(source_message_id) = arguments
        .from_message_id
        .as_deref()
        .map(|value| parse_message_id(value).ok_or(()))
        .transpose()
    else {
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
        if !can_write_in(state, principal, project_id, Action::PageEdit).await? {
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
            Action::PageEdit,
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
        Ok::<_, AppError>(ToolResult::ok(DecisionValue::from(decision)))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "decision_record"))
}

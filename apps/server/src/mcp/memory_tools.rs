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
        public_url, scope_projects, truncate_text,
    },
    talk::can_write_in,
};
use crate::{
    AppState,
    error::AppError,
    memory::{self, DecisionInput, PageInput, Writer},
    page_blocks::{BlockKind, Edit, EditError, EditOp, PageBlocks, ReadError, Selection},
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
    let input = PageInput {
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
// Pages as blocks (AWD-53): read a page's outline and blocks, and change it
// block by block instead of rewriting the whole page.

/// How much of a block page_blocks shows without includeMarkdown.
const BLOCK_PREVIEW_CHARACTERS: usize = 120;

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageBlocksArguments {
    pub page_id: String,
    /// The version your positions came from. Needed with section, from or
    /// to; the read is refused if the page has changed since.
    pub version: Option<i64>,
    /// Read the section under the heading at this position (the heading and
    /// every block up to the next heading at its level or above).
    pub section: Option<usize>,
    /// Read the blocks from this position…
    pub from: Option<usize>,
    /// …to this one, both included (defaults to from).
    pub to: Option<usize>,
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
    /// Positions hold only within this version. Pass it as page_edit's
    /// expectedVersion.
    pub version: i64,
    /// The headings, each with the last block of its section.
    pub outline: Vec<OutlineLine>,
    /// Every block, or the ones you selected.
    pub blocks: Vec<BlockLine>,
    /// Exactly the stored text of a section or range you asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Some Markdown was left out to keep the answer small; read a range.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct OutlineLine {
    pub position: usize,
    pub level: u8,
    pub text: String,
    pub hash: String,
    pub section_end: usize,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct BlockLine {
    /// Where the block is in this version; not a lasting id.
    pub position: usize,
    /// heading, paragraph, code, quote, list, hr, table, or overflow (the
    /// text past what the page draws as Markdown).
    #[serde(rename = "type")]
    pub kind: String,
    /// A heading's level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// Pass it with the position to page_edit; it proves you read this text.
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

fn blocks_value(
    state: &AppState,
    page: &memory::PageView,
    blocks: &PageBlocks<'_>,
    selection: Option<(usize, usize)>,
    include_markdown: bool,
) -> PageBlocksValue {
    let view = blocks.view();
    let mut budget = PAGE_BODY_CHARACTERS;
    let mut truncated = false;
    let (first, last) = selection.unwrap_or((0, view.blocks.len().saturating_sub(1)));
    let whole = selection.is_some() || include_markdown;
    let lines = view
        .blocks
        .iter()
        .filter(|block| {
            !view.blocks.is_empty() && block.position >= first && block.position <= last
        })
        .map(|block| {
            let (preview, markdown) = if whole {
                let length = block.markdown.chars().count();
                if length <= budget {
                    budget -= length;
                    (None, Some(block.markdown.to_owned()))
                } else {
                    truncated = true;
                    let (preview, _) = truncate_text(block.markdown, BLOCK_PREVIEW_CHARACTERS);
                    (Some(preview), None)
                }
            } else {
                let first_line = block.markdown.lines().next().unwrap_or_default();
                let (preview, _) = truncate_text(first_line, BLOCK_PREVIEW_CHARACTERS);
                (Some(preview), None)
            };
            BlockLine {
                position: block.position,
                kind: block.kind.name().to_owned(),
                level: match block.kind {
                    BlockKind::Heading { level, .. } => Some(*level),
                    _ => None,
                },
                hash: block.hash.clone(),
                preview,
                markdown,
            }
        })
        .collect();
    let markdown = selection.map(|(from, to)| {
        let excerpt = &blocks.markdown()[blocks.blocks()[from].start..blocks.blocks()[to].end];
        let (text, cut) = truncate_text(excerpt, PAGE_BODY_CHARACTERS);
        truncated |= cut;
        text
    });
    PageBlocksValue {
        url: public_url(
            state,
            &format!("/projects/{}/pages/{}", page.project_id, page.id),
        ),
        page_id: page.id.to_string(),
        project_id: page.project_id.to_string(),
        title: page.title.clone(),
        version: blocks.version(),
        outline: blocks
            .outline()
            .into_iter()
            .map(|entry| OutlineLine {
                position: entry.position,
                level: entry.level,
                text: entry.text,
                hash: entry.hash,
                section_end: entry.section_end,
            })
            .collect(),
        blocks: lines,
        markdown,
        truncated,
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
    let selection = match (arguments.section, arguments.from, arguments.to) {
        (None, None, None) => None,
        (Some(position), None, None) => Some(Selection::Section {
            version: arguments.version.unwrap_or(-1),
            position,
        }),
        (None, Some(from), to) => Some(Selection::Blocks {
            version: arguments.version.unwrap_or(-1),
            from,
            to: to.unwrap_or(from),
        }),
        (None, None, Some(_)) => {
            return ToolResult::invalid("from", "Pass from with to.");
        }
        _ => {
            return ToolResult::invalid("section", "Pass a section or a range, not both.");
        }
    };
    if selection.is_some() && arguments.version.is_none() {
        return ToolResult::invalid(
            "version",
            "Pass the version your positions came from (page_blocks returns it).",
        );
    }
    let result = async {
        let Some(page) = readable_page(state, principal, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        let blocks = PageBlocks::parse(&page.body, page.version);
        let range = match selection {
            None => None,
            Some(selection) => match blocks.read(selection) {
                Ok(excerpt) => excerpt.blocks,
                Err(error @ ReadError::OtherVersion { .. }) => {
                    return Ok(ToolResult::conflict(error.to_string()));
                }
                Err(error) => {
                    let field = match selection {
                        Selection::Section { .. } => "section",
                        _ => "from",
                    };
                    return Ok(ToolResult::invalid(field, error.to_string()));
                }
            },
        };
        Ok::<_, AppError>(ToolResult::ok(blocks_value(
            state,
            &page,
            &blocks,
            range,
            arguments.include_markdown.unwrap_or(false),
        )))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_blocks"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageEditArguments {
    pub page_id: String,
    /// The version you read the positions and hashes from (page_blocks).
    /// The edit is refused if the page has changed since.
    pub expected_version: i64,
    /// Up to 100 edits, applied together or not at all. Every position
    /// refers to expectedVersion, not to the page after earlier edits.
    pub edits: Vec<PageEditInput>,
    /// A short note on what changed, kept with the version.
    pub note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageEditInput {
    /// replace, insertBefore, insertAfter, delete, or append (to the end of
    /// the page; no position or hash).
    pub op: String,
    /// The block's position, from page_blocks.
    pub position: Option<usize>,
    /// The block's hash, from page_blocks: the edit is refused if its text
    /// has changed.
    pub hash: Option<String>,
    /// The new Markdown, one or more blocks (not for delete).
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
    let mut edits = Vec::with_capacity(arguments.edits.len());
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
        edits.push(Edit {
            op,
            position: edit.position,
            hash: edit.hash,
            markdown: edit.markdown,
        });
    }
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::PageEdit).await? {
            return Ok(ToolResult::denied("You can't edit pages in that project."));
        }
        let mut transaction = state.pool.begin().await?;
        let row = sqlx::query_as::<_, (Uuid, String, i64)>(
            "SELECT project_id, body, version FROM project_pages WHERE id = $1 FOR UPDATE",
        )
        .bind(page_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((owner, body, version)) = row else {
            return Ok(ToolResult::not_found("That page"));
        };
        if owner != project_id {
            return Ok(ToolResult::not_found("That page"));
        }
        let blocks = PageBlocks::parse(&body, version);
        let new_body = match blocks.edit(arguments.expected_version, &edits) {
            Ok(new_body) => new_body,
            Err(error @ (EditError::OtherVersion { .. } | EditError::HashMismatch { .. })) => {
                return Ok(ToolResult::conflict(error.to_string()));
            }
            Err(error) => return Ok(ToolResult::invalid("edits", error.to_string())),
        };
        let input = PageInput {
            title: None,
            body: Some(new_body),
            is_guide: None,
            skill_description: None,
            expected_version: Some(arguments.expected_version),
            restore_version: None,
            archived: None,
            note: arguments.note,
        };
        memory::update_page(&mut transaction, writer(principal), page_id, &input).await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "page.mcp_block_edited",
            "page",
            page_id,
            serde_json::json!({ "edits": edits.len(), "fromVersion": version }),
        )
        .await?;
        transaction.commit().await?;
        let Some(page) = readable_page(state, principal, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        let blocks = PageBlocks::parse(&page.body, page.version);
        Ok::<_, AppError>(ToolResult::ok(blocks_value(
            state, &page, &blocks, None, false,
        )))
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

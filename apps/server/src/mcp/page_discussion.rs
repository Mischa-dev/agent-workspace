//! Comments and suggestions on pages (AWD-53): read a page's comment
//! threads and suggested edits, comment (on words in a block, or the whole
//! page), reply, resolve, and decide suggestions. People see the same in
//! the page's Discussion.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, parse_id, public_url},
    memory_tools::writer,
    talk::can_write_in,
};
use crate::{
    AppState,
    error::AppError,
    memory,
    page_docs::{
        comments::{self, CommentChange, NewComment, ThreadView},
        store,
        suggestions::{self, Decision, Refusal},
    },
    policy::Action,
};

/// The page, when the caller can read it in the project the call is in.
async fn readable(
    state: &AppState,
    principal: &McpPrincipal,
    page_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let project =
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM project_pages WHERE id = $1")
            .bind(page_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some(project_id) = project.filter(|project| *project == principal.project_id) else {
        return Ok(None);
    };
    if memory::project_access(
        &state.pool,
        principal.actor_id,
        principal.actor_kind,
        project_id,
        Action::McpRead,
    )
    .await
    .is_err()
    {
        return Ok(None);
    }
    Ok(Some(project_id))
}

fn page_url(state: &AppState, project_id: Uuid, page_id: Uuid) -> String {
    public_url(state, &format!("/projects/{project_id}/pages/{page_id}"))
}

// ---------------------------------------------------------------------------
// page_comments

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageCommentsArguments {
    pub page_id: String,
    /// Include resolved threads (default false).
    pub include_resolved: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageCommentsValue {
    pub page_id: String,
    pub threads: Vec<ThreadLine>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadLine {
    /// The thread's id; reply with page_comment (replyTo), resolve with
    /// page_comment_update.
    pub id: String,
    /// The block it's about (an id from page_blocks); none for the whole
    /// page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    /// The words it's about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// False when the block or the words are no longer on the page.
    pub anchor_found: bool,
    pub resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    pub comments: Vec<CommentLine>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommentLine {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// human, or the kind of agent.
    pub author_kind: String,
    /// Empty when deleted.
    pub text: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

fn thread_line(thread: ThreadView) -> ThreadLine {
    ThreadLine {
        id: thread.id.to_string(),
        block_id: thread.block_id,
        quote: thread.quote,
        anchor_found: thread.anchor_found,
        resolved: thread.resolved,
        resolved_by: thread.resolved_by,
        comments: thread
            .comments
            .into_iter()
            .map(|comment| CommentLine {
                id: comment.id.to_string(),
                author: comment.author,
                author_kind: comment.author_kind,
                text: comment.body,
                at: comment.created_at,
                edited: comment.edited_at.is_some(),
                deleted: comment.deleted,
            })
            .collect(),
    }
}

pub(super) async fn page_comments(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageCommentsArguments,
) -> ToolResult<PageCommentsValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    let result = async {
        let Some(project_id) = readable(state, principal, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        let blocks = comments::current_blocks(&state.pool, page_id).await?;
        let threads = comments::list(
            &state.pool,
            page_id,
            blocks.as_deref(),
            arguments.include_resolved.unwrap_or(false),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(PageCommentsValue {
            page_id: page_id.to_string(),
            threads: threads.into_iter().map(thread_line).collect(),
            url: page_url(state, project_id, page_id),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_comments"))
}

// ---------------------------------------------------------------------------
// page_comment

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageCommentArguments {
    pub page_id: String,
    /// The comment, in Markdown (up to 10,000 characters).
    pub text: String,
    /// Start a thread about one block: its id from page_blocks. Leave it
    /// out to comment on the whole page.
    pub block_id: Option<String>,
    /// With blockId: the exact words in that block the comment is about.
    pub quote: Option<String>,
    /// Reply in a thread instead: the thread's id, or any comment's in it.
    pub reply_to: Option<String>,
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageCommentValue {
    pub comment_id: String,
    pub thread_id: String,
    pub url: String,
}

pub(super) async fn page_comment(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageCommentArguments,
) -> ToolResult<PageCommentValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    let reply_to = match arguments.reply_to.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("replyTo", "Pass a thread or comment ID."),
        },
        None => None,
    };
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::PageEdit).await? {
            return Ok(ToolResult::denied(
                "You can't comment on pages in that project.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        let Some(page) = store::lock_page(&mut transaction, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        if page.project_id != project_id {
            return Ok(ToolResult::not_found("That page"));
        }
        let created = comments::create(
            &mut transaction,
            writer(principal),
            page_id,
            &page,
            &NewComment {
                body: &arguments.text,
                block_id: arguments.block_id.as_deref(),
                quote: arguments.quote.as_deref(),
                reply_to,
            },
        )
        .await?;
        let (comment_id, thread_id) = match created {
            Ok(created) => created,
            Err(comments::Refusal::NotFound) => {
                return Ok(ToolResult::not_found("That comment thread"));
            }
            Err(comments::Refusal::Invalid(field, message)) => {
                let field = match field {
                    "body" => "text",
                    other => other,
                };
                return Ok(ToolResult::invalid(field, message));
            }
            Err(comments::Refusal::NotYours(message)) => return Ok(ToolResult::denied(message)),
        };
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "page.comment_created",
            "page",
            page_id,
            serde_json::json!({ "commentId": comment_id }),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(PageCommentValue {
            comment_id: comment_id.to_string(),
            thread_id: thread_id.to_string(),
            url: page_url(state, project_id, page_id),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_comment"))
}

// ---------------------------------------------------------------------------
// page_comment_update

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageCommentUpdateArguments {
    pub page_id: String,
    /// The comment (for resolve, any comment in the thread).
    pub comment_id: String,
    /// New words for your own comment.
    pub text: Option<String>,
    /// Resolve (true) or reopen (false) the comment's thread.
    pub resolved: Option<bool>,
    /// Delete your own comment.
    pub delete: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageCommentUpdateValue {
    pub comment_id: String,
    pub thread_id: String,
    pub url: String,
}

pub(super) async fn page_comment_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageCommentUpdateArguments,
) -> ToolResult<PageCommentUpdateValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    let Some(comment_id) = parse_id(&arguments.comment_id) else {
        return ToolResult::invalid("commentId", "Pass a comment ID.");
    };
    let change = match (
        arguments.text.as_deref(),
        arguments.resolved,
        arguments.delete,
    ) {
        (Some(text), None, None | Some(false)) => CommentChange::Edit(text),
        (None, Some(resolved), None | Some(false)) => CommentChange::Resolve(resolved),
        (None, None, Some(true)) => CommentChange::Delete,
        _ => {
            return ToolResult::invalid(
                "text",
                "Pass one of text (edit your comment), resolved, or delete: true.",
            );
        }
    };
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::PageEdit).await? {
            return Ok(ToolResult::denied(
                "You can't comment on pages in that project.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        let Some(page) = store::lock_page(&mut transaction, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        if page.project_id != project_id {
            return Ok(ToolResult::not_found("That page"));
        }
        let thread_id = match comments::change(
            &mut transaction,
            writer(principal),
            project_id,
            page_id,
            comment_id,
            change,
        )
        .await?
        {
            Ok(thread_id) => thread_id,
            Err(comments::Refusal::NotFound) => return Ok(ToolResult::not_found("That comment")),
            Err(comments::Refusal::Invalid(field, message)) => {
                let field = match field {
                    "body" => "text",
                    other => other,
                };
                return Ok(ToolResult::invalid(field, message));
            }
            Err(comments::Refusal::NotYours(message)) => return Ok(ToolResult::denied(message)),
        };
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "page.comment_updated",
            "page",
            page_id,
            serde_json::json!({ "commentId": comment_id }),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(PageCommentUpdateValue {
            comment_id: comment_id.to_string(),
            thread_id: thread_id.to_string(),
            url: page_url(state, project_id, page_id),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_comment_update"))
}

// ---------------------------------------------------------------------------
// page_suggestions

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageSuggestionsArguments {
    pub page_id: String,
    /// Also list the latest decided ones (accepted, rejected, withdrawn).
    pub include_decided: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageSuggestionsValue {
    pub page_id: String,
    pub suggestions: Vec<SuggestionLine>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SuggestionLine {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub author_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// pending, accepted, rejected or withdrawn.
    pub status: String,
    /// The page's version it was made against.
    pub base_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// The page's version accepting it made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_version: Option<i64>,
    pub at: String,
    pub edits: Vec<SuggestedEdit>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SuggestedEdit {
    /// replace, insertBefore, insertAfter, delete or append.
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    /// The new Markdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// The block's Markdown when it was suggested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

pub(super) async fn page_suggestions(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageSuggestionsArguments,
) -> ToolResult<PageSuggestionsValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    let result = async {
        let Some(project_id) = readable(state, principal, page_id).await? else {
            return Ok(ToolResult::not_found("That page"));
        };
        let rows = suggestions::list(
            &state.pool,
            page_id,
            arguments.include_decided.unwrap_or(false),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(PageSuggestionsValue {
            page_id: page_id.to_string(),
            suggestions: rows
                .into_iter()
                .map(|row| SuggestionLine {
                    id: row.id.to_string(),
                    author: row.author,
                    author_id: row.author_id.to_string(),
                    note: row.note,
                    status: row.status,
                    base_version: row.base_version,
                    decided_by: row.decided_by,
                    applied_version: row.applied_version,
                    at: row.created_at,
                    edits: row
                        .edits
                        .0
                        .into_iter()
                        .map(|edit| SuggestedEdit {
                            op: edit.op,
                            block_id: edit.block_id,
                            markdown: edit.markdown,
                            before: edit.before,
                        })
                        .collect(),
                })
                .collect(),
            url: page_url(state, project_id, page_id),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_suggestions"))
}

// ---------------------------------------------------------------------------
// page_suggestion_decide

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageSuggestionDecideArguments {
    pub page_id: String,
    pub suggestion_id: String,
    /// accept or reject (on a page you made), or withdraw (your own
    /// suggestion).
    pub decision: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PageSuggestionDecideValue {
    pub suggestion_id: String,
    /// accepted, rejected or withdrawn.
    pub status: String,
    /// The page's version now.
    pub version: i64,
    pub url: String,
}

pub(super) async fn page_suggestion_decide(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PageSuggestionDecideArguments,
) -> ToolResult<PageSuggestionDecideValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(page_id) = parse_id(&arguments.page_id) else {
        return ToolResult::invalid("pageId", "Pass a page ID or link.");
    };
    let Some(suggestion_id) = parse_id(&arguments.suggestion_id) else {
        return ToolResult::invalid("suggestionId", "Pass a suggestion ID.");
    };
    let decision = match arguments.decision.as_str() {
        "accept" => Decision::Accept,
        "reject" => Decision::Reject,
        "withdraw" => Decision::Withdraw,
        _ => return ToolResult::invalid("decision", "decision is accept, reject or withdraw"),
    };
    let result = async {
        let project_id = principal.project_id;
        if !can_write_in(state, principal, project_id, Action::PageEdit).await? {
            return Ok(ToolResult::denied("You can't edit pages in that project."));
        }
        let in_project = sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM project_pages WHERE id = $1")
            .bind(page_id)
            .fetch_optional(&state.pool)
            .await?
            == Some(project_id);
        if !in_project {
            return Ok(ToolResult::not_found("That page"));
        }
        let mut transaction = state.pool.begin().await?;
        let decided = match suggestions::decide(
            &mut transaction,
            writer(principal),
            page_id,
            suggestion_id,
            decision,
        )
        .await?
        {
            Ok(decided) => decided,
            Err(Refusal::NotFound) => return Ok(ToolResult::not_found("That suggestion")),
            Err(Refusal::NotYours(message)) => return Ok(ToolResult::denied(message)),
            Err(Refusal::AlreadyDecided(status)) => {
                return Ok(ToolResult::conflict(format!(
                    "That suggestion was already {status}."
                )));
            }
            Err(Refusal::Archived) => {
                return Ok(ToolResult::invalid(
                    "pageId",
                    "That page is archived. Bring it back with page_write (archived: false) first.",
                ));
            }
            Err(Refusal::Stale(why)) => {
                return Ok(ToolResult::conflict(format!(
                    "{why} The suggestion no longer fits the page; reject it, or (if it's yours) withdraw it and suggest again from page_blocks."
                )));
            }
        };
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            match decided.status {
                "accepted" => "page.suggestion_accepted",
                "rejected" => "page.suggestion_rejected",
                _ => "page.suggestion_withdrawn",
            },
            "page",
            page_id,
            serde_json::json!({ "suggestionId": suggestion_id }),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(PageSuggestionDecideValue {
            suggestion_id: suggestion_id.to_string(),
            status: decided.status.to_owned(),
            version: decided.version,
            url: page_url(state, project_id, page_id),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "page_suggestion_decide"))
}

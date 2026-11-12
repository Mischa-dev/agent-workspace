//! Comments on pages (AWD-53, criterion 7): threads, each started by a
//! comment that may be anchored to words in a block, with replies, and
//! resolved or reopened as a whole. People and agents comment the same way.
//!
//! An anchor is a top-level block's lasting id and the words it's about
//! (the quote). The block's id survives edits, so the anchor follows the
//! block wherever it moves; if the words are edited away or the block is
//! deleted, the thread stays, showing what it was about, and is marked as
//! no longer found on the page.

use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    store,
    suggestions::{discussion_updated, notify},
    tree::{Block, Node, plain_text},
    ydoc,
};
use crate::{
    error::AppError,
    memory::Writer,
    notifications::{self, NewNotification},
};

pub const MAX_COMMENT: usize = 10_000;
pub const MAX_QUOTE: usize = 1_000;
/// The most comments one page keeps.
const MAX_COMMENTS_PER_PAGE: i64 = 5_000;

/// A block's words, each paragraph, cell and item on a line of its own.
pub fn block_text(block: &Block) -> String {
    fn walk(block: &Block, out: &mut String) {
        let line = |text: &str, out: &mut String| {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        };
        match &block.node {
            Node::Paragraph(content) | Node::Heading { content, .. } => {
                line(&plain_text(content), out);
            }
            Node::CodeBlock { text, .. } => line(text, out),
            Node::Blockquote(blocks) => blocks.iter().for_each(|block| walk(block, out)),
            Node::BulletList(items) | Node::OrderedList { items, .. } => items
                .iter()
                .flat_map(|item| &item.content)
                .for_each(|block| walk(block, out)),
            Node::HorizontalRule => {}
            Node::Table(rows) => rows
                .iter()
                .flatten()
                .for_each(|cell| line(&plain_text(&cell.content), out)),
            Node::Embed { label, .. } => line(label, out),
        }
    }
    let mut out = String::new();
    walk(block, &mut out);
    out
}

/// Text with every run of white space as one space, trimmed: how quotes
/// are compared, since the editor and the server break lines differently.
pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the quote is still in the block with that id.
pub fn anchor_found(blocks: &[Block], block_id: &str, quote: Option<&str>) -> bool {
    let Some(block) = blocks
        .iter()
        .find(|block| block.id.as_deref() == Some(block_id))
    else {
        return false;
    };
    quote.is_none_or(|quote| squash(&block_text(block)).contains(&squash(quote)))
}

/// What's wrong with an anchor someone asks for, in words for them.
pub fn check_anchor(
    blocks: &[Block],
    block_id: &str,
    quote: Option<&str>,
) -> Result<(), &'static str> {
    if !ydoc::valid_block_id(block_id) {
        return Err("Pass a block's id from page_blocks.");
    }
    if !blocks
        .iter()
        .any(|block| block.id.as_deref() == Some(block_id))
    {
        return Err("There's no block with that id on the page now; read it again.");
    }
    if let Some(quote) = quote {
        if squash(quote).is_empty() {
            return Err("The quote needs some words in it, or leave it out.");
        }
        if quote.chars().count() > MAX_QUOTE {
            return Err("Keep the quote under 1,000 characters.");
        }
        if !anchor_found(blocks, block_id, Some(quote)) {
            return Err(
                "Those words aren't in that block; quote them exactly as the block has them.",
            );
        }
    }
    Ok(())
}

pub fn check_body(body: &str) -> Result<String, &'static str> {
    let body = body.trim();
    if body.is_empty() {
        return Err("Write something in the comment.");
    }
    if body.chars().count() > MAX_COMMENT {
        return Err("Keep a comment under 10,000 characters.");
    }
    if body
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err("Take the control characters out of the comment.");
    }
    Ok(body.to_owned())
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, FromRow)]
struct CommentRow {
    id: Uuid,
    thread_id: Uuid,
    author_id: Uuid,
    author: Option<String>,
    author_kind: String,
    via: Option<String>,
    body: String,
    block_id: Option<String>,
    quote: Option<String>,
    resolved_by: Option<String>,
    resolved_at: Option<String>,
    created_at: String,
    edited_at: Option<String>,
    deleted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentView {
    pub id: Uuid,
    pub author_id: Uuid,
    pub author: Option<String>,
    /// human, or the kind of agent.
    pub author_kind: String,
    /// The connected app a person wrote it through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Empty when it was deleted.
    pub body: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    /// The thread's id: its first comment's.
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// Whether what it's anchored to is still on the page (always true for
    /// a comment on the whole page).
    pub anchor_found: bool,
    pub resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    pub comments: Vec<CommentView>,
}

/// A page's threads, oldest first. `blocks` is the page's document as it is
/// now, to say whether each anchor is still found; resolved threads only if
/// asked.
pub async fn list(
    pool: &PgPool,
    page_id: Uuid,
    blocks: Option<&[Block]>,
    include_resolved: bool,
) -> Result<Vec<ThreadView>, AppError> {
    let rows = sqlx::query_as::<_, CommentRow>(
        r#"
        SELECT comment.id, comment.thread_id, comment.author_actor_id AS author_id,
               author.display_name AS author, actor.kind AS author_kind,
               via_client.client_name AS via,
               comment.body, thread.block_id, thread.quote,
               resolver.display_name AS resolved_by,
               to_char(thread.resolved_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS resolved_at,
               to_char(comment.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
               to_char(comment.edited_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS edited_at,
               comment.deleted_at IS NOT NULL AS deleted
        FROM project_page_comments comment
        JOIN project_page_comments thread ON thread.id = comment.thread_id
        JOIN actors actor ON actor.id = comment.author_actor_id
        LEFT JOIN project_memberships author
          ON author.project_id = comment.project_id AND author.actor_id = comment.author_actor_id
        LEFT JOIN project_memberships resolver
          ON resolver.project_id = comment.project_id AND resolver.actor_id = thread.resolved_by_actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = comment.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        WHERE comment.page_id = $1 AND ($2 OR thread.resolved_at IS NULL)
        ORDER BY thread.created_at, thread.id, comment.created_at, comment.id
        LIMIT 5000
        "#,
    )
    .bind(page_id)
    .bind(include_resolved)
    .fetch_all(pool)
    .await?;
    let mut threads: Vec<ThreadView> = Vec::new();
    for row in rows {
        if threads
            .last()
            .is_none_or(|thread| thread.id != row.thread_id)
        {
            let anchor_found = match (&row.block_id, blocks) {
                (Some(block_id), Some(blocks)) => {
                    anchor_found(blocks, block_id, row.quote.as_deref())
                }
                (Some(_), None) => false,
                (None, _) => true,
            };
            threads.push(ThreadView {
                id: row.thread_id,
                block_id: row.block_id.clone(),
                quote: row.quote.clone(),
                anchor_found,
                resolved: row.resolved_at.is_some(),
                resolved_by: row.resolved_by.clone(),
                resolved_at: row.resolved_at.clone(),
                comments: Vec::new(),
            });
        }
        let thread = threads.last_mut().expect("pushed above");
        thread.comments.push(CommentView {
            id: row.id,
            author_id: row.author_id,
            author: row.author,
            author_kind: row.author_kind,
            via: row.via,
            body: row.body,
            created_at: row.created_at,
            edited_at: row.edited_at,
            deleted: row.deleted,
        });
    }
    // A deleted comment shows only to hold its thread's replies together.
    for thread in &mut threads {
        let first = thread.comments.first().map(|comment| comment.id);
        thread
            .comments
            .retain(|comment| !comment.deleted || Some(comment.id) == first);
    }
    threads.retain(|thread| thread.comments.iter().any(|comment| !comment.deleted));
    Ok(threads)
}

/// The page's blocks now, if it has a document.
pub async fn current_blocks(pool: &PgPool, page_id: Uuid) -> Result<Option<Vec<Block>>, AppError> {
    match store::snapshot(pool, page_id).await? {
        Some(snapshot) => Ok(Some(store::read_blocks(&snapshot.state)?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Writing

/// A new comment: a thread's first (maybe anchored), or a reply.
pub struct NewComment<'a> {
    pub body: &'a str,
    pub block_id: Option<&'a str>,
    pub quote: Option<&'a str>,
    pub reply_to: Option<Uuid>,
}

/// Why a comment change wasn't made.
#[derive(Debug)]
pub enum Refusal {
    NotFound,
    Invalid(&'static str, &'static str),
    NotYours(&'static str),
}

impl Refusal {
    pub fn into_error(self) -> AppError {
        match self {
            Self::NotFound => AppError::NotFound("comment"),
            Self::NotYours(message) => AppError::Refused {
                field: "comment",
                message,
            },
            Self::Invalid(field, message) => AppError::Refused { field, message },
        }
    }
}

/// Adds a comment to a page. Call with the page locked; access is checked
/// by the caller. Returns the comment's id and its thread's.
pub async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    page_id: Uuid,
    page: &store::LockedPage,
    comment: &NewComment<'_>,
) -> Result<Result<(Uuid, Uuid), Refusal>, AppError> {
    let body = match check_body(comment.body) {
        Ok(body) => body,
        Err(message) => return Ok(Err(Refusal::Invalid("body", message))),
    };
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM project_page_comments WHERE page_id = $1",
    )
    .bind(page_id)
    .fetch_one(&mut **transaction)
    .await?;
    if count >= MAX_COMMENTS_PER_PAGE {
        return Ok(Err(Refusal::Invalid(
            "body",
            "This page has as many comments as it can keep. Resolve and move the discussion elsewhere.",
        )));
    }
    let id = Uuid::now_v7();
    let thread_id = match comment.reply_to {
        Some(reply_to) => {
            if comment.block_id.is_some() || comment.quote.is_some() {
                return Ok(Err(Refusal::Invalid(
                    "blockId",
                    "A reply goes in its thread, which has the anchor; leave out blockId and quote.",
                )));
            }
            let thread = sqlx::query_scalar::<_, Uuid>(
                "SELECT thread_id FROM project_page_comments WHERE id = $1 AND page_id = $2",
            )
            .bind(reply_to)
            .bind(page_id)
            .fetch_optional(&mut **transaction)
            .await?;
            match thread {
                Some(thread) => thread,
                None => return Ok(Err(Refusal::NotFound)),
            }
        }
        None => {
            if comment.quote.is_some() && comment.block_id.is_none() {
                return Ok(Err(Refusal::Invalid(
                    "quote",
                    "A quote needs the block it's in (blockId).",
                )));
            }
            if let Some(block_id) = comment.block_id {
                let stored = store::document(transaction, page_id, page).await?;
                let blocks = store::read_blocks(&stored.state)?;
                if let Err(message) = check_anchor(&blocks, block_id, comment.quote) {
                    return Ok(Err(Refusal::Invalid(
                        if message.contains("quote") || message.contains("words") {
                            "quote"
                        } else {
                            "blockId"
                        },
                        message,
                    )));
                }
            }
            id
        }
    };
    sqlx::query(
        r#"
        INSERT INTO project_page_comments (
            id, project_id, page_id, thread_id, author_actor_id, via_oauth_grant_id,
            body, block_id, quote
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(id)
    .bind(page.project_id)
    .bind(page_id)
    .bind(thread_id)
    .bind(writer.actor_id)
    .bind(writer.via_grant_id)
    .bind(&body)
    .bind(
        comment
            .reply_to
            .is_none()
            .then_some(comment.block_id)
            .flatten(),
    )
    .bind(
        comment
            .reply_to
            .is_none()
            .then(|| comment.quote.map(|quote| quote.trim().to_owned()))
            .flatten(),
    )
    .execute(&mut **transaction)
    .await?;
    // A reply reopens its thread.
    if comment.reply_to.is_some() {
        sqlx::query(
            "UPDATE project_page_comments SET resolved_at = NULL, resolved_by_actor_id = NULL WHERE id = $1",
        )
        .bind(thread_id)
        .execute(&mut **transaction)
        .await?;
    }
    // Tell the page's maker and everyone else in the thread.
    let mut recipients = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT DISTINCT author_actor_id FROM project_page_comments
        WHERE thread_id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(thread_id)
    .fetch_all(&mut **transaction)
    .await?;
    if let Some(owner) = super::suggestions::page_creator(transaction, page_id).await? {
        recipients.push(owner);
    }
    recipients.sort();
    recipients.dedup();
    for recipient in recipients {
        let mut notification = NewNotification::page(
            page.project_id,
            recipient,
            notifications::kind::PAGE_COMMENT,
            page_id,
            format!("page-comments:{page_id}"),
            format!("page-comment:{id}"),
        );
        notification.actor_id = Some(writer.actor_id);
        let preview: String = body.chars().take(140).collect();
        notification.detail = serde_json::json!({
            "title": format!("New comment on {}", page.title),
            "preview": preview,
            "href": format!("/projects/{}/pages/{page_id}#comment-{thread_id}", page.project_id),
        });
        notify(transaction, &notification).await?;
    }
    discussion_updated(transaction, page.project_id, page_id).await?;
    Ok(Ok((id, thread_id)))
}

/// A change to a comment: new words (the author's own), resolve or reopen
/// its thread (anyone who may comment), or delete it (the author's own).
pub enum CommentChange<'a> {
    Edit(&'a str),
    Resolve(bool),
    Delete,
}

/// Changes a comment on `page_id`. Returns its thread's id.
pub async fn change(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    project_id: Uuid,
    page_id: Uuid,
    comment_id: Uuid,
    change: CommentChange<'_>,
) -> Result<Result<Uuid, Refusal>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, bool)>(
        r#"
        SELECT thread_id, author_actor_id, deleted_at IS NOT NULL
        FROM project_page_comments WHERE id = $1 AND page_id = $2 FOR UPDATE
        "#,
    )
    .bind(comment_id)
    .bind(page_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((thread_id, author, deleted)) = row else {
        return Ok(Err(Refusal::NotFound));
    };
    // A deleted first comment stays as the thread's tombstone while replies
    // remain, and the thread still resolves and reopens through it.
    let resolving_thread = matches!(change, CommentChange::Resolve(_)) && thread_id == comment_id;
    if deleted && !resolving_thread {
        return Ok(Err(Refusal::NotFound));
    }
    match change {
        CommentChange::Edit(body) => {
            if author != writer.actor_id {
                return Ok(Err(Refusal::NotYours(
                    "You can only edit your own comments.",
                )));
            }
            let body = match check_body(body) {
                Ok(body) => body,
                Err(message) => return Ok(Err(Refusal::Invalid("body", message))),
            };
            sqlx::query(
                "UPDATE project_page_comments SET body = $2, edited_at = now() WHERE id = $1",
            )
            .bind(comment_id)
            .bind(body)
            .execute(&mut **transaction)
            .await?;
        }
        CommentChange::Resolve(resolved) => {
            sqlx::query(
                r#"
                UPDATE project_page_comments
                SET resolved_at = CASE WHEN $2 THEN COALESCE(resolved_at, now()) END,
                    resolved_by_actor_id = CASE WHEN $2 THEN COALESCE(resolved_by_actor_id, $3) END
                WHERE id = $1
                "#,
            )
            .bind(thread_id)
            .bind(resolved)
            .bind(writer.actor_id)
            .execute(&mut **transaction)
            .await?;
        }
        CommentChange::Delete => {
            if author != writer.actor_id {
                return Ok(Err(Refusal::NotYours(
                    "You can only delete your own comments.",
                )));
            }
            sqlx::query(
                "UPDATE project_page_comments SET body = '', deleted_at = now() WHERE id = $1",
            )
            .bind(comment_id)
            .execute(&mut **transaction)
            .await?;
        }
    }
    discussion_updated(transaction, project_id, page_id).await?;
    Ok(Ok(thread_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_docs::tree::from_markdown;

    fn page() -> Vec<Block> {
        let mut blocks =
            from_markdown("# Plan\n\nShip the **first** slice\nthis week.\n\n- one\n- two\n");
        for (index, block) in blocks.iter_mut().enumerate() {
            block.id = Some(format!("block{index}"));
        }
        blocks
    }

    #[test]
    fn quotes_match_across_marks_and_line_breaks() {
        let blocks = page();
        assert!(anchor_found(&blocks, "block1", Some("first slice this")));
        assert!(anchor_found(&blocks, "block2", Some("one two")));
        assert!(!anchor_found(&blocks, "block1", Some("second")));
        assert!(!anchor_found(&blocks, "block9", None));
        assert!(anchor_found(&blocks, "block0", None));
    }

    #[test]
    fn anchors_are_checked() {
        let blocks = page();
        assert!(check_anchor(&blocks, "block1", Some("first slice")).is_ok());
        assert!(check_anchor(&blocks, "block1", Some("nope")).is_err());
        assert!(check_anchor(&blocks, "nothere", None).is_err());
        assert!(check_anchor(&blocks, "1", None).is_err());
        assert!(check_anchor(&blocks, "block1", Some("   ")).is_err());
    }

    #[test]
    fn bodies_are_checked() {
        assert_eq!(check_body("  hi  ").as_deref(), Ok("hi"));
        assert!(check_body(" ").is_err());
        assert!(check_body("a\u{0007}b").is_err());
        assert!(check_body("line\nline").is_ok());
    }
}

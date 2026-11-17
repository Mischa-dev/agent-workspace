//! Comments on pages (AWD-53, criterion 7): threads, each started by a
//! comment that may be anchored to words in a block, with replies, and
//! resolved or reopened as a whole. People and agents comment the same way.
//!
//! Since C1.5 a page's comments are its discussion (`crate::discussion`):
//! each thread is a thread conversation under the page's comments, its
//! anchor `{"kind": "text", "blockId", "quote"}`, and each comment a
//! message, sent through the one send pipeline (mentions, followers'
//! notifications, wake-ups) and edited or removed through
//! `message_edits`. This module keeps the page's own shapes: the browser's
//! Discussion panel and the `page_comment*` tools read and write through it.
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
    suggestions::discussion_updated,
    tree::{Block, Node, plain_text},
    ydoc,
};
use crate::{
    AppState,
    conversations::{
        MessageCommandActor, ThreadPatch, parse_actor_kind, threads::ThreadCreator, update_thread,
    },
    discussion::{self, CommentBody, CommentPlace, SubjectKind},
    error::AppError,
    idempotency::IdempotencyKey,
    message_content::MessageContent,
    message_edits::{self, Change, ChangeError, Refused},
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
    resolved: bool,
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
    /// The thread's id (its conversation's).
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

/// The status a resolved thread has, as the discussion's threads use it.
const RESOLVED: &str = "Resolved";
const OPEN: &str = "Open";

/// A page's threads, oldest first. `blocks` is the page's document as it is
/// now, to say whether each anchor is still found; resolved threads only if
/// asked. The caller has checked that the reader may read the page.
pub async fn list(
    pool: &PgPool,
    page_id: Uuid,
    blocks: Option<&[Block]>,
    include_resolved: bool,
) -> Result<Vec<ThreadView>, AppError> {
    let rows = sqlx::query_as::<_, CommentRow>(
        r#"
        -- visibility: the threads under a page's comments, which whoever may read the
        -- page may read (`subject` visibility); the caller checked the page
        SELECT message.id, thread.id AS thread_id, message.author_actor_id AS author_id,
               author.display_name AS author, actor.kind AS author_kind,
               via_client.client_name AS via,
               message.plain_text AS body,
               thread.anchor->>'blockId' AS block_id, thread.anchor->>'quote' AS quote,
               lower(COALESCE(thread.thread_status, '')) = 'resolved' AS resolved,
               CASE WHEN lower(COALESCE(thread.thread_status, '')) = 'resolved'
                    THEN resolver.display_name END AS resolved_by,
               CASE WHEN lower(COALESCE(thread.thread_status, '')) = 'resolved'
                    THEN to_char(thread.thread_status_changed_at AT TIME ZONE 'UTC',
                                 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') END AS resolved_at,
               to_char(message.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
               to_char(message.edited_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS edited_at,
               message.removed_at IS NOT NULL AS deleted
        FROM conversations comments
        JOIN conversations thread
          ON thread.parent_channel_id = comments.id AND thread.kind = 'thread'
        JOIN messages message ON message.conversation_id = thread.id
        JOIN actors actor ON actor.id = message.author_actor_id
        LEFT JOIN project_memberships author
          ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
        LEFT JOIN project_memberships resolver
          ON resolver.project_id = thread.project_id
         AND resolver.actor_id = thread.thread_status_changed_by_actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = message.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        WHERE comments.kind = 'comments' AND comments.subject_kind = 'page'
          AND comments.subject_id = $1
          AND ($2 OR lower(COALESCE(thread.thread_status, '')) <> 'resolved')
        ORDER BY thread.created_at, thread.id, message.created_at, message.id
        LIMIT 5000
        "#,
    )
    .bind(page_id)
    .bind(include_resolved)
    .fetch_all(crate::db::scoped(pool))
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
                resolved: row.resolved,
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

/// Who is commenting: how the message is sent, and who opens a new thread.
pub(crate) struct Commenter {
    pub(crate) actor: MessageCommandActor,
    pub(crate) creator: ThreadCreator,
}

/// A new comment: a thread's first (maybe anchored), or a reply.
pub struct NewComment<'a> {
    pub body: &'a str,
    pub block_id: Option<&'a str>,
    pub quote: Option<&'a str>,
    /// The thread's id, or any comment's in it.
    pub reply_to: Option<Uuid>,
    /// Makes a retry find the comment it made; a fresh one when absent.
    pub key: Option<IdempotencyKey>,
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

/// The thread under `page_id`'s comments that `id` names: the thread
/// itself, or a comment in it.
async fn thread_of(
    connection: &mut sqlx::PgConnection,
    page_id: Uuid,
    id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        -- visibility: a thread under a page's comments; the caller checked the page
        SELECT thread.id, thread.project_id
        FROM conversations comments
        JOIN conversations thread
          ON thread.parent_channel_id = comments.id AND thread.kind = 'thread'
        WHERE comments.kind = 'comments' AND comments.subject_kind = 'page'
          AND comments.subject_id = $1
          AND (thread.id = $2 OR EXISTS (
                SELECT 1 FROM messages message
                WHERE message.id = $2 AND message.conversation_id = thread.id))
        "#,
    )
    .bind(page_id)
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// Adds a comment to a page through its discussion. Access to the page is
/// checked by the caller. Returns the comment's id and its thread's.
pub(crate) async fn create(
    state: &AppState,
    commenter: Commenter,
    page_id: Uuid,
    comment: NewComment<'_>,
) -> Result<Result<(Uuid, Uuid), Refusal>, AppError> {
    let body = match check_body(comment.body) {
        Ok(body) => body,
        Err(message) => return Ok(Err(Refusal::Invalid("body", message))),
    };
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let count = sqlx::query_scalar::<_, i64>(
        r#"
        -- visibility: a count only, of a page's discussion the caller may comment on
        SELECT count(*)
        FROM conversations comments
        JOIN conversations place ON place.id = comments.id OR place.parent_channel_id = comments.id
        JOIN messages message ON message.conversation_id = place.id
        WHERE comments.kind = 'comments' AND comments.subject_kind = 'page'
          AND comments.subject_id = $1
        "#,
    )
    .bind(page_id)
    .fetch_one(&mut *connection)
    .await?;
    if count >= MAX_COMMENTS_PER_PAGE {
        return Ok(Err(Refusal::Invalid(
            "body",
            "This page has as many comments as it can keep. Resolve and move the discussion elsewhere.",
        )));
    }
    let (place, reopen) = match comment.reply_to {
        Some(reply_to) => {
            if comment.block_id.is_some() || comment.quote.is_some() {
                return Ok(Err(Refusal::Invalid(
                    "blockId",
                    "A reply goes in its thread, which has the anchor; leave out blockId and quote.",
                )));
            }
            match thread_of(&mut connection, page_id, reply_to).await? {
                Some((thread_id, _)) => (CommentPlace::Thread(thread_id), Some(thread_id)),
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
            let anchor = match comment.block_id {
                Some(block_id) => {
                    let blocks = current_blocks(&state.pool, page_id)
                        .await?
                        .unwrap_or_default();
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
                    let mut anchor = serde_json::json!({ "kind": "text", "blockId": block_id });
                    if let Some(quote) = comment.quote {
                        anchor["quote"] = serde_json::Value::String(quote.trim().to_owned());
                    }
                    Some(anchor)
                }
                None => None,
            };
            (CommentPlace::NewThread { anchor }, None)
        }
    };
    connection.commit().await?;
    let key = match comment.key {
        Some(key) => key,
        None => IdempotencyKey::parse(&format!("page-comment-{}", Uuid::now_v7()))?,
    };
    let actor_id = commenter.actor.actor_id;
    let actor_kind = parse_actor_kind(&commenter.actor.actor_kind)?;
    let posted = match discussion::post(
        state,
        commenter.actor,
        commenter.creator,
        SubjectKind::Page,
        page_id,
        place,
        key,
        CommentBody::Text(body),
        &[],
        None,
    )
    .await
    {
        Ok(posted) => posted,
        Err(AppError::NotFound("thread")) => return Ok(Err(Refusal::NotFound)),
        Err(error) => return Err(error),
    };
    let thread_id = posted.thread_id.ok_or(AppError::NotFound("thread"))?;
    // A reply reopens its thread; open pages hear of the change.
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    if let Some(thread_id) = reopen {
        set_resolved(&mut transaction, actor_id, actor_kind, thread_id, false).await?;
    }
    let project_id = sqlx::query_scalar::<_, Uuid>(
        "-- visibility: the thread just posted to\nSELECT project_id FROM conversations WHERE id = $1",
    )
    .bind(thread_id)
    .fetch_one(&mut *transaction)
    .await?;
    discussion_updated(&mut transaction, project_id, page_id).await?;
    transaction.commit().await?;
    Ok(Ok((posted.message_id, thread_id)))
}

/// Resolves or reopens a thread, when it isn't already so.
async fn set_resolved(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: crate::policy::ActorKind,
    thread_id: Uuid,
    resolved: bool,
) -> Result<(), AppError> {
    let status = sqlx::query_scalar::<_, Option<String>>(
        "-- visibility: a thread under a page's comments the caller may comment on\nSELECT thread_status FROM conversations WHERE id = $1",
    )
    .bind(thread_id)
    .fetch_one(&mut **transaction)
    .await?;
    let is_resolved = status.is_some_and(|status| status.eq_ignore_ascii_case(RESOLVED));
    if is_resolved == resolved {
        return Ok(());
    }
    let patch = ThreadPatch {
        name: None,
        status: Some(if resolved { RESOLVED } else { OPEN }.to_owned()),
        outcome: None,
        clear_outcome: false,
    };
    update_thread(transaction, actor_id, actor_kind, thread_id, &patch).await?;
    Ok(())
}

/// A change to a comment: new words (the author's own), resolve or reopen
/// its thread (anyone who may comment), or delete it (the author's own, or
/// as someone who moderates the space).
pub enum CommentChange<'a> {
    Edit(&'a str),
    Resolve(bool),
    Delete,
}

fn refused(error: ChangeError, change: Change) -> Result<Refusal, AppError> {
    match error {
        ChangeError::App(AppError::NotFound(_)) => Ok(Refusal::NotFound),
        ChangeError::App(error) => Err(error),
        ChangeError::Refused(refused) => Ok(match refused {
            Refused::NotYourMessage | Refused::OnlyPeopleModerate if change == Change::Remove => {
                Refusal::NotYours("You can only delete your own comments.")
            }
            Refused::NotYourMessage | Refused::OnlyPeopleModerate => {
                Refusal::NotYours("You can only edit your own comments.")
            }
            Refused::RoleCannotWrite => return Err(AppError::Forbidden),
            Refused::Removed => Refusal::NotFound,
            Refused::Archived => Refusal::Invalid(
                "comment",
                "The page is archived, so its comments can't change.",
            ),
            Refused::RevisionMoved(_) => {
                Refusal::Invalid("body", "The comment changed meanwhile; read it again.")
            }
            Refused::TooManyEdits => Refusal::Invalid(
                "body",
                "That comment has been edited too many times. Add a new comment instead.",
            ),
        }),
    }
}

/// Changes a comment on `page_id` (`comment_id` may name the thread
/// itself, for its first comment). Returns its thread's id.
pub(crate) async fn change(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MessageCommandActor,
    page_id: Uuid,
    comment_id: Uuid,
    change: CommentChange<'_>,
) -> Result<Result<Uuid, Refusal>, AppError> {
    let Some((thread_id, project_id)) = thread_of(transaction, page_id, comment_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    // The thread's id names its first comment, as it did when a thread was
    // its first comment.
    let comment_id = if comment_id == thread_id {
        match sqlx::query_scalar::<_, Uuid>(
            "-- visibility: a thread under a page's comments; the caller checked the page\n\
             SELECT id FROM messages WHERE conversation_id = $1 ORDER BY created_at, id LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(&mut **transaction)
        .await?
        {
            Some(first) => first,
            None => return Ok(Err(Refusal::NotFound)),
        }
    } else {
        comment_id
    };
    match change {
        CommentChange::Resolve(resolved) => {
            let actor_kind = parse_actor_kind(&actor.actor_kind)?;
            set_resolved(transaction, actor.actor_id, actor_kind, thread_id, resolved).await?;
        }
        CommentChange::Edit(body) => {
            let body = match check_body(body) {
                Ok(body) => body,
                Err(message) => return Ok(Err(Refusal::Invalid("body", message))),
            };
            let content = crate::mcp::content_from_text(state, project_id, &body).await?;
            let Ok(content) = MessageContent::from_json(content) else {
                return Ok(Err(Refusal::Invalid(
                    "body",
                    "Keep a comment under 10,000 characters.",
                )));
            };
            let locked = match message_edits::lock_message_for_change(
                transaction,
                actor,
                Some(thread_id),
                comment_id,
                Change::Edit,
            )
            .await
            {
                Ok(locked) => locked,
                Err(error) => return refused(error, Change::Edit).map(Err),
            };
            if locked.removed {
                return Ok(Err(Refusal::NotFound));
            }
            if let Err(error) =
                message_edits::apply_edit(transaction, &locked, actor, content, None).await
            {
                return refused(error, Change::Edit).map(Err);
            }
        }
        CommentChange::Delete => {
            let locked = match message_edits::lock_message_for_change(
                transaction,
                actor,
                Some(thread_id),
                comment_id,
                Change::Remove,
            )
            .await
            {
                Ok(locked) => locked,
                Err(error) => return refused(error, Change::Remove).map(Err),
            };
            if locked.removed {
                return Ok(Err(Refusal::NotFound));
            }
            if let Err(error) = message_edits::apply_removal(transaction, &locked, actor).await {
                return refused(error, Change::Remove).map(Err);
            }
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

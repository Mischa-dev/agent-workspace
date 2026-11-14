//! Where a page's live document is kept, and how changes to it are saved
//! (AWD-53 slice 3).
//!
//! A page that has been opened (in the editor, or by an agent's block tools)
//! has a stored document, and that document is the page's source of truth.
//! People's editors send it changes, agents' edits change it, and the page's
//! Markdown body is derived from it in the same transaction, so `page_get`,
//! search and exports keep working. A page nobody has opened has only its
//! Markdown, and is converted the first time it's opened: the document is
//! built from the Markdown exactly as the page drew it, and the Markdown
//! stays as it was until someone changes something.
//!
//! The derived Markdown keeps the words of every block that didn't change
//! (see [`derive_markdown`]), so an edit to one paragraph changes only that
//! paragraph's lines of the body and of its history.
//!
//! Every write locks the page row first, so the document, the body, the
//! version and the history always change together.

use std::{
    borrow::Cow,
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    ops::Range,
};

use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;
use yrs::Doc;

use super::{
    tree::{Block, Node, canon_blocks, from_markdown, plain_text, strip_ids},
    write::{write_block, write_document},
    ydoc,
};
use crate::{
    error::AppError,
    memory::{self, MAX_PAGE_BODY, Writer},
    outbox::{OutboxEventPayload, PageUpdatedPayload, enqueue_typed},
    page_blocks::{self, BlockKind, PARSE_LIMIT, PageBlocks},
};

/// The largest stored document, in bytes (the table checks it too).
pub const MAX_STATE_BYTES: usize = 5 * 1024 * 1024;
/// The largest change one request may send, in bytes.
pub const MAX_UPDATE_BYTES: usize = 2 * 1024 * 1024;
/// While the same person keeps editing a page live, their changes go into
/// one version of its history until it's this old; then a new one starts.
const MERGE_HISTORY_MINUTES: i32 = 10;

// ---------------------------------------------------------------------------
// Deriving the Markdown

/// A page's Markdown, derived from its document's blocks, and where each
/// block is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    pub markdown: String,
    /// Each block's range in `markdown`; none for a block with no Markdown
    /// (an empty paragraph).
    pub ranges: Vec<Option<Range<usize>>>,
}

/// The Markdown for `blocks`. Where `source` (the page's Markdown before the
/// change, or what someone just wrote) already has a block, its words are
/// kept as they were, so a change rewrites only the blocks it touched; the
/// rest is written afresh. The result is checked by reading it back, and
/// written wholly afresh if it doesn't read back as the blocks do. It ends
/// the way `source` ends (with a line break, say).
pub fn derive_markdown(source: &str, blocks: &[Block]) -> Derived {
    derive_markdown_with(source, &[], blocks)
}

/// [`derive_markdown`], also keeping the words of `sent`: Markdown someone
/// sent with the change, for the blocks it says.
pub fn derive_markdown_with(source: &str, sent: &[&str], blocks: &[Block]) -> Derived {
    let pieces = pieces_of(source, 0);
    // The blank space after the source's last block.
    let trailing = pieces
        .last()
        .map(|last| &source[last.end..])
        .filter(|rest| is_blank(rest))
        .unwrap_or_default();
    let mut derived = reuse_source(source, pieces, sent, blocks).unwrap_or_else(|| {
        let written = write_document(blocks);
        let mut ranges = vec![None; blocks.len()];
        for block in &written.blocks {
            ranges[block.index] = Some(block.start..block.end);
        }
        Derived {
            markdown: written.markdown,
            ranges,
        }
    });
    if !derived.markdown.is_empty() {
        derived.markdown.push_str(trailing);
    }
    derived
}

/// Middles bigger than this (blocks × source blocks) aren't matched block
/// by block; their blocks are written afresh.
const MAX_MATCH_CELLS: usize = 1_000_000;

/// A block's Markdown in one of the texts the words come from.
struct Piece<'s> {
    text: &'s str,
    /// Which text, and where in it.
    source: usize,
    start: usize,
    end: usize,
    /// Its place among that text's pieces.
    index: usize,
}

fn pieces_of(text: &str, source: usize) -> Vec<Piece<'_>> {
    PageBlocks::parse(text)
        .blocks()
        .iter()
        .filter(|block| !matches!(block.kind, BlockKind::Overflow { .. }))
        .enumerate()
        .map(|(index, block)| Piece {
            text,
            source,
            start: block.start,
            end: block.end,
            index,
        })
        .collect()
}

fn reuse_source<'s>(
    source: &'s str,
    mut pieces: Vec<Piece<'s>>,
    sent: &[&'s str],
    blocks: &[Block],
) -> Option<Derived> {
    for (number, text) in sent.iter().enumerate() {
        pieces.extend(pieces_of(text, number + 1));
    }
    if pieces.is_empty() {
        return None;
    }
    // What each piece says on its own, when that's one block.
    let said: Vec<Option<Block>> = pieces
        .iter()
        .map(|piece| {
            let mut read = from_markdown(&piece.text[piece.start..piece.end]);
            (read.len() == 1).then(|| read.pop()).flatten()
        })
        .collect();
    // Each block as its Markdown must read back, for the blocks Markdown can
    // say exactly.
    let wanted: Vec<Option<Block>> = blocks.iter().map(expressible).collect();
    let matched = match_in_order(&wanted, &said);

    let mut markdown = String::with_capacity(source.len());
    let mut ranges = vec![None; blocks.len()];
    let mut expected: Vec<Block> = Vec::with_capacity(blocks.len());
    let mut previous_marker: Option<char> = None;
    // The piece the last block written came from, if any.
    let mut last_piece: Option<&Piece<'_>> = None;
    let mut written_any = false;
    for (index, block) in blocks.iter().enumerate() {
        let reused = matched[index].and_then(|at| {
            let piece = &pieces[at];
            let text = &piece.text[piece.start..piece.end];
            let marker = list_marker(&block.node, text);
            // A list just after a list with the same marker would join it.
            (marker.is_none() || marker != previous_marker).then_some((at, text, marker))
        });
        let (text, marker, piece) = match reused {
            Some((at, text, marker)) => {
                expected.push(said[at].clone()?);
                (Cow::Borrowed(text), marker, Some(&pieces[at]))
            }
            None => {
                let Some((text, marker)) = write_block(block, previous_marker) else {
                    continue;
                };
                expected.extend(from_markdown(&text));
                (Cow::Owned(text), marker, None)
            }
        };
        if written_any {
            match (last_piece, piece) {
                // Neighbours in the same text keep the space between them.
                (Some(left), Some(right))
                    if left.source == right.source && left.index + 1 == right.index =>
                {
                    markdown.push_str(&left.text[left.end..right.start]);
                }
                _ => markdown.push_str("\n\n"),
            }
        } else if let Some(first) = piece
            && first.source == 0
            && first.index == 0
            && is_blank(&source[..first.start])
        {
            markdown.push_str(&source[..first.start]);
        }
        let start = markdown.len();
        markdown.push_str(&text);
        ranges[index] = Some(start..markdown.len());
        previous_marker = marker;
        last_piece = piece;
        written_any = true;
    }
    // Past the web's parsing limit nothing reads back as blocks anyway.
    let readable = markdown.encode_utf16().count() <= PARSE_LIMIT;
    if readable && from_markdown(&markdown) != expected {
        return None;
    }
    Some(Derived { markdown, ranges })
}

/// The block without its ids, if its Markdown can say it exactly.
fn expressible(block: &Block) -> Option<Block> {
    let stripped = strip_ids(std::slice::from_ref(block));
    if canon_blocks(&stripped) == stripped {
        stripped.into_iter().next()
    } else {
        None
    }
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// The marker of a list block's Markdown: its bullet, or the `.` or `)`
/// after an ordered list's number.
fn list_marker(node: &Node, text: &str) -> Option<char> {
    if !matches!(node, Node::BulletList(_) | Node::OrderedList { .. }) {
        return None;
    }
    let item = page_blocks::list_item(text.lines().next()?)?;
    item.marker.chars().last()
}

fn fingerprint(block: &Option<Block>) -> Option<u64> {
    block.as_ref().map(|block| {
        let mut hasher = DefaultHasher::new();
        block.hash(&mut hasher);
        hasher.finish()
    })
}

/// Pairs equal entries of `left` and `right`, keeping both in order (a
/// longest common subsequence, after the common start and end). `None`
/// entries never match.
fn match_in_order(left: &[Option<Block>], right: &[Option<Block>]) -> Vec<Option<usize>> {
    let left_prints: Vec<Option<u64>> = left.iter().map(fingerprint).collect();
    let right_prints: Vec<Option<u64>> = right.iter().map(fingerprint).collect();
    let same = |i: usize, j: usize| {
        left_prints[i].is_some() && left_prints[i] == right_prints[j] && left[i] == right[j]
    };
    let (n, m) = (left.len(), right.len());
    let mut matched = vec![None; n];
    let mut prefix = 0;
    while prefix < n && prefix < m && same(prefix, prefix) {
        matched[prefix] = Some(prefix);
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < n - prefix && suffix < m - prefix && same(n - 1 - suffix, m - 1 - suffix) {
        matched[n - 1 - suffix] = Some(m - 1 - suffix);
        suffix += 1;
    }
    let (rows, columns) = (n - prefix - suffix, m - prefix - suffix);
    if rows == 0 || columns == 0 || rows.saturating_mul(columns) > MAX_MATCH_CELLS {
        return matched;
    }
    let width = columns + 1;
    let mut table = vec![0u32; (rows + 1) * width];
    for i in (0..rows).rev() {
        for j in (0..columns).rev() {
            table[i * width + j] = if same(prefix + i, prefix + j) {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < rows && j < columns {
        if same(prefix + i, prefix + j) {
            matched[prefix + i] = Some(prefix + j);
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matched
}

// ---------------------------------------------------------------------------
// Changing a document

/// A document built from a page's Markdown, as the page draws it, with a
/// new id on every block.
pub fn build(markdown: &str) -> Doc {
    let doc = ydoc::new_doc();
    ydoc::replace(&doc, &from_markdown(markdown));
    ydoc::normalize(&doc);
    doc
}

/// The blocks `markdown` says, with the ids of the document's blocks they
/// are: a block whose Markdown is unchanged is the document's block exactly
/// (so what Markdown can't say about it, it keeps), and between those, an
/// edited block keeps its id: a block of the same kind in the same stretch,
/// the one sharing the most words with it.
pub fn blocks_for_markdown(current: &[Block], markdown: &str) -> Vec<Block> {
    let new = from_markdown(markdown);
    // Each current block as its Markdown reads back.
    let read_back: Vec<Option<Block>> = current.iter().map(read_back).collect();
    let wanted: Vec<Option<Block>> = new.iter().cloned().map(Some).collect();
    let matched = match_in_order(&wanted, &read_back);
    let mut result: Vec<Block> = new;
    let mut index = 0;
    // Each stretch of unmatched new blocks, with the current blocks between
    // the matches around it.
    while index < result.len() {
        if let Some(old) = matched[index] {
            result[index] = current[old].clone();
            index += 1;
            continue;
        }
        let new_start = index;
        while index < result.len() && matched[index].is_none() {
            index += 1;
        }
        let old_start = matched[..new_start]
            .iter()
            .rev()
            .flatten()
            .next()
            .map_or(0, |old| old + 1);
        let old_end = matched[index..]
            .iter()
            .flatten()
            .next()
            .copied()
            .unwrap_or(current.len());
        if old_start < old_end {
            pair_edited(&current[old_start..old_end], &mut result[new_start..index]);
        }
    }
    result
}

/// Gives each new block in a stretch the id of the current block it most
/// likely was: same kind, most words in common, nearest in place.
fn pair_edited(current: &[Block], new: &mut [Block]) {
    const MAX_PAIRS: usize = 10_000;
    let mut pairs: Vec<(usize, usize, usize, usize)> = Vec::new();
    if current.len().saturating_mul(new.len()) <= MAX_PAIRS {
        let current_words: Vec<_> = current.iter().map(block_words).collect();
        for (i, block) in new.iter().enumerate() {
            let words = block_words(block);
            for (j, old) in current.iter().enumerate() {
                if old.node.name() == block.node.name() {
                    let shared = words.intersection(&current_words[j]).count();
                    let distance = (i * current.len()).abs_diff(j * new.len());
                    pairs.push((shared, distance, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    } else {
        // Too many to weigh: pair them in order.
        let mut next = 0;
        for (i, block) in new.iter().enumerate() {
            if let Some(j) =
                (next..current.len()).find(|&j| current[j].node.name() == block.node.name())
            {
                pairs.push((0, 0, i, j));
                next = j + 1;
            }
        }
    }
    let mut new_taken = vec![false; new.len()];
    let mut old_taken = vec![false; current.len()];
    for (_, _, i, j) in pairs {
        if !new_taken[i] && !old_taken[j] {
            new_taken[i] = true;
            old_taken[j] = true;
            new[i].id = current[j].id.clone();
        }
    }
}

/// The words in a block's text, in lowercase.
fn block_words(block: &Block) -> std::collections::HashSet<String> {
    fn text(block: &Block, out: &mut String) {
        match &block.node {
            Node::Paragraph(content) | Node::Heading { content, .. } => {
                out.push_str(&plain_text(content));
            }
            Node::CodeBlock { text: code, .. } => out.push_str(code),
            Node::Blockquote(blocks) => blocks.iter().for_each(|block| text(block, out)),
            Node::BulletList(items) | Node::OrderedList { items, .. } => items
                .iter()
                .flat_map(|item| &item.content)
                .for_each(|block| text(block, out)),
            Node::HorizontalRule => {}
            Node::Table(rows) => rows.iter().flatten().for_each(|cell| {
                out.push_str(&plain_text(&cell.content));
                out.push(' ');
            }),
            Node::Embed { label, .. } => out.push_str(label),
        }
        out.push(' ');
    }
    let mut all = String::new();
    text(block, &mut all);
    all.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// What a block's Markdown reads back as, when that's one block.
fn read_back(block: &Block) -> Option<Block> {
    if let Some(exact) = expressible(block) {
        return Some(exact);
    }
    let (text, _) = write_block(block, None)?;
    let mut read = from_markdown(&text);
    (read.len() == 1).then(|| read.pop()).flatten()
}

/// A document change worked out in memory, ready to save.
pub struct Change {
    pub state: Vec<u8>,
    /// The document is different from before.
    pub changed: bool,
    pub blocks: Vec<Block>,
    pub derived: Derived,
}

/// Loads `state`, lets `edit` change the document, and works out the result.
/// `source` (the page's Markdown) and `sent` (Markdown sent with the change)
/// are the words to keep for blocks they already say.
pub fn change(
    state: &[u8],
    source: &str,
    sent: &[&str],
    edit: impl FnOnce(&Doc, &[Block]) -> Result<(), AppError>,
) -> Result<Change, AppError> {
    let doc = ydoc::load(state).map_err(|_| AppError::Internal("load page document"))?;
    let before = ydoc::read_doc(&doc).map_err(|_| AppError::Internal("read page document"))?;
    edit(&doc, &before)?;
    let blocks = ydoc::read_doc(&doc).map_err(|_| AppError::Internal("read page document"))?;
    let state_after = ydoc::encode(&doc);
    let changed = state_after != state;
    let derived = derive_markdown_with(source, sent, &blocks);
    Ok(Change {
        state: state_after,
        changed,
        blocks,
        derived,
    })
}

/// A browser's change, applied: the change to save, and what the browser
/// lacks in return.
pub struct Applied {
    pub change: Change,
    /// What a peer at the browser's state vector is missing.
    pub reply: Vec<u8>,
    /// The document's state vector after the change.
    pub state_vector: Vec<u8>,
}

/// Applies a browser's update to a stored document (see
/// [`ydoc::apply_client_update`]), and works out the result and the reply
/// for a browser whose state vector is `since`.
pub fn apply_browser_update(
    state: &[u8],
    source: &str,
    update: &[u8],
    since: &[u8],
) -> Result<Result<Applied, ydoc::Refused>, AppError> {
    let doc = ydoc::load(state).map_err(|_| AppError::Internal("load page document"))?;
    if let Err(refused) = ydoc::apply_client_update(&doc, update) {
        return Ok(Err(refused));
    }
    let blocks = ydoc::read_doc(&doc).map_err(|_| AppError::Internal("read page document"))?;
    let state_after = ydoc::encode(&doc);
    let changed = state_after != state;
    let derived = derive_markdown(source, &blocks);
    let reply = ydoc::encode_since(&doc, since).map_err(|_| AppError::Invalid("stateVector"))?;
    Ok(Ok(Applied {
        change: Change {
            state: state_after,
            changed,
            blocks,
            derived,
        },
        reply,
        state_vector: ydoc::state_vector(&doc),
    }))
}

/// A stored document's blocks.
pub fn read_blocks(state: &[u8]) -> Result<Vec<Block>, AppError> {
    let doc = ydoc::load(state).map_err(|_| AppError::Internal("load page document"))?;
    ydoc::read_doc(&doc).map_err(|_| AppError::Internal("read page document"))
}

/// What a browser at state vector `since` lacks of a stored document, and
/// the document's state vector.
pub fn missing(state: &[u8], since: &[u8]) -> Result<(Vec<u8>, Vec<u8>), AppError> {
    let doc = ydoc::load(state).map_err(|_| AppError::Internal("load page document"))?;
    let reply = ydoc::encode_since(&doc, since).map_err(|_| AppError::Invalid("stateVector"))?;
    Ok((reply, ydoc::state_vector(&doc)))
}

/// A stored document's state vector.
pub fn state_vector_of(state: &[u8]) -> Result<Vec<u8>, AppError> {
    let doc = ydoc::load(state).map_err(|_| AppError::Internal("load page document"))?;
    Ok(ydoc::state_vector(&doc))
}

// ---------------------------------------------------------------------------
// Storage

/// The page row, locked for the rest of the transaction.
#[derive(Clone, Debug, FromRow)]
pub struct LockedPage {
    pub project_id: Uuid,
    pub title: String,
    pub body: String,
    pub version: i64,
    pub archived: bool,
}

pub async fn lock_page(
    transaction: &mut Transaction<'_, Postgres>,
    page_id: Uuid,
) -> Result<Option<LockedPage>, AppError> {
    Ok(sqlx::query_as::<_, LockedPage>(
        r#"
        SELECT project_id, title, body, version, archived_at IS NOT NULL AS archived
        FROM project_pages WHERE id = $1 FOR UPDATE
        "#,
    )
    .bind(page_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// A stored document.
#[derive(Clone, Debug, FromRow)]
pub struct Stored {
    pub generation: Uuid,
    pub state: Vec<u8>,
}

/// A stored document with the page's version and Markdown, read together.
#[derive(Clone, Debug, FromRow)]
pub struct Snapshot {
    pub generation: Uuid,
    pub state: Vec<u8>,
    pub version: i64,
    pub body: String,
}

/// The page's document, if it has one, read in one go with the page's
/// version so the two belong together.
pub async fn snapshot(pool: &PgPool, page_id: Uuid) -> Result<Option<Snapshot>, AppError> {
    Ok(sqlx::query_as::<_, Snapshot>(
        r#"
        SELECT document.generation, document.state, page.version, page.body
        FROM project_page_documents document
        JOIN project_pages page ON page.id = document.page_id
        WHERE document.page_id = $1
        "#,
    )
    .bind(page_id)
    .fetch_optional(pool)
    .await?)
}

/// The page's document, converting the page if it hasn't one yet.
pub async fn open(pool: &PgPool, page_id: Uuid) -> Result<Snapshot, AppError> {
    if let Some(snapshot) = snapshot(pool, page_id).await? {
        return Ok(snapshot);
    }
    let mut transaction = pool.begin().await?;
    let page = lock_page(&mut transaction, page_id)
        .await?
        .ok_or(AppError::NotFound("page"))?;
    let stored = document(&mut transaction, page_id, &page).await?;
    transaction.commit().await?;
    Ok(Snapshot {
        generation: stored.generation,
        state: stored.state,
        version: page.version,
        body: page.body,
    })
}

/// The page's stored document, if it has one. Call with the page locked.
pub async fn stored(
    transaction: &mut Transaction<'_, Postgres>,
    page_id: Uuid,
) -> Result<Option<Stored>, AppError> {
    let stored = sqlx::query_as::<_, Stored>(
        "SELECT generation, state FROM project_page_documents WHERE page_id = $1",
    )
    .bind(page_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    if readable(&stored.state) {
        return Ok(Some(stored));
    }
    // A document that can't be read (it shouldn't happen) is rebuilt from
    // the page's Markdown, as a new generation so open editors start over.
    tracing::warn!(%page_id, "page document unreadable; rebuilding it from the page's Markdown");
    let body = sqlx::query_scalar::<_, String>("SELECT body FROM project_pages WHERE id = $1")
        .bind(page_id)
        .fetch_one(&mut **transaction)
        .await?;
    let rebuilt = Stored {
        generation: Uuid::now_v7(),
        state: ydoc::encode(&build(&body)),
    };
    sqlx::query(
        r#"
        UPDATE project_page_documents
        SET generation = $2, state = $3, updated_at = now()
        WHERE page_id = $1
        "#,
    )
    .bind(page_id)
    .bind(rebuilt.generation)
    .bind(&rebuilt.state)
    .execute(&mut **transaction)
    .await?;
    Ok(Some(rebuilt))
}

fn readable(state: &[u8]) -> bool {
    ydoc::load(state)
        .ok()
        .is_some_and(|doc| ydoc::read_doc(&doc).is_ok())
}

/// The page's document, made from its Markdown if it has none. Call with the
/// page locked. Converting changes nothing anyone can see: the version and
/// the Markdown stay as they are until the first edit.
pub async fn document(
    transaction: &mut Transaction<'_, Postgres>,
    page_id: Uuid,
    page: &LockedPage,
) -> Result<Stored, AppError> {
    if let Some(stored) = stored(transaction, page_id).await? {
        return Ok(stored);
    }
    let converted = Stored {
        generation: Uuid::now_v7(),
        state: ydoc::encode(&build(&page.body)),
    };
    if converted.state.len() > MAX_STATE_BYTES {
        return Err(too_big());
    }
    sqlx::query(
        r#"
        INSERT INTO project_page_documents (page_id, project_id, generation, state)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(page_id)
    .bind(page.project_id)
    .bind(converted.generation)
    .bind(&converted.state)
    .execute(&mut **transaction)
    .await?;
    Ok(converted)
}

pub fn too_long() -> AppError {
    AppError::Refused {
        field: "body",
        message: "This page is too long to save. Make it shorter, or move part of it to another page.",
    }
}

pub fn too_big() -> AppError {
    AppError::Refused {
        field: "document",
        message: "This page has grown too big to save. Move part of it to another page.",
    }
}

/// How a saved change goes into the page's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    /// A version of its own.
    Version,
    /// Live editing: merged into the writer's version from the last few
    /// minutes, if the latest version is theirs.
    Live,
}

/// Saves a document change worked out by [`change`]: the document, the
/// derived Markdown, a new version and history, and the `page.updated`
/// event. Call with the page locked. Returns the page's version after it,
/// and whether a new version went into the history.
pub async fn save(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    page_id: Uuid,
    page: &LockedPage,
    change: &Change,
    note: Option<&str>,
    history: History,
) -> Result<(i64, bool), AppError> {
    crate::events::ensure_actor(transaction, writer.actor_id).await?;
    crate::domain::set_via_grant(transaction, writer.via_grant_id).await?;
    let body = &change.derived.markdown;
    if !change.changed && *body == page.body {
        return Ok((page.version, false));
    }
    if change.state.len() > MAX_STATE_BYTES {
        return Err(too_big());
    }
    if body.chars().count() > MAX_PAGE_BODY {
        return Err(too_long());
    }
    if change.changed {
        sqlx::query(
            "UPDATE project_page_documents SET state = $2, updated_at = now() WHERE page_id = $1",
        )
        .bind(page_id)
        .bind(&change.state)
        .execute(&mut **transaction)
        .await?;
    }
    let version = page.version + 1;
    sqlx::query(
        r#"
        UPDATE project_pages
        SET body = $2, version = $3, updated_by_actor_id = $4, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(page_id)
    .bind(body)
    .bind(version)
    .bind(writer.actor_id)
    .execute(&mut **transaction)
    .await?;
    let mut new_version = false;
    if *body != page.body {
        new_version = memory::record_revision(
            transaction,
            writer,
            page_id,
            version,
            &page.title,
            body,
            note,
            (history == History::Live).then_some(MERGE_HISTORY_MINUTES),
        )
        .await?;
    }
    page_updated(transaction, page.project_id, page_id, version).await?;
    Ok((version, new_version))
}

/// Tells open pages and editors that the page changed.
pub async fn page_updated(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    page_id: Uuid,
    version: i64,
) -> Result<(), AppError> {
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::PageUpdated(PageUpdatedPayload { page_id, version }),
    )
    .await?;
    Ok(())
}

/// For `update_page`: the page's new Markdown when the page has a stored
/// document. The document changes to hold `markdown` (keeping the ids of
/// blocks that stay, and changing edited blocks in place so concurrent
/// typing survives), and the Markdown returned is derived from it. `None`
/// when the page has no document: its Markdown is simply replaced.
pub async fn set_markdown(
    transaction: &mut Transaction<'_, Postgres>,
    page_id: Uuid,
    markdown: &str,
) -> Result<Option<String>, AppError> {
    let Some(stored) = stored(transaction, page_id).await? else {
        return Ok(None);
    };
    let change = change(&stored.state, markdown, &[], |doc, current| {
        ydoc::replace(doc, &blocks_for_markdown(current, markdown));
        Ok(())
    })?;
    if change.state.len() > MAX_STATE_BYTES {
        return Err(too_big());
    }
    if change.changed {
        sqlx::query(
            "UPDATE project_page_documents SET state = $2, updated_at = now() WHERE page_id = $1",
        )
        .bind(page_id)
        .bind(&change.state)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(Some(change.derived.markdown))
}

/// Block ids by position, for answers that list a page's blocks.
pub fn ids(blocks: &[Block]) -> HashMap<usize, String> {
    blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| Some((index, block.id.clone()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_docs::testing::{Random, random_blocks};

    const PAGE: &str = "# Plan\n\nSome *words*   here.\n\n* one\n* two\n\n1) first\n2) second\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n";

    fn doc_blocks(markdown: &str) -> Vec<Block> {
        ydoc::read_doc(&build(markdown)).expect("reads")
    }

    #[test]
    fn untouched_pages_keep_their_markdown_byte_for_byte() {
        for source in [
            PAGE,
            "",
            "just a line",
            "\n\n# Leading blank lines\ntext\n\n\n\ntrailing\n\n",
            "- a\n\n\n- b\n",
            "> quote\n> more\n\npara\r\nwith crlf\r\n",
        ] {
            let blocks = doc_blocks(source);
            let derived = derive_markdown(source, &blocks);
            if source.trim().is_empty() {
                assert_eq!(derived.markdown, "", "{source:?}");
            } else {
                assert_eq!(derived.markdown, source, "{source:?}");
            }
        }
    }

    #[test]
    fn an_edit_rewrites_only_the_block_it_touched() {
        let doc = build(PAGE);
        let mut blocks = ydoc::read_doc(&doc).expect("reads");
        // Change the paragraph's text.
        blocks[1] = from_markdown("Some **other** words.").remove(0);
        let derived = derive_markdown(PAGE, &blocks);
        assert_eq!(
            derived.markdown,
            PAGE.replace("Some *words*   here.", "Some **other** words.")
        );
        let range = derived.ranges[1].clone().expect("has Markdown");
        assert_eq!(&derived.markdown[range], "Some **other** words.");
    }

    #[test]
    fn lists_that_would_join_are_rewritten_apart() {
        let source = "* one\n\npara\n\n* two\n";
        let mut blocks = doc_blocks(source);
        // Take out the paragraph: the two lists now touch.
        blocks.remove(1);
        let derived = derive_markdown(source, &blocks);
        assert_eq!(
            from_markdown(&derived.markdown).len(),
            2,
            "{:?}",
            derived.markdown
        );
        assert!(
            derived.markdown.starts_with("* one\n\n"),
            "{:?}",
            derived.markdown
        );
    }

    #[test]
    fn derived_markdown_always_reads_back_as_the_blocks() {
        let mut random = Random(0x5703_e000_0001);
        for case in 0..600 {
            let first = random_blocks(&mut random, 0, 6);
            let second = random_blocks(&mut random, 0, 6);
            let source = write_document(&first).markdown;
            let derived = derive_markdown(&source, &second);
            let fresh = write_document(&second).markdown;
            assert_eq!(
                from_markdown(&derived.markdown),
                from_markdown(&fresh),
                "case {case}: {source:?} -> {:?}",
                derived.markdown
            );
        }
    }

    #[test]
    fn markdown_for_a_document_keeps_ids_of_blocks_that_stay_or_were_edited() {
        let current = doc_blocks(PAGE);
        let ids: Vec<Option<String>> = current.iter().map(|block| block.id.clone()).collect();
        let edited = PAGE
            .replace("Some *words*   here.", "Other words.")
            .replace("# Plan", "# Plan\n\nA new paragraph.");
        let blocks = blocks_for_markdown(&current, &edited);
        assert_eq!(blocks.len(), current.len() + 1);
        assert_eq!(blocks[0].id, ids[0], "the heading stays");
        assert_eq!(blocks[1].id, None, "the new paragraph is new");
        assert_eq!(
            blocks[2].id, ids[1],
            "the edited paragraph is the same block"
        );
        for (index, id) in ids.iter().enumerate().skip(2) {
            assert_eq!(&blocks[index + 1].id, id, "block {index} keeps its id");
            assert_eq!(
                blocks[index + 1],
                current[index],
                "block {index} is unchanged"
            );
        }
        // A paragraph rewritten from scratch in place is still that block.
        let rewritten = PAGE.replace("Some *words*   here.", "Nothing alike.");
        assert_eq!(blocks_for_markdown(&current, &rewritten)[1].id, ids[1]);
    }

    #[test]
    fn set_markdown_through_a_document_keeps_every_id_of_an_unchanged_page() {
        let doc = build(PAGE);
        let before = ydoc::read_doc(&doc).expect("reads");
        let state = ydoc::encode(&doc);
        let change = change(&state, PAGE, &[], |doc, current| {
            ydoc::replace(doc, &blocks_for_markdown(current, PAGE));
            Ok(())
        })
        .expect("changes");
        assert!(!change.changed, "nothing changed");
        assert_eq!(change.blocks, before);
        assert_eq!(change.derived.markdown, PAGE);
    }
}

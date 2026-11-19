//! Suggested edits (AWD-53, criterion 6; K1.12). Someone whose role only
//! suggests on a page (`access::write_mode` answers `Propose`: they hold
//! `pages.suggest`, or `pages.edit_own` on a page someone else made) doesn't
//! change it: their edits are kept as a suggestion that anyone who may edit
//! the page accepts or rejects; the author may withdraw it. Which accounts
//! suggest is the role's, never the account's kind: existing agents hold
//! "Member, suggests" from the K1.12 migration.
//!
//! A suggestion is a list of block edits by lasting block id, each carrying
//! the hash of the block as the author read it, so it can't undo a change
//! they hadn't seen: accepting a suggestion whose blocks have changed since
//! is refused, and the suggestion stays for someone to reject. Accepting
//! saves the edits as the author's, with a note naming who accepted them.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    edits::{self, Edit, EditError, EditOp, Target},
    store,
    tree::{Block, strip_ids},
    write::write_block,
    ydoc,
};
use crate::{
    error::AppError,
    memory::Writer,
    notifications::{self, NewNotification},
    outbox::{OutboxEventPayload, PageDiscussionUpdatedPayload, enqueue_typed},
};

/// The most pending suggestions one agent may leave on one page.
pub const MAX_PENDING_PER_AUTHOR: i64 = 50;
/// How many decided suggestions a page's list shows.
const DECIDED_SHOWN: i64 = 20;
/// How much of a block's Markdown a suggestion keeps to show what it
/// changes.
const BEFORE_CHARACTERS: usize = 4_000;

/// Who created the page, read with the page locked.
pub async fn page_creator(
    transaction: &mut Transaction<'_, Postgres>,
    page_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT created_by_actor_id FROM project_pages WHERE id = $1",
    )
    .bind(page_id)
    .fetch_optional(&mut **transaction)
    .await?
    .flatten())
}

// ---------------------------------------------------------------------------
// Edits as they're kept

/// One edit of a suggestion, naming its block by lasting id.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredEdit {
    /// replace, insertBefore, insertAfter, delete or append.
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The new Markdown (not for delete).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// The named block's Markdown when the suggestion was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

fn op_name(op: EditOp) -> &'static str {
    match op {
        EditOp::Replace => "replace",
        EditOp::InsertBefore => "insertBefore",
        EditOp::InsertAfter => "insertAfter",
        EditOp::Delete => "delete",
        EditOp::Append => "append",
    }
}

fn op_of(name: &str) -> Option<EditOp> {
    Some(match name {
        "replace" => EditOp::Replace,
        "insertBefore" => EditOp::InsertBefore,
        "insertAfter" => EditOp::InsertAfter,
        "delete" => EditOp::Delete,
        "append" => EditOp::Append,
        _ => return None,
    })
}

fn block_markdown(block: &Block) -> String {
    let text = write_block(block, None)
        .map(|(text, _)| text)
        .unwrap_or_default();
    match text.char_indices().nth(BEFORE_CHARACTERS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

/// Checks `edits` against the page's blocks now and turns them into edits
/// by block id (a position becomes the id of the block there), with each
/// named block's hash and Markdown. Fails as [`edits::apply`] would.
pub fn resolve(current: &[Block], edits: &[Edit]) -> Result<Vec<StoredEdit>, EditError> {
    edits::apply(current, edits)?;
    let mut stored = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let block = edit.target.as_ref().map(|target| {
            let position = match target {
                Target::Id(id) => current
                    .iter()
                    .position(|block| block.id.as_deref() == Some(id.as_str())),
                Target::Position(position) => Some(*position),
            };
            position.and_then(|position| current.get(position))
        });
        let block = match block {
            Some(Some(block)) => Some(block),
            Some(None) => {
                return Err(EditError::NoSuchBlock {
                    index,
                    target: edit.target.clone().unwrap_or(Target::Position(0)),
                    blocks: current.len(),
                });
            }
            None => None,
        };
        let block_id = match block {
            Some(block) => Some(block.id.clone().ok_or(EditError::NoSuchBlock {
                index,
                target: edit.target.clone().unwrap_or(Target::Position(0)),
                blocks: current.len(),
            })?),
            None => None,
        };
        stored.push(StoredEdit {
            op: op_name(edit.op).to_owned(),
            block_id,
            // Inserts don't need the block as it was, but keeping its hash
            // lets the owner see what it was next to.
            hash: block.map(edits::content_hash),
            markdown: edit.markdown.clone(),
            before: block.map(block_markdown),
        });
    }
    Ok(stored)
}

/// The kept edits, to apply to the page as it is when accepted. Inserts
/// carry no hash: the block they're next to may have changed meanwhile.
pub fn to_edits(stored: &[StoredEdit]) -> Result<Vec<Edit>, AppError> {
    stored
        .iter()
        .map(|edit| {
            let op = op_of(&edit.op).ok_or(AppError::Internal("suggestion edit op"))?;
            Ok(Edit {
                op,
                target: edit.block_id.clone().map(Target::Id),
                hash: matches!(op, EditOp::Replace | EditOp::Delete)
                    .then(|| edit.hash.clone())
                    .flatten(),
                markdown: edit.markdown.clone(),
            })
        })
        .collect()
}

/// Block edits that turn the page's blocks into the page `markdown` says:
/// what `page_write` suggests when an agent rewrites someone else's page.
/// Blocks that stay are left alone; edited blocks are replaced in place,
/// removed ones deleted, and new ones inserted where they go. If that can't
/// say the change (blocks moved around), it replaces the whole page. `None`
/// when nothing changes.
pub fn diff(current: &[Block], markdown: &str) -> Option<Vec<Edit>> {
    let wanted = store::blocks_for_markdown(current, markdown);
    if strip_ids(&wanted) == strip_ids(current) {
        return None;
    }
    let written = |block: &Block| write_block(block, None).map(|(text, _)| text);
    let by_id = |id: &str| current.iter().find(|block| block.id.as_deref() == Some(id));
    let mut edits = Vec::new();
    let mut previous: Option<String> = None;
    let mut fits = true;
    for block in &wanted {
        match block.id.as_deref().and_then(by_id) {
            Some(old) => {
                if strip_ids(std::slice::from_ref(old)) != strip_ids(std::slice::from_ref(block)) {
                    match written(block) {
                        Some(text) => edits.push(Edit {
                            op: EditOp::Replace,
                            target: old.id.clone().map(Target::Id),
                            hash: Some(edits::content_hash(old)),
                            markdown: Some(text),
                        }),
                        None => fits = false,
                    }
                }
                previous = old.id.clone();
            }
            None => match written(block) {
                Some(text) => edits.push(match &previous {
                    Some(id) => Edit {
                        op: EditOp::InsertAfter,
                        target: Some(Target::Id(id.clone())),
                        hash: None,
                        markdown: Some(text),
                    },
                    None => match current.first().and_then(|first| first.id.clone()) {
                        Some(first) => Edit {
                            op: EditOp::InsertBefore,
                            target: Some(Target::Id(first)),
                            hash: None,
                            markdown: Some(text),
                        },
                        None => Edit {
                            op: EditOp::Append,
                            target: None,
                            hash: None,
                            markdown: Some(text),
                        },
                    },
                }),
                None => fits = false,
            },
        }
    }
    for old in current {
        let kept = old
            .id
            .as_deref()
            .is_some_and(|id| wanted.iter().any(|block| block.id.as_deref() == Some(id)));
        if !kept {
            edits.push(Edit {
                op: EditOp::Delete,
                target: old.id.clone().map(Target::Id),
                hash: Some(edits::content_hash(old)),
                markdown: None,
            });
        }
    }
    // Inserts after the same block land in the order they're made, so the
    // result reads as wanted unless blocks moved; check, and say the whole
    // page otherwise.
    fits = fits
        && edits::apply(current, &edits)
            .is_ok_and(|result| strip_ids(&result) == strip_ids(&wanted));
    if fits {
        return Some(edits);
    }
    let mut whole: Vec<Edit> = current
        .iter()
        .map(|old| Edit {
            op: EditOp::Delete,
            target: old.id.clone().map(Target::Id),
            hash: Some(edits::content_hash(old)),
            markdown: None,
        })
        .collect();
    let trimmed = markdown.trim();
    if !trimmed.is_empty() {
        whole.push(Edit {
            op: EditOp::Append,
            target: None,
            hash: None,
            markdown: Some(trimmed.to_owned()),
        });
    }
    Some(whole)
}

// ---------------------------------------------------------------------------
// Storage

/// A suggestion as it's listed.
#[derive(Clone, Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionRow {
    pub id: Uuid,
    pub page_id: Uuid,
    pub author_id: Uuid,
    pub author: Option<String>,
    pub author_kind: String,
    pub note: Option<String>,
    pub base_version: i64,
    #[serde(skip)]
    pub edits: sqlx::types::Json<Vec<StoredEdit>>,
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub applied_version: Option<i64>,
    pub created_at: String,
}

const SUGGESTION_SELECT: &str = r#"
    SELECT suggestion.id, suggestion.thing_id AS page_id, suggestion.author_actor_id AS author_id,
           author.display_name AS author, actor.kind AS author_kind,
           suggestion.note, suggestion.base_version, suggestion.change -> 'edits' AS edits,
           suggestion.status,
           decider.display_name AS decided_by,
           to_char(suggestion.decided_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS decided_at,
           suggestion.applied_version,
           to_char(suggestion.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at
    FROM proposals suggestion
    JOIN actors actor ON actor.id = suggestion.author_actor_id
    LEFT JOIN project_memberships author
      ON author.project_id = suggestion.space_id AND author.actor_id = suggestion.author_actor_id
    LEFT JOIN project_memberships decider
      ON decider.project_id = suggestion.space_id AND decider.actor_id = suggestion.decided_by_actor_id
"#;

/// A page's pending suggestions, oldest first, then (if asked) the latest
/// decided ones, newest first.
pub async fn list(
    pool: &PgPool,
    page_id: Uuid,
    include_decided: bool,
) -> Result<Vec<SuggestionRow>, AppError> {
    let mut rows = sqlx::query_as::<_, SuggestionRow>(&format!(
        "{SUGGESTION_SELECT} WHERE suggestion.thing_kind = 'page' AND suggestion.thing_id = $1 \
         AND suggestion.status = 'pending' \
         ORDER BY suggestion.created_at, suggestion.id LIMIT 200"
    ))
    .bind(page_id)
    .fetch_all(crate::db::scoped(pool))
    .await?;
    if include_decided {
        rows.extend(
            sqlx::query_as::<_, SuggestionRow>(&format!(
                "{SUGGESTION_SELECT} WHERE suggestion.thing_kind = 'page' AND suggestion.thing_id = $1 \
                 AND suggestion.status <> 'pending' \
                 ORDER BY suggestion.decided_at DESC, suggestion.id DESC LIMIT $2"
            ))
            .bind(page_id)
            .bind(DECIDED_SHOWN)
            .fetch_all(crate::db::scoped(pool))
            .await?,
        );
    }
    Ok(rows)
}

pub async fn get(
    executor: impl sqlx::PgExecutor<'_>,
    suggestion_id: Uuid,
) -> Result<Option<SuggestionRow>, AppError> {
    Ok(sqlx::query_as::<_, SuggestionRow>(&format!(
        "{SUGGESTION_SELECT} WHERE suggestion.id = $1 AND suggestion.thing_kind = 'page'"
    ))
    .bind(suggestion_id)
    .fetch_optional(executor)
    .await?)
}

/// Tells open pages that a page's comments or suggestions changed.
pub async fn discussion_updated(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    page_id: Uuid,
) -> Result<(), AppError> {
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::PageDiscussionUpdated(PageDiscussionUpdatedPayload { page_id }),
    )
    .await?;
    Ok(())
}

/// Keeps a suggestion of `edits` (already checked with [`resolve`]) from
/// `author` on a page, and tells the page's owner. Call with the page
/// locked.
pub async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    author: Writer,
    page_id: Uuid,
    page: &store::LockedPage,
    edits: &[StoredEdit],
    note: Option<&str>,
) -> Result<Uuid, AppError> {
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM proposals WHERE thing_kind = 'page' AND thing_id = $1 AND author_actor_id = $2 AND status = 'pending'",
    )
    .bind(page_id)
    .bind(author.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if pending >= MAX_PENDING_PER_AUTHOR {
        return Err(AppError::Refused {
            field: "edits",
            message: "You have too many suggestions waiting on this page. Wait for some to be decided, or withdraw them.",
        });
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO proposals (
            id, workspace_id, space_id, thing_kind, thing_id, operation, author_actor_id,
            via_oauth_grant_id, note, base_version, change
        )
        SELECT $1, project.organization_id, project.id, 'page', $3, 'page_edit', $4, $8, $5, $6,
               jsonb_build_object('edits', $7::jsonb)
        FROM projects project WHERE project.id = $2
        "#,
    )
    .bind(id)
    .bind(page.project_id)
    .bind(page_id)
    .bind(author.actor_id)
    .bind(note)
    .bind(page.version)
    .bind(sqlx::types::Json(edits))
    .bind(author.via_grant_id)
    .execute(&mut **transaction)
    .await?;
    if let Some(owner) = page_creator(transaction, page_id).await? {
        let mut notification = NewNotification::page(
            page.project_id,
            owner,
            notifications::kind::PAGE_SUGGESTION,
            page_id,
            format!("page-suggestions:{page_id}"),
            format!("page-suggestion:{id}"),
        );
        notification.actor_id = Some(author.actor_id);
        notification.detail = serde_json::json!({
            "pageSuggestionId": id,
            "href": format!(
                "{}#suggestion-{id}",
                crate::addresses::Target::page(page.project_id, page_id).legacy_path()
            ),
        });
        notify(transaction, &notification).await?;
    }
    crate::proposals::announce_fact(transaction, id, false).await?;
    discussion_updated(transaction, page.project_id, page_id).await?;
    Ok(id)
}

/// What became of new text offered as a suggestion.
#[derive(Debug)]
pub enum Suggested {
    /// Kept as this suggestion, of `edits` block edits to `from_version`.
    Made {
        id: Uuid,
        edits: usize,
        from_version: i64,
    },
    /// The text is what the page already says.
    Unchanged,
    /// The page is archived.
    Archived,
    /// It changes more blocks than one suggestion holds.
    TooBig,
    /// The edits don't fit the page (the message says why).
    Invalid(String),
}

/// Keeps `markdown`, offered by `author` as the whole of a page's text, as
/// a suggestion of the block edits that turn the page into it: the one way
/// both the web editor's save and `page_write` route a `Propose` grant
/// (C1.8). Locks the page; `expected_version` refuses a page that moved on,
/// and a page no longer in `in_space` (where the author was allowed) reads
/// as missing.
pub async fn suggest_text(
    transaction: &mut Transaction<'_, Postgres>,
    author: Writer,
    in_space: Uuid,
    page_id: Uuid,
    markdown: &str,
    expected_version: Option<i64>,
    note: Option<&str>,
) -> Result<Suggested, AppError> {
    let Some(page) = store::lock_page(transaction, page_id).await? else {
        return Err(AppError::NotFound("page"));
    };
    if page.project_id != in_space {
        return Err(AppError::NotFound("page"));
    }
    if let Some(expected) = expected_version
        && expected != page.version
    {
        return Err(AppError::VersionConflict {
            resource: "page",
            current_version: page.version,
        });
    }
    if page.archived {
        return Ok(Suggested::Archived);
    }
    let stored = store::document(transaction, page_id, &page).await?;
    let current = store::read_blocks(&stored.state)?;
    let Some(edits) = diff(&current, markdown) else {
        return Ok(Suggested::Unchanged);
    };
    if edits.len() > edits::MAX_EDITS {
        return Ok(Suggested::TooBig);
    }
    let stored_edits = match resolve(&current, &edits) {
        Ok(stored) => stored,
        Err(error) => return Ok(Suggested::Invalid(error.to_string())),
    };
    let id = create(transaction, author, page_id, &page, &stored_edits, note).await?;
    Ok(Suggested::Made {
        id,
        edits: stored_edits.len(),
        from_version: page.version,
    })
}

/// Delivers a notification to someone who can still see the page's project.
pub(crate) async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    notification: &NewNotification,
) -> Result<(), AppError> {
    let kind = sqlx::query_scalar::<_, String>(
        "SELECT kind FROM actors WHERE id = $1 AND status = 'active'",
    )
    .bind(notification.recipient_actor_id)
    .fetch_optional(&mut **transaction)
    .await?;
    // People have an inbox; agents read suggestions and comments with their
    // tools.
    if kind.as_deref() != Some("human") {
        return Ok(());
    }
    let reaches = sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
        .bind(notification.recipient_actor_id)
        .bind(notification.project_id)
        .fetch_one(&mut **transaction)
        .await?;
    if !reaches {
        return Ok(());
    }
    if notifications::deliver(transaction, notification).await? {
        notifications::announce(
            transaction,
            notification.project_id,
            notification.recipient_actor_id,
        )
        .await?;
    }
    Ok(())
}

/// What someone decides about a suggestion.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Accept,
    Reject,
    /// The agent that made it takes it back.
    Withdraw,
}

/// Why a decision wasn't taken.
#[derive(Debug)]
pub enum Refusal {
    NotFound,
    /// Only people who may edit the page (or the agent that owns it) decide;
    /// only the author withdraws.
    NotYours(&'static str),
    AlreadyDecided(String),
    Archived,
    /// The page changed where the suggestion edits it.
    Stale(String),
}

/// The outcome of a decision taken.
pub struct Decided {
    pub status: &'static str,
    pub version: i64,
}

/// Accepts, rejects or withdraws a suggestion on `page_id`. `may_edit` is
/// whether `decider` may edit this page directly (`access::write_mode`
/// answered `Direct`: `pages.edit`, or `pages.edit_own` on their own page);
/// accepting and rejecting need it, and only the author withdraws.
pub async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    decider: Writer,
    may_edit: bool,
    in_space: Uuid,
    page_id: Uuid,
    suggestion_id: Uuid,
    decision: Decision,
) -> Result<Result<Decided, Refusal>, AppError> {
    let Some(page) = store::lock_page(transaction, page_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    // `may_edit` was decided in `in_space`; a page moved out since is
    // another space's to decide.
    if page.project_id != in_space {
        return Ok(Err(Refusal::NotFound));
    }
    let Some(suggestion) = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<String>,
            sqlx::types::Json<Vec<StoredEdit>>,
        ),
    >(
        r#"
        SELECT author_actor_id, status, note, change -> 'edits' FROM proposals
        WHERE id = $1 AND thing_kind = 'page' AND thing_id = $2 FOR UPDATE
        "#,
    )
    .bind(suggestion_id)
    .bind(page_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    let (author_id, status, note, sqlx::types::Json(stored_edits)) = suggestion;
    if status != "pending" {
        return Ok(Err(Refusal::AlreadyDecided(status)));
    }
    match decision {
        Decision::Withdraw if decider.actor_id != author_id => {
            return Ok(Err(Refusal::NotYours(
                "Only the agent that made a suggestion can withdraw it. Reject it instead.",
            )));
        }
        Decision::Accept | Decision::Reject if !may_edit => {
            return Ok(Err(Refusal::NotYours(
                "Deciding a suggestion needs edit access to the page. Ask someone who edits it.",
            )));
        }
        _ => {}
    }
    let mut version = page.version;
    let mut applied_change_set = None;
    let status = match decision {
        Decision::Accept => {
            if page.archived {
                return Ok(Err(Refusal::Archived));
            }
            let stored = store::document(transaction, page_id, &page).await?;
            let current = store::read_blocks(&stored.state)?;
            let edits = to_edits(&stored_edits)?;
            let blocks = match edits::apply(&current, &edits) {
                Ok(blocks) => blocks,
                Err(error) => {
                    return Ok(Err(Refusal::Stale(match error {
                        EditError::HashMismatch { .. } => {
                            "A block it changes has been edited since it was suggested.".to_owned()
                        }
                        EditError::NoSuchBlock { .. } => {
                            "A block it changes is no longer on the page.".to_owned()
                        }
                        other => other.to_string(),
                    })));
                }
            };
            let sent: Vec<&str> = stored_edits
                .iter()
                .filter_map(|edit| edit.markdown.as_deref())
                .collect();
            let change = store::change(&stored.state, &page.body, &sent, |doc, _| {
                ydoc::replace(doc, &blocks);
                Ok(())
            })?;
            let decider_name = sqlx::query_scalar::<_, String>(
                "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
            )
            .bind(page.project_id)
            .bind(decider.actor_id)
            .fetch_optional(&mut **transaction)
            .await?
            .unwrap_or_else(|| "someone".to_owned());
            let accepted = if decider.actor_id == author_id {
                "Suggestion applied".to_owned()
            } else {
                format!("Suggestion accepted by {decider_name}")
            };
            let note = match note {
                Some(note) => format!("{note} ({accepted})"),
                None => accepted,
            };
            let note: String = note.chars().take(500).collect();
            // The edits are the author's: the history says so.
            let author_kind =
                sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
                    .bind(author_id)
                    .fetch_one(&mut **transaction)
                    .await?;
            let author = Writer {
                actor_id: author_id,
                actor_kind: crate::conversations::parse_actor_kind(&author_kind)?,
                via_grant_id: None,
                confined_project: None,
            };
            let (saved, _) = store::save(
                transaction,
                author,
                page_id,
                &page,
                &change,
                Some(&note),
                store::History::Version,
            )
            .await?;
            version = saved;
            if let Some(change_set) =
                crate::change_sets::at_version(transaction, "page", page_id, saved).await?
            {
                sqlx::query(
                    "UPDATE document_change_sets SET proposal_id = $2, open_until = NULL WHERE id = $1",
                )
                .bind(change_set)
                .bind(suggestion_id)
                .execute(&mut **transaction)
                .await?;
                applied_change_set = Some(change_set);
            }
            "accepted"
        }
        Decision::Reject => "rejected",
        Decision::Withdraw => "withdrawn",
    };
    sqlx::query(
        r#"
        UPDATE proposals
        SET status = $2, decided_by_actor_id = $3, decided_at = now(), updated_at = now(),
            applied_version = CASE WHEN $2 = 'accepted' THEN $4 END,
            applied_change_set_id = $5
        WHERE id = $1
        "#,
    )
    .bind(suggestion_id)
    .bind(status)
    .bind(decider.actor_id)
    .bind(version)
    .bind(applied_change_set)
    .execute(&mut **transaction)
    .await?;
    discussion_updated(transaction, page.project_id, page_id).await?;
    Ok(Ok(Decided { status, version }))
}

impl Refusal {
    /// The refusal as an error people and agents read.
    pub fn into_error(self) -> AppError {
        match self {
            Self::NotFound => AppError::NotFound("suggestion"),
            Self::NotYours(message) => AppError::Refused {
                field: "suggestion",
                message,
            },
            Self::AlreadyDecided(_) => AppError::Refused {
                field: "suggestion",
                message: "That suggestion has already been decided.",
            },
            Self::Archived => AppError::Refused {
                field: "page",
                message: "This page is archived. Bring it back to accept suggestions.",
            },
            Self::Stale(_) => AppError::Refused {
                field: "suggestion",
                message: "The page has changed where this suggestion edits it, so it no longer fits. Reject it, and ask for a new one if it's still wanted.",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_docs::tree::from_markdown;

    fn with_ids(mut blocks: Vec<Block>) -> Vec<Block> {
        for (index, block) in blocks.iter_mut().enumerate() {
            block.id = Some(format!("blk{index}"));
        }
        blocks
    }

    #[test]
    fn diff_says_only_what_changed() {
        let current = with_ids(from_markdown("# Plan\n\nOne\n\nTwo\n\nThree\n"));
        let wanted = "# Plan\n\nOne\n\nTwo, changed\n\nNew\n\n## Next\n";
        let edits = diff(&current, wanted).expect("a change");
        let ops: Vec<_> = edits
            .iter()
            .map(|edit| (edit.op, edit.target.clone()))
            .collect();
        // Edited paragraphs are changed in place (the third one reads as
        // an edit of "Three"); the new heading goes after it.
        assert_eq!(
            ops,
            vec![
                (EditOp::Replace, Some(Target::Id("blk2".into()))),
                (EditOp::Replace, Some(Target::Id("blk3".into()))),
                (EditOp::InsertAfter, Some(Target::Id("blk3".into()))),
            ]
        );
        let result = edits::apply(&current, &edits).expect("applies");
        assert_eq!(strip_ids(&result), strip_ids(&from_markdown(wanted)));
        // Taking a block out deletes it.
        let edits = diff(&current, "# Plan\n\nOne\n\nThree\n").expect("a change");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].op, EditOp::Delete);
    }

    #[test]
    fn diff_of_the_same_page_is_nothing() {
        let current = with_ids(from_markdown("One\n\nTwo\n"));
        assert_eq!(diff(&current, "One\n\nTwo\n"), None);
    }

    #[test]
    fn diff_adds_at_the_top_and_to_an_empty_page() {
        let current = with_ids(from_markdown("One\n"));
        let edits = diff(&current, "Zero\n\nOne\n").expect("a change");
        assert_eq!(edits[0].op, EditOp::InsertBefore);
        let empty: Vec<Block> = Vec::new();
        let edits = diff(&empty, "Hello\n").expect("a change");
        assert_eq!(edits[0].op, EditOp::Append);
    }

    #[test]
    fn resolve_names_blocks_by_id_and_keeps_what_they_were() {
        let current = with_ids(from_markdown("One\n\nTwo\n"));
        let stored = resolve(
            &current,
            &[Edit {
                op: EditOp::Replace,
                target: Some(Target::Position(1)),
                hash: Some(edits::content_hash(&current[1])),
                markdown: Some("Deux".into()),
            }],
        )
        .expect("resolves");
        assert_eq!(stored[0].block_id.as_deref(), Some("blk1"));
        assert_eq!(stored[0].before.as_deref(), Some("Two"));
        // A block that changed since makes the kept edit refuse.
        let mut changed = current.clone();
        changed[1] = with_ids(from_markdown("x\n\nTwo again\n")).remove(1);
        let edits = to_edits(&stored).expect("edits");
        assert!(matches!(
            edits::apply(&changed, &edits),
            Err(EditError::HashMismatch { .. })
        ));
    }
}

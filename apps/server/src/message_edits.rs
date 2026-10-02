//! Editing and removing messages (docs/plans/mcp-tools.md, decision 3).
//!
//! People and agents edit and remove their own messages. Every edit keeps the
//! earlier versions in `message_revisions`, visible to everyone who can see
//! the conversation. Removing a message leaves a tombstone: its text and the
//! text of every revision are deleted (who, when and through which app are
//! kept), and so is every copy of the text this server made: mentions, task
//! references, pins, reply requests, idempotent replays and thread names
//! suggested from it. Project owners and admins (people, never agents) can
//! remove anyone's message.
//!
//! The web routes here and the MCP tools share `lock_message_for_change`,
//! `apply_edit` and `apply_removal`. The database allows a change to a sent
//! message only while `aw.message_command` names it (`allow_change`), and only
//! in the shapes these commands write (migration 0047).

use std::collections::{BTreeSet, HashSet};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        ActorView, ConversationAccessRow, MessageAppendResponse, MessageCommandActor, MessageView,
        ResolvedContent, TimelineEntryView, authorize_conversation_row_for, enqueue_event,
        enqueue_external_message_events, fetch_conversation_access, fetch_timeline_entry,
        finish_idempotent_command, insert_audit_event_for, insert_message_projections,
        lock_message_conversation, message_content_error, parse_actor_kind,
        resolve_message_content, suggested_thread_name, web_suggested_thread_name,
    },
    error::{ApiJson, ApiPath, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    message_content::MessageContent,
    outbox::{ConversationUpdatedPayload, MessageUpdatedPayload, OutboxEventPayload},
    policy::{Action, ActorKind},
    reactions,
    replies::{self, Withdraw},
};

// Same transport cap as sending: a valid message body fits well below it.
const MAX_HTTP_BODY_BYTES: usize = 131_072;
/// The highest revision a message can reach (999 edits), which stops an agent
/// stuck in a loop from editing forever.
pub(crate) const MAX_REVISION: i32 = 1000;
/// Why reply requests about a removed message were withdrawn.
pub(crate) const REMOVED_MESSAGE_REASON: &str = "The message was removed.";
/// The name a thread gets when its name repeated a removed message.
const NEUTRAL_THREAD_NAME: &str = "Thread";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/conversations/{id}/messages/{message_id}",
            get(get_message).patch(edit_message).delete(remove_message),
        )
        .route(
            "/api/v1/conversations/{id}/messages/{message_id}/revisions",
            get(message_revisions),
        )
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
}

/// A message locked for an edit or a removal, after the caller was checked.
pub(crate) struct LockedMessage {
    pub(crate) conversation: ConversationAccessRow,
    pub(crate) id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) content: serde_json::Value,
    pub(crate) plain_text: String,
    pub(crate) revision: i32,
    pub(crate) removed: bool,
    /// Removing someone else's message as a project owner or admin.
    pub(crate) as_moderator: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Change {
    Edit,
    Remove,
}

/// Why a visible message can't be changed. Invisible messages never get this
/// far: they are NotFound, so their existence isn't revealed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Refused {
    /// Only the author edits; only the author or a moderator removes.
    NotYourMessage,
    /// The caller's role can't send here (a Viewer, for example).
    RoleCannotWrite,
    /// Agents remove only their own messages, whatever their role.
    OnlyPeopleModerate,
    Removed,
    /// Edits wait until the conversation is restored.
    Archived,
    /// The message is at this revision, not the expected one.
    RevisionMoved(i32),
    TooManyEdits,
}

#[derive(Debug)]
pub(crate) enum ChangeError {
    Refused(Refused),
    App(AppError),
}

impl From<AppError> for ChangeError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for ChangeError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

impl From<Refused> for AppError {
    fn from(refused: Refused) -> Self {
        match refused {
            Refused::NotYourMessage | Refused::RoleCannotWrite | Refused::OnlyPeopleModerate => {
                AppError::Forbidden
            }
            Refused::Removed => AppError::Conflict("message", "removed"),
            Refused::Archived => AppError::Conflict("conversation", "archived"),
            Refused::RevisionMoved(current) => AppError::VersionConflict {
                resource: "message",
                current_version: i64::from(current),
            },
            Refused::TooManyEdits => AppError::Conflict("message", "too many edits"),
        }
    }
}

impl From<ChangeError> for AppError {
    fn from(error: ChangeError) -> Self {
        match error {
            ChangeError::Refused(refused) => refused.into(),
            ChangeError::App(error) => error,
        }
    }
}

/// The outcome of an edit.
pub(crate) struct MessageEdit {
    pub(crate) entry: TimelineEntryView,
    /// False when the message already had exactly this content.
    pub(crate) changed: bool,
    pub(crate) from_revision: i32,
    /// Mentioned now and in no earlier revision, other than the editor.
    pub(crate) newly_mentioned: Vec<Uuid>,
}

/// A thread started from a removed message, after any renaming.
#[derive(Clone, Debug)]
pub(crate) struct RemovedMessageThread {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) status: String,
}

/// The outcome of a removal.
pub(crate) struct MessageRemoval {
    pub(crate) entry: TimelineEntryView,
    /// True when it was already removed; nothing changed.
    pub(crate) already_removed: bool,
    pub(crate) as_moderator: bool,
    pub(crate) cancelled_reply_requests: i64,
    pub(crate) unpinned: i64,
    /// Threads started from the message whose name repeated its text and was
    /// replaced with a neutral one.
    pub(crate) renamed_threads: i64,
    /// Threads started from the message, after any renaming.
    pub(crate) threads: Vec<RemovedMessageThread>,
}

/// What the web edit route returns and stores for idempotent replays. MCP's
/// message_edit stores the same body under the same scope, so a key retried
/// on the other surface still replays.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEditResponse {
    pub conversation_id: Uuid,
    pub message: MessageView,
    pub entry: TimelineEntryView,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRemoveResponse {
    pub conversation_id: Uuid,
    pub message: MessageView,
    pub entry: TimelineEntryView,
    pub already_removed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageEntryResponse {
    entry: TimelineEntryView,
}

/// Every version of a message, oldest first.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRevisionsView {
    pub message_id: Uuid,
    pub revision: i32,
    pub removed: bool,
    pub revisions: Vec<MessageRevisionView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRevisionView {
    pub revision: i32,
    /// Null once the message was removed.
    pub content: Option<serde_json::Value>,
    pub plain_text: Option<String>,
    pub written_by: ActorView,
    pub written_at: String,
    /// The connected app a person wrote it through, when that app's "show
    /// via" setting is on.
    pub via: Option<String>,
    pub redacted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditMessageRequest {
    content: serde_json::Value,
    expected_revision: Option<i32>,
}

/// The idempotency scope edits share on the web and through MCP.
pub(crate) fn edit_scope(message_id: Uuid) -> Result<String, AppError> {
    idempotency::resource_bound_scope("message", message_id, "edit")
}

/// The request hash edits share on the web and through MCP.
pub(crate) fn edit_request_hash(
    content: &MessageContent,
    expected_revision: Option<i32>,
) -> Result<RequestHash, AppError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct EditHash<'a> {
        content: &'a MessageContent,
        expected_revision: Option<i32>,
    }
    RequestHash::for_value(&EditHash {
        content,
        expected_revision,
    })
}

/// Allow this transaction to change one message (and its revisions and
/// projections), as the triggers from migration 0047 require.
pub(crate) async fn allow_change(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT set_config('aw.message_command', $1, true)")
        .bind(message_id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

#[derive(FromRow)]
struct MessageLockRow {
    id: Uuid,
    project_id: Uuid,
    author_actor_id: Uuid,
    content: serde_json::Value,
    plain_text: String,
    revision: i32,
    removed: bool,
}

/// Lock a message for an edit or a removal and check that `actor` may make
/// it. The conversation is locked first (FOR UPDATE, through
/// `lock_message_conversation` with ProjectView, which checks
/// `conversation_visible_to` and, for agents, their credential and
/// `agent_connection_can_access`), then the message row FOR UPDATE: the same
/// order as sending. A message the actor can't see, or that isn't in
/// `conversation_hint`, is NotFound.
pub(crate) async fn lock_message_for_change(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MessageCommandActor,
    conversation_hint: Option<Uuid>,
    message_id: Uuid,
    change: Change,
) -> Result<LockedMessage, ChangeError> {
    let conversation_id = match conversation_hint {
        Some(conversation_id) => conversation_id,
        None => sqlx::query_scalar::<_, Uuid>("SELECT conversation_id FROM messages WHERE id = $1")
            .bind(message_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(AppError::NotFound("message"))?,
    };
    let conversation =
        match lock_message_conversation(transaction, actor, conversation_id, Action::ProjectView)
            .await
        {
            Ok(conversation) => conversation,
            Err(AppError::NotFound(_)) => return Err(AppError::NotFound("message").into()),
            Err(error) => return Err(error.into()),
        };
    let message = sqlx::query_as::<_, MessageLockRow>(
        r#"
        SELECT id, project_id, author_actor_id, content, plain_text, revision,
               removed_at IS NOT NULL AS removed
        FROM messages
        WHERE id = $1 AND conversation_id = $2 AND project_id = $3
        FOR UPDATE
        "#,
    )
    .bind(message_id)
    .bind(conversation.id)
    .bind(conversation.project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("message"))?;
    if conversation.project_archived {
        return Err(AppError::ProjectArchived.into());
    }
    let actor_kind = parse_actor_kind(&actor.actor_kind)?;
    let is_author = message.author_actor_id == actor.actor_id;
    let can_write = conversation.allows(actor.actor_id, actor_kind, Action::MessageSend)?;
    let as_moderator = match change {
        Change::Edit => {
            if !is_author {
                return Err(ChangeError::Refused(Refused::NotYourMessage));
            }
            if !can_write {
                return Err(ChangeError::Refused(Refused::RoleCannotWrite));
            }
            if conversation.is_archived() {
                return Err(ChangeError::Refused(Refused::Archived));
            }
            false
        }
        Change::Remove => {
            if is_author && can_write {
                false
            } else if actor_kind == ActorKind::Human
                && conversation.allows(actor.actor_id, actor_kind, Action::MessageModerate)?
            {
                true
            } else if is_author {
                return Err(ChangeError::Refused(Refused::RoleCannotWrite));
            } else if actor_kind != ActorKind::Human {
                return Err(ChangeError::Refused(Refused::OnlyPeopleModerate));
            } else {
                return Err(ChangeError::Refused(Refused::NotYourMessage));
            }
        }
    };
    Ok(LockedMessage {
        conversation,
        id: message.id,
        project_id: message.project_id,
        content: message.content,
        plain_text: message.plain_text,
        revision: message.revision,
        removed: message.removed,
        as_moderator,
    })
}

/// The locked message's timeline entry, with its reactions as `viewer` sees
/// them (none once it is removed).
async fn locked_entry(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedMessage,
    viewer: Uuid,
) -> Result<TimelineEntryView, AppError> {
    let mut entry = fetch_timeline_entry(&mut **transaction, &locked.conversation, locked.id)
        .await?
        .ok_or(AppError::Internal("locked message has no timeline entry"))?;
    reactions::attach_reactions(
        &mut **transaction,
        locked.conversation.id,
        std::slice::from_mut(&mut entry),
        viewer,
    )
    .await?;
    Ok(entry)
}

/// Replace the text of a locked message. The earlier versions stay in
/// `message_revisions`; mentions and task references follow the new content.
/// Agents newly mentioned are told, and in a channel or thread a person's
/// edit asks them to reply. Editing to identical content changes nothing.
pub(crate) async fn apply_edit(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedMessage,
    actor: &MessageCommandActor,
    content: MessageContent,
    expected_revision: Option<i32>,
) -> Result<MessageEdit, ChangeError> {
    if locked.removed {
        return Err(ChangeError::Refused(Refused::Removed));
    }
    if expected_revision.is_some_and(|expected| expected != locked.revision) {
        return Err(ChangeError::Refused(Refused::RevisionMoved(
            locked.revision,
        )));
    }
    if locked.revision >= MAX_REVISION {
        return Err(ChangeError::Refused(Refused::TooManyEdits));
    }
    let ResolvedContent {
        content_json,
        plain_text,
        mentioned_actor_ids,
        work_item_references,
        work_item_rows: _,
    } = resolve_message_content(transaction, locked.project_id, content).await?;
    if content_json == locked.content {
        return Ok(MessageEdit {
            entry: locked_entry(transaction, locked, actor.actor_id).await?,
            changed: false,
            from_revision: locked.revision,
            newly_mentioned: Vec::new(),
        });
    }
    allow_change(transaction, locked.id).await?;
    if locked.revision == 1 {
        // The first edit keeps the original as revision 1.
        sqlx::query(
            r#"
            INSERT INTO message_revisions (
                project_id, message_id, revision, content, plain_text,
                written_by_actor_id, via_oauth_grant_id, written_at
            )
            SELECT project_id, id, revision, content, plain_text,
                   author_actor_id, via_oauth_grant_id, created_at
            FROM messages
            WHERE id = $1 AND revision = 1
            ON CONFLICT (message_id, revision) DO NOTHING
            "#,
        )
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    }
    let next_revision = locked.revision + 1;
    sqlx::query(
        r#"
        INSERT INTO message_revisions (
            project_id, message_id, revision, content, plain_text,
            written_by_actor_id, via_oauth_grant_id, written_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, now())
        "#,
    )
    .bind(locked.project_id)
    .bind(locked.id)
    .bind(next_revision)
    .bind(&content_json)
    .bind(&plain_text)
    .bind(actor.actor_id)
    .bind(actor.via_grant_id)
    .execute(&mut **transaction)
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE messages
        SET content = $3, plain_text = $4, revision = revision + 1, edited_at = now()
        WHERE id = $1 AND revision = $2 AND removed_at IS NULL
        "#,
    )
    .bind(locked.id)
    .bind(locked.revision)
    .bind(&content_json)
    .bind(&plain_text)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal("edit a locked message").into());
    }

    // Mentions: drop the ones taken out, add only the new ones (the insert
    // trigger validates every row it sees, even ones a conflict would skip).
    let previous = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM message_mentions WHERE message_id = $1",
    )
    .bind(locked.id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    sqlx::query("DELETE FROM message_mentions WHERE message_id = $1 AND NOT (actor_id = ANY($2))")
        .bind(locked.id)
        .bind(&mentioned_actor_ids)
        .execute(&mut **transaction)
        .await?;
    let added = mentioned_actor_ids
        .iter()
        .copied()
        .filter(|actor_id| !previous.contains(actor_id))
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM message_work_item_references WHERE message_id = $1")
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    insert_message_projections(
        transaction,
        locked.project_id,
        locked.id,
        &added,
        &work_item_references,
    )
    .await?;

    // Newly mentioned: in no earlier revision. Someone taken out and put back
    // isn't told twice, and the editor never counts.
    let earlier = sqlx::query_scalar::<_, Option<String>>(
        r#"
        SELECT DISTINCT COALESCE(node->>'actorId', node->>'actor_id')
        FROM message_revisions revision,
             jsonb_array_elements(revision.content->'nodes') node
        WHERE revision.message_id = $1
          AND revision.revision <= $2
          AND revision.content IS NOT NULL
          AND node->>'type' = 'mention'
        "#,
    )
    .bind(locked.id)
    .bind(locked.revision)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .flatten()
    .filter_map(|id| Uuid::parse_str(&id).ok())
    .collect::<HashSet<_>>();
    let newly_mentioned = mentioned_actor_ids
        .iter()
        .copied()
        .filter(|actor_id| *actor_id != actor.actor_id && !earlier.contains(actor_id))
        .collect::<Vec<_>>();

    enqueue_event(
        transaction,
        locked.project_id,
        OutboxEventPayload::MessageUpdated(MessageUpdatedPayload {
            conversation_id: locked.conversation.id,
            message_id: locked.id,
            revision: next_revision,
            removed: false,
        }),
    )
    .await?;
    enqueue_external_message_events(
        transaction,
        &locked.conversation,
        actor.actor_id,
        locked.id,
        &newly_mentioned,
    )
    .await?;
    replies::ask_after_edit(
        transaction,
        replies::NewMessage {
            project_id: locked.project_id,
            conversation_id: locked.conversation.id,
            members_conversation_id: locked.conversation.membership_conversation_id(),
            conversation_kind: locked.conversation.effective_kind(),
            message_id: locked.id,
            author_actor_id: actor.actor_id,
            author_kind: &actor.actor_kind,
            mentioned_actor_ids: &newly_mentioned,
        },
    )
    .await?;
    Ok(MessageEdit {
        entry: locked_entry(transaction, locked, actor.actor_id).await?,
        changed: true,
        from_revision: locked.revision,
        newly_mentioned,
    })
}

/// Remove a locked message for everyone. Its text and every revision's text
/// are deleted (who wrote each, when and through which app stay), and so is
/// every copy the server made: mentions, task references, pins, open reply
/// requests about it, stored replays of the send and its edits, and thread
/// names suggested from it (and older audit rows naming those threads).
/// Removing an already removed message changes nothing.
pub(crate) async fn apply_removal(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedMessage,
    actor: &MessageCommandActor,
) -> Result<MessageRemoval, ChangeError> {
    if locked.removed {
        return Ok(MessageRemoval {
            entry: locked_entry(transaction, locked, actor.actor_id).await?,
            already_removed: true,
            as_moderator: locked.as_moderator,
            cancelled_reply_requests: 0,
            unpinned: 0,
            renamed_threads: 0,
            threads: threads_from(transaction, locked.id).await?,
        });
    }
    allow_change(transaction, locked.id).await?;

    // Read every version's text once more, only to recognize thread names
    // suggested from it, before it is deleted.
    let mut texts = sqlx::query_scalar::<_, String>(
        "SELECT plain_text FROM message_revisions WHERE message_id = $1 AND plain_text IS NOT NULL",
    )
    .bind(locked.id)
    .fetch_all(&mut **transaction)
    .await?;
    texts.push(locked.plain_text.clone());

    sqlx::query(
        r#"
        UPDATE message_revisions
        SET content = NULL, plain_text = NULL, redacted_at = now(), redacted_by_actor_id = $2
        WHERE message_id = $1 AND redacted_at IS NULL
        "#,
    )
    .bind(locked.id)
    .bind(actor.actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM message_mentions WHERE message_id = $1")
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM message_work_item_references WHERE message_id = $1")
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    let updated = sqlx::query(
        r#"
        UPDATE messages
        SET content = '{"version": 1, "nodes": []}'::jsonb, plain_text = '',
            removed_at = now(), removed_by_actor_id = $2, removed_via_oauth_grant_id = $3
        WHERE id = $1 AND removed_at IS NULL
        "#,
    )
    .bind(locked.id)
    .bind(actor.actor_id)
    .bind(actor.via_grant_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal("remove a locked message").into());
    }
    let cancelled_reply_requests = replies::withdraw_requests(
        transaction,
        Withdraw::Message {
            conversation_id: locked.conversation.id,
            message_id: locked.id,
        },
        REMOVED_MESSAGE_REASON,
    )
    .await?;
    let unpinned = i64::try_from(
        sqlx::query(
            "DELETE FROM conversation_pins WHERE target_kind = 'message' AND target_id = $1",
        )
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?
        .rows_affected(),
    )
    .unwrap_or(i64::MAX);
    let renamed_threads = neutralize_thread_names(transaction, locked, &texts).await?;
    scrub_thread_audit_names(transaction, locked).await?;

    let entry = locked_entry(transaction, locked, actor.actor_id).await?;
    scrub_send_replays(transaction, locked, &entry).await?;
    // A retried edit then finds nothing to replay and is refused as removed.
    sqlx::query("DELETE FROM idempotency_keys WHERE scope = $1")
        .bind(edit_scope(locked.id)?)
        .execute(&mut **transaction)
        .await?;
    enqueue_event(
        transaction,
        locked.project_id,
        OutboxEventPayload::MessageUpdated(MessageUpdatedPayload {
            conversation_id: locked.conversation.id,
            message_id: locked.id,
            revision: locked.revision,
            removed: true,
        }),
    )
    .await?;
    Ok(MessageRemoval {
        entry,
        already_removed: false,
        as_moderator: locked.as_moderator,
        cancelled_reply_requests,
        unpinned,
        renamed_threads,
        threads: threads_from(transaction, locked.id).await?,
    })
}

/// The hash stored in place of a scrubbed replay's request hash. A hash of
/// the original request would let anyone holding a database copy confirm a
/// guessed secret, so it becomes the hash of empty content, which no request
/// can send: a retry with the old key is then refused as a different payload
/// instead of being replayed or sent again.
fn tombstone_request_hash() -> Result<RequestHash, AppError> {
    RequestHash::for_value(&MessageContent {
        version: 1,
        nodes: Vec::new(),
    })
}

/// Replace the stored replay of the send that created this message (web,
/// message_send, reply_send, thread_start and conversation_start all store
/// one) with the tombstone.
async fn scrub_send_replays(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedMessage,
    entry: &TimelineEntryView,
) -> Result<(), AppError> {
    let message = entry
        .message
        .clone()
        .ok_or(AppError::Internal("removed message entry has no message"))?;
    let body = serde_json::to_value(MessageAppendResponse {
        conversation_id: locked.conversation.id,
        message,
        entry: entry.clone(),
    })
    .map_err(|_| AppError::Internal("serialize removed message replay"))?;
    let scope =
        idempotency::resource_bound_scope("conversation", locked.conversation.id, "message_send")?;
    sqlx::query(
        r#"
        UPDATE idempotency_keys
        SET response_body = $3, request_hash = $4
        WHERE scope = $1 AND response_body->'message'->>'id' = $2
        "#,
    )
    .bind(scope)
    .bind(locked.id.to_string())
    .bind(body)
    .bind(tombstone_request_hash()?.as_bytes().as_slice())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Whether a thread name is one suggested from the removed text, or the
/// numbered variant (" (2)") used when that name was taken.
fn repeats_suggestion(name: &str, suggestions: &HashSet<String>) -> bool {
    let name = name.trim().to_lowercase();
    if suggestions.contains(&name) {
        return true;
    }
    name.strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" ("))
        .is_some_and(|(stem, number)| {
            !number.is_empty()
                && number.chars().all(|character| character.is_ascii_digit())
                && suggestions.contains(stem)
        })
}

/// Rename threads started from the message whose name was suggested from its
/// text (by thread_start, or prefilled by the web dialog) to a neutral name,
/// and scrub the web's stored replay of their creation. Names someone chose
/// themselves stay; the remover sees them listed and can rename them.
async fn neutralize_thread_names(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedMessage,
    texts: &[String],
) -> Result<i64, AppError> {
    let suggestions = texts
        .iter()
        .flat_map(|text| [suggested_thread_name(text), web_suggested_thread_name(text)])
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect::<HashSet<_>>();
    if suggestions.is_empty() {
        return Ok(0);
    }
    let threads = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, name FROM conversations
        WHERE origin_message_id = $1 AND kind = 'thread' AND parent_channel_id = $2
        ORDER BY created_at, id
        "#,
    )
    .bind(locked.id)
    .bind(locked.conversation.id)
    .fetch_all(&mut **transaction)
    .await?;
    let to_rename = threads
        .into_iter()
        .filter(|(_, name)| repeats_suggestion(name, &suggestions))
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    if to_rename.is_empty() {
        return Ok(0);
    }
    // Unique among every thread here, archived ones too, so restoring one
    // can't collide.
    let mut taken = sqlx::query_scalar::<_, String>(
        "SELECT lower(name) FROM conversations WHERE parent_channel_id = $1 AND kind = 'thread'",
    )
    .bind(locked.conversation.id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let scope =
        idempotency::resource_bound_scope("channel", locked.conversation.id, "thread_create")?;
    let mut renamed = 0;
    for thread_id in to_rename {
        let name = (1..)
            .map(|number| {
                if number == 1 {
                    NEUTRAL_THREAD_NAME.to_owned()
                } else {
                    format!("{NEUTRAL_THREAD_NAME} ({number})")
                }
            })
            .find(|candidate| !taken.contains(&candidate.to_lowercase()))
            .unwrap_or_else(|| NEUTRAL_THREAD_NAME.to_owned());
        taken.insert(name.to_lowercase());
        let version = sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE conversations
            SET name = $2, version = version + 1, updated_at = now()
            WHERE id = $1
            RETURNING version
            "#,
        )
        .bind(thread_id)
        .bind(&name)
        .fetch_one(&mut **transaction)
        .await?;
        enqueue_event(
            transaction,
            locked.project_id,
            OutboxEventPayload::ConversationUpdated(ConversationUpdatedPayload {
                conversation_id: thread_id,
                version,
            }),
        )
        .await?;
        sqlx::query(
            r#"
            UPDATE idempotency_keys
            SET response_body = jsonb_set(response_body, '{conversation,name}', to_jsonb($3::text)),
                request_hash = $4
            WHERE scope = $1 AND response_body->'conversation'->>'id' = $2
            "#,
        )
        .bind(&scope)
        .bind(thread_id.to_string())
        .bind(&name)
        .bind(tombstone_request_hash()?.as_bytes().as_slice())
        .execute(&mut **transaction)
        .await?;
        renamed += 1;
    }
    Ok(renamed)
}

/// Drop the name from audit rows of status changes on threads started from
/// the message. The web wrote the thread's name there before audit metadata
/// kept to IDs and counts, and that name may be (or once have been) one
/// suggested from the removed text. Who changed what, and when, stays.
async fn scrub_thread_audit_names(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedMessage,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE audit_events AS audit
        SET metadata = audit.metadata - 'name'
        FROM conversations AS thread
        WHERE thread.origin_message_id = $1 AND thread.kind = 'thread'
          AND audit.project_id = thread.project_id
          AND audit.target_kind = 'conversation' AND audit.target_id = thread.id
          AND audit.action = 'conversation.thread_updated'
          AND audit.metadata ? 'name'
        "#,
    )
    .bind(locked.id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn threads_from(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<Vec<RemovedMessageThread>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT id, name, thread_status FROM conversations
        WHERE origin_message_id = $1 AND kind = 'thread'
        ORDER BY created_at, id
        "#,
    )
    .bind(message_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|(id, name, status)| RemovedMessageThread { id, name, status })
    .collect())
}

#[derive(FromRow)]
struct RevisionRow {
    revision: i32,
    content: Option<serde_json::Value>,
    plain_text: Option<String>,
    writer_id: Uuid,
    writer_kind: String,
    writer_name: Option<String>,
    writer_handle: Option<String>,
    written_at: String,
    via: Option<String>,
    redacted: bool,
}

/// Every version of a message in `conversation`, oldest first, or None when
/// the message isn't there. `conversation` was loaded through a lookup that
/// checks `conversation_visible_to`. A message never edited has one version:
/// itself (redacted once removed).
pub(crate) async fn message_revisions_in(
    pool: &sqlx::PgPool,
    conversation: &ConversationAccessRow,
    message_id: Uuid,
) -> Result<Option<MessageRevisionsView>, AppError> {
    let Some((revision, removed)) = sqlx::query_as::<_, (i32, bool)>(
        r#"
        SELECT revision, removed_at IS NOT NULL
        FROM messages
        WHERE id = $1 AND conversation_id = $2 AND project_id = $3
        "#,
    )
    .bind(message_id)
    .bind(conversation.id)
    .bind(conversation.project_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let mut rows = sqlx::query_as::<_, RevisionRow>(
        r#"
        SELECT revision.revision, revision.content, revision.plain_text,
               writer.id AS writer_id, writer.kind AS writer_kind,
               membership.display_name AS writer_name, membership.handle AS writer_handle,
               revision.written_at::text AS written_at,
               via_client.client_name AS via,
               revision.redacted_at IS NOT NULL AS redacted
        FROM message_revisions revision
        JOIN actors writer ON writer.id = revision.written_by_actor_id
        LEFT JOIN project_memberships membership
          ON membership.project_id = revision.project_id
         AND membership.actor_id = revision.written_by_actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = revision.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        WHERE revision.message_id = $1 AND revision.project_id = $2
        ORDER BY revision.revision
        "#,
    )
    .bind(message_id)
    .bind(conversation.project_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        rows = sqlx::query_as::<_, RevisionRow>(
            r#"
            SELECT message.revision,
                   CASE WHEN message.removed_at IS NULL THEN message.content END AS content,
                   CASE WHEN message.removed_at IS NULL THEN message.plain_text END AS plain_text,
                   author.id AS writer_id, author.kind AS writer_kind,
                   membership.display_name AS writer_name, membership.handle AS writer_handle,
                   message.created_at::text AS written_at,
                   via_client.client_name AS via,
                   message.removed_at IS NOT NULL AS redacted
            FROM messages message
            JOIN actors author ON author.id = message.author_actor_id
            LEFT JOIN project_memberships membership
              ON membership.project_id = message.project_id
             AND membership.actor_id = message.author_actor_id
            LEFT JOIN mcp_oauth_grants via_grant
              ON via_grant.id = message.via_oauth_grant_id AND via_grant.show_via
            LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
            WHERE message.id = $1 AND message.conversation_id = $2
            "#,
        )
        .bind(message_id)
        .bind(conversation.id)
        .fetch_all(pool)
        .await?;
    }
    let revisions = rows
        .into_iter()
        .map(|row| MessageRevisionView {
            revision: row.revision,
            content: if row.redacted { None } else { row.content },
            plain_text: if row.redacted { None } else { row.plain_text },
            written_by: ActorView {
                id: row.writer_id,
                kind: row.writer_kind,
                display_name: row
                    .writer_name
                    .unwrap_or_else(|| "Former member".to_owned()),
                handle: row
                    .writer_handle
                    .unwrap_or_else(|| "former-member".to_owned()),
                avatar_url: None,
            },
            written_at: row.written_at,
            via: row.via,
            redacted: row.redacted,
        })
        .collect();
    Ok(Some(MessageRevisionsView {
        message_id,
        revision,
        removed,
        revisions,
    }))
}

fn browser_actor(principal: &AuthenticatedActor) -> MessageCommandActor {
    MessageCommandActor {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind.clone(),
        mcp_credential: None,
        via_grant_id: None,
    }
}

/// A conversation the principal can see, archived or not, for reading one of
/// its messages.
async fn readable_conversation(
    state: &AppState,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
) -> Result<ConversationAccessRow, AppError> {
    let conversation = fetch_conversation_access(&state.pool, principal.actor_id, conversation_id)
        .await?
        .ok_or(AppError::NotFound("message"))?;
    match authorize_conversation_row_for(
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        &conversation,
        Action::ProjectView,
        true,
    ) {
        Ok(()) => Ok(conversation),
        Err(AppError::NotFound(_)) => Err(AppError::NotFound("message")),
        Err(error) => Err(error),
    }
}

async fn get_message(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, message_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<MessageEntryResponse>, AppError> {
    let conversation = readable_conversation(&state, &principal, conversation_id).await?;
    let mut entry = fetch_timeline_entry(&state.pool, &conversation, message_id)
        .await?
        .ok_or(AppError::NotFound("message"))?;
    reactions::attach_reactions(
        &state.pool,
        conversation.id,
        std::slice::from_mut(&mut entry),
        principal.actor_id,
    )
    .await?;
    Ok(Json(MessageEntryResponse { entry }))
}

async fn message_revisions(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, message_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<MessageRevisionsView>, AppError> {
    let conversation = readable_conversation(&state, &principal, conversation_id).await?;
    let revisions = message_revisions_in(&state.pool, &conversation, message_id)
        .await?
        .ok_or(AppError::NotFound("message"))?;
    Ok(Json(revisions))
}

async fn edit_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, message_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<EditMessageRequest>,
) -> Result<(StatusCode, Json<MessageEditResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let content = MessageContent::from_json(request.content).map_err(message_content_error)?;
    let request_hash = edit_request_hash(&content, request.expected_revision)?;
    let actor = browser_actor(&principal);
    let mut transaction = state.pool.begin().await?;
    let locked = lock_message_for_change(
        &mut transaction,
        &actor,
        Some(conversation_id),
        message_id,
        Change::Edit,
    )
    .await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &edit_scope(message_id)?,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let body: MessageEditResponse = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(body)));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let edit = apply_edit(
        &mut transaction,
        &locked,
        &actor,
        content,
        request.expected_revision,
    )
    .await?;
    let body = MessageEditResponse {
        conversation_id: locked.conversation.id,
        message: edit
            .entry
            .message
            .clone()
            .ok_or(AppError::Internal("edited entry has no message"))?,
        entry: edit.entry,
        changed: edit.changed,
    };
    finish_idempotent_command(&mut transaction, &reservation, StatusCode::OK, &body).await?;
    if edit.changed {
        insert_audit_event_for(
            &mut transaction,
            &headers,
            &principal,
            locked.project_id,
            "conversation.message_edited",
            "message",
            locked.id,
            serde_json::json!({
                "conversation_id": locked.conversation.id,
                "from_revision": edit.from_revision,
                "to_revision": body.message.revision,
                "newly_mentioned": edit.newly_mentioned.len(),
            }),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok((StatusCode::OK, Json(body)))
}

async fn remove_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, message_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<MessageRemoveResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let actor = browser_actor(&principal);
    let mut transaction = state.pool.begin().await?;
    let locked = lock_message_for_change(
        &mut transaction,
        &actor,
        Some(conversation_id),
        message_id,
        Change::Remove,
    )
    .await?;
    let removal = apply_removal(&mut transaction, &locked, &actor).await?;
    if !removal.already_removed {
        insert_audit_event_for(
            &mut transaction,
            &headers,
            &principal,
            locked.project_id,
            "conversation.message_removed",
            "message",
            locked.id,
            removal_audit_metadata(&locked, &removal),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(Json(MessageRemoveResponse {
        conversation_id: locked.conversation.id,
        message: removal
            .entry
            .message
            .clone()
            .ok_or(AppError::Internal("removed entry has no message"))?,
        entry: removal.entry,
        already_removed: removal.already_removed,
    }))
}

/// Audit metadata for a removal: IDs and counts only, never text or names.
pub(crate) fn removal_audit_metadata(
    locked: &LockedMessage,
    removal: &MessageRemoval,
) -> serde_json::Value {
    serde_json::json!({
        "conversation_id": locked.conversation.id,
        "as": if removal.as_moderator { "moderator" } else { "author" },
        "revision": locked.revision,
        "cancelled_reply_requests": removal.cancelled_reply_requests,
        "unpinned": removal.unpinned,
        "renamed_threads": removal.renamed_threads,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::repeats_suggestion;
    use crate::conversations::{suggested_thread_name, web_suggested_thread_name};

    #[test]
    fn thread_names_suggested_from_text_are_recognized() {
        let text =
            "deploy key SECRET-123 for the staging cluster, please rotate it before Friday\nmore";
        let suggestions = [suggested_thread_name(text), web_suggested_thread_name(text)]
            .into_iter()
            .map(|name| name.to_lowercase())
            .collect::<HashSet<_>>();
        let rust = suggested_thread_name(text);
        assert!(rust.ends_with('…'));
        assert!(repeats_suggestion(&rust, &suggestions));
        assert!(repeats_suggestion(&format!("{rust} (2)"), &suggestions));
        assert!(repeats_suggestion(
            &web_suggested_thread_name(text).to_uppercase(),
            &suggestions
        ));
        assert!(!repeats_suggestion("Rotation plan", &suggestions));
        assert!(!repeats_suggestion(&format!("{rust} (x)"), &suggestions));
        assert!(!repeats_suggestion(&format!("{rust} ()"), &suggestions));
    }

    #[test]
    fn web_suggestion_matches_the_dialog() {
        assert_eq!(
            web_suggested_thread_name("  Short line  \nnext"),
            "Short line"
        );
        let long = format!("{} tail", "word ".repeat(13));
        let name = web_suggested_thread_name(&long);
        assert!(name.ends_with('…'));
        assert!(!name.contains("  "));
        assert!(name.chars().count() <= 61);
        let unbroken = "x".repeat(80);
        assert_eq!(
            web_suggested_thread_name(&unbroken),
            format!("{}…", "x".repeat(60))
        );
    }
}

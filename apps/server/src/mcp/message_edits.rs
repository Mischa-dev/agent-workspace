//! message_edit, message_delete and message_history (docs/plans/mcp-tools.md,
//! decision 3). The commands are the web's: `crate::message_edits` locks the
//! message (conversation first, checked with `conversation_visible_to`), and
//! applies the edit or the removal. Edits share the web's idempotency scope
//! and stored body, so a key retried on the other surface still replays.

use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        MESSAGE_CHARACTERS, SNIPPET_CHARACTERS, ToolResult, insert_audit, mark_via,
        parse_message_id, public_url, snippet, truncate_text,
    },
    talk::{ThreadRef, message_actor, message_content},
};
use crate::{
    AppState,
    conversations::{
        ActorView, ConversationAccessRow, authorize_conversation_row_for,
        fetch_conversation_access, finish_idempotent_command,
    },
    error::AppError,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome},
    message_content::MessageContent,
    message_edits::{
        Change, ChangeError, MessageEditResponse, Refused, apply_edit, apply_removal,
        edit_request_hash, edit_scope, lock_message_for_change, message_revisions_in,
        removal_audit_metadata,
    },
    policy::Action,
};

const REMOVED_NOTE: &str = "Removed for everyone. If it held a secret, rotate the secret: anyone who already read it, and any agent that fetched it, may have a copy.";
const INVALID_CONTENT: &str =
    "The content isn't valid. Someone mentioned may have left the project.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MessageEditArguments {
    /// The message's ID, or a link to it.
    pub message_id: String,
    /// The new text. @handles of people in the project become mentions.
    pub text: Option<String>,
    /// Structured content instead of text, for exact mentions and task links.
    pub content: Option<MessageContent>,
    /// The revision you read (conversation_read shows `revision` once a
    /// message has been edited; otherwise it's 1). Leave it out to edit
    /// whatever is there.
    pub expected_revision: Option<i32>,
    /// A stable ID for this edit, so a retry never applies it twice.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MessageDeleteArguments {
    /// The message's ID, or a link to it.
    pub message_id: String,
    /// Accepted for consistency; removing is always safe to retry.
    #[allow(dead_code)]
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MessageHistoryArguments {
    /// The message's ID, or a link to it.
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct EditedMessage {
    pub conversation_id: String,
    pub message_id: String,
    /// Pass as expectedRevision next time.
    pub revision: i32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    /// False when the message already said exactly this; nothing changed.
    pub changed: bool,
    /// "Name (@handle)" of everyone this edit newly mentioned; they were
    /// notified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newly_mentioned: Vec<String>,
    /// True when this clientRequestId was already used for the same edit, so
    /// nothing changed this time.
    pub idempotent_replay: bool,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemovedMessage {
    pub conversation_id: String,
    pub message_id: String,
    pub removed_at: String,
    /// "Name (@handle)".
    pub removed_by: String,
    /// True when it was already removed; nothing changed.
    pub already_removed: bool,
    /// Agents' reply requests about this message that were cancelled.
    pub cancelled_reply_requests: i64,
    /// Threads started from this message. Names suggested from its text were
    /// replaced with a neutral one; rename any other whose name repeats what
    /// was removed with thread_update.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<ThreadRef>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageHistoryValue {
    pub conversation_id: String,
    pub message_id: String,
    /// The current revision.
    pub revision: i32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    /// Oldest first.
    pub revisions: Vec<RevisionItem>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RevisionItem {
    pub revision: i32,
    /// Empty when redacted.
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The message was removed, and this version's text with it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
    /// "Name (@handle)", with ", agent" for agents.
    pub written_by: String,
    pub written_at: String,
    /// The app a person wrote it through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

fn actor_label(actor: &ActorView) -> String {
    if actor.kind == "external_agent" {
        format!("{} (@{}, agent)", actor.display_name, actor.handle)
    } else {
        format!("{} (@{})", actor.display_name, actor.handle)
    }
}

/// The web address of a message: its conversation's page plus
/// `#message-<id>`, which message tools accept in place of the ID.
fn message_url(state: &AppState, conversation: &ConversationAccessRow, message_id: Uuid) -> String {
    let path = match (conversation.kind.as_str(), conversation.effective_kind()) {
        ("thread", "dm" | "group") => format!(
            "/messages/{}?thread={}",
            conversation.parent_channel_id.unwrap_or(conversation.id),
            conversation.id
        ),
        (_, "dm" | "group") => format!("/messages/{}", conversation.id),
        ("thread", _) => format!(
            "/projects/{}/threads/{}",
            conversation.project_id, conversation.id
        ),
        _ => format!(
            "/projects/{}/channels/{}",
            conversation.project_id, conversation.id
        ),
    };
    public_url(state, &format!("{path}#message-{message_id}"))
}

/// A refused or failed change in plain words. Errors that aren't a refusal
/// go back to the caller's `from_error`.
fn refusal<T>(error: ChangeError, change: Change) -> Result<ToolResult<T>, AppError> {
    let refused = match error {
        ChangeError::Refused(refused) => refused,
        ChangeError::App(AppError::NotFound(_)) => {
            return Ok(ToolResult::not_found("That message"));
        }
        ChangeError::App(AppError::ProjectArchived) => {
            return Ok(ToolResult::conflict(
                "That project is archived. Restore it first.",
            ));
        }
        ChangeError::App(AppError::Validation(_) | AppError::Invalid(_)) => {
            return Ok(ToolResult::invalid("content", INVALID_CONTENT));
        }
        ChangeError::App(error) => return Err(error),
    };
    Ok(match (refused, change) {
        (Refused::NotYourMessage, Change::Edit) => {
            ToolResult::denied("You can only edit your own messages.")
        }
        (Refused::NotYourMessage, Change::Remove) => ToolResult::denied(
            "You can only remove your own messages. Project owners and admins can remove anyone's.",
        ),
        (Refused::OnlyPeopleModerate, _) => ToolResult::denied(
            "Agents can only remove their own messages. Ask a project owner or admin to remove this one.",
        ),
        (Refused::RoleCannotWrite, _) => {
            ToolResult::denied("Your role in this project can't change messages.")
        }
        (Refused::Removed, _) => {
            ToolResult::conflict("That message was removed, so it can't be edited.")
        }
        (Refused::Archived, _) => ToolResult::conflict(
            "That conversation is archived. Restore it before editing messages.",
        ),
        (Refused::RevisionMoved(current), _) => ToolResult::conflict(format!(
            "This message was edited since you read it (now revision {current}). Read it again with conversation_read, then retry."
        )),
        (Refused::TooManyEdits, _) => ToolResult::conflict(
            "This message has been edited too many times. Post a new message instead.",
        ),
    })
}

fn edited(
    state: &AppState,
    conversation: &ConversationAccessRow,
    body: &MessageEditResponse,
    newly_mentioned: Vec<String>,
    idempotent_replay: bool,
) -> EditedMessage {
    EditedMessage {
        conversation_id: body.conversation_id.to_string(),
        message_id: body.message.id.to_string(),
        revision: body.message.revision,
        text: snippet(&body.message.plain_text, SNIPPET_CHARACTERS),
        edited_at: body.message.edited_at.clone(),
        changed: body.changed,
        newly_mentioned,
        idempotent_replay,
        url: message_url(state, conversation, body.message.id),
    }
}

pub(super) async fn message_edit(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MessageEditArguments,
) -> ToolResult<EditedMessage> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(message_id) = parse_message_id(&arguments.message_id) else {
        return ToolResult::invalid("messageId", "Pass a message ID or a link to it.");
    };
    let actor = match message_actor(principal) {
        Ok(actor) => actor,
        Err(message) => return ToolResult::denied(message),
    };
    let key = arguments
        .client_request_id
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let Ok(idempotency_key) = IdempotencyKey::parse(&key) else {
        return ToolResult::invalid(
            "clientRequestId",
            "Use letters, numbers, dashes or dots, up to 128 characters.",
        );
    };
    let content = match message_content(
        state,
        principal,
        arguments.text.as_deref(),
        arguments.content,
        "message_edit",
    )
    .await
    {
        Ok(content) => content,
        Err(refused) => return refused,
    };
    let Ok(content) = MessageContent::from_json(content) else {
        return ToolResult::invalid("content", INVALID_CONTENT);
    };
    let result = async {
        let request_hash = edit_request_hash(&content, arguments.expected_revision)?;
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let locked =
            match lock_message_for_change(&mut transaction, &actor, None, message_id, Change::Edit)
                .await
            {
                Ok(locked) => locked,
                Err(error) => return refusal(error, Change::Edit),
            };
        if locked.project_id != principal.project_id {
            return Ok(ToolResult::not_found("That message"));
        }
        let reservation = match idempotency::reserve(
            &mut transaction,
            principal.actor_id,
            &edit_scope(message_id)?,
            &idempotency_key,
            request_hash,
        )
        .await
        {
            Ok(IdempotencyOutcome::Replay(replay)) => {
                let body: MessageEditResponse = replay.decode()?;
                transaction.commit().await?;
                return Ok(ToolResult::ok(edited(
                    state,
                    &locked.conversation,
                    &body,
                    Vec::new(),
                    true,
                )));
            }
            Ok(IdempotencyOutcome::Reserved(reservation)) => reservation,
            Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                return Ok(ToolResult::conflict(
                    "That clientRequestId was already used for a different edit. Use a new one.",
                ));
            }
            Err(error) => return Err(error),
        };
        let edit = match apply_edit(
            &mut transaction,
            &locked,
            &actor,
            content,
            arguments.expected_revision,
        )
        .await
        {
            Ok(edit) => edit,
            Err(error) => return refusal(error, Change::Edit),
        };
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
        // The web route stores the same body under the same scope.
        finish_idempotent_command(&mut transaction, &reservation, StatusCode::OK, &body).await?;
        let newly_mentioned = sqlx::query_scalar::<_, String>(
            r#"
            SELECT display_name || ' (@' || handle || ')'
            FROM project_memberships
            WHERE project_id = $1 AND actor_id = ANY($2)
            ORDER BY lower(display_name)
            "#,
        )
        .bind(locked.project_id)
        .bind(&edit.newly_mentioned)
        .fetch_all(&mut *transaction)
        .await?;
        if edit.changed {
            insert_audit(
                &mut transaction,
                principal,
                locked.project_id,
                "conversation.mcp_message_edited",
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
        Ok(ToolResult::ok(edited(
            state,
            &locked.conversation,
            &body,
            newly_mentioned,
            false,
        )))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "message_edit"))
}

pub(super) async fn message_delete(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MessageDeleteArguments,
) -> ToolResult<RemovedMessage> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(message_id) = parse_message_id(&arguments.message_id) else {
        return ToolResult::invalid("messageId", "Pass a message ID or a link to it.");
    };
    let actor = match message_actor(principal) {
        Ok(actor) => actor,
        Err(message) => return ToolResult::denied(message),
    };
    let result = async {
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let locked = match lock_message_for_change(
            &mut transaction,
            &actor,
            None,
            message_id,
            Change::Remove,
        )
        .await
        {
            Ok(locked) => locked,
            Err(error) => return refusal(error, Change::Remove),
        };
        if locked.project_id != principal.project_id {
            return Ok(ToolResult::not_found("That message"));
        }
        let removal = match apply_removal(&mut transaction, &locked, &actor).await {
            Ok(removal) => removal,
            Err(error) => return refusal(error, Change::Remove),
        };
        if !removal.already_removed {
            insert_audit(
                &mut transaction,
                principal,
                locked.project_id,
                "conversation.mcp_message_removed",
                "message",
                locked.id,
                removal_audit_metadata(&locked, &removal),
            )
            .await?;
        }
        transaction.commit().await?;
        let removed = removal
            .entry
            .message
            .as_ref()
            .and_then(|message| message.removed.as_ref())
            .ok_or(AppError::Internal("removed entry has no removal"))?;
        Ok(ToolResult::ok(RemovedMessage {
            conversation_id: locked.conversation.id.to_string(),
            message_id: locked.id.to_string(),
            removed_at: removed.removed_at.clone(),
            removed_by: actor_label(&removed.removed_by),
            already_removed: removal.already_removed,
            cancelled_reply_requests: removal.cancelled_reply_requests,
            threads: removal
                .threads
                .into_iter()
                .map(|thread| ThreadRef {
                    id: thread.id.to_string(),
                    name: thread.name,
                    status: thread.status,
                })
                .collect(),
            note: REMOVED_NOTE.to_owned(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "message_delete"))
}

pub(super) async fn message_history(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MessageHistoryArguments,
) -> ToolResult<MessageHistoryValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(message_id) = parse_message_id(&arguments.message_id) else {
        return ToolResult::invalid("messageId", "Pass a message ID or a link to it.");
    };
    let result = async {
        let Some(conversation_id) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT conversation_id FROM messages
            WHERE id = $1 AND project_id = $2 AND conversation_visible_to(conversation_id, $3)
            "#,
        )
        .bind(message_id)
        .bind(principal.project_id)
        .bind(principal.actor_id)
        .fetch_optional(&state.pool)
        .await?
        else {
            return Ok(ToolResult::not_found("That message"));
        };
        // Archived conversations stay readable here, as on the web.
        let Some(conversation) =
            fetch_conversation_access(&state.pool, principal.actor_id, conversation_id).await?
        else {
            return Ok(ToolResult::not_found("That message"));
        };
        if conversation.project_id != principal.project_id
            || authorize_conversation_row_for(
                principal.actor_id,
                principal.actor_kind,
                &conversation,
                Action::McpRead,
                true,
            )
            .is_err()
        {
            return Ok(ToolResult::not_found("That message"));
        }
        let Some(history) = message_revisions_in(&state.pool, &conversation, message_id).await?
        else {
            return Ok(ToolResult::not_found("That message"));
        };
        Ok::<_, AppError>(ToolResult::ok(MessageHistoryValue {
            conversation_id: conversation.id.to_string(),
            message_id: message_id.to_string(),
            revision: history.revision,
            removed: history.removed,
            revisions: history
                .revisions
                .into_iter()
                .map(|revision| {
                    let (text, truncated) = if revision.redacted {
                        (String::new(), false)
                    } else {
                        truncate_text(
                            revision.plain_text.as_deref().unwrap_or_default(),
                            MESSAGE_CHARACTERS,
                        )
                    };
                    RevisionItem {
                        revision: revision.revision,
                        text,
                        truncated,
                        redacted: revision.redacted,
                        written_by: actor_label(&revision.written_by),
                        written_at: revision.written_at,
                        via: revision.via,
                    }
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "message_history"))
}

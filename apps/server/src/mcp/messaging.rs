//! The message commands as operations run them (K2.7). Each one is the
//! command the hand-written tool ran, now called from
//! `conversations::ops` with the caller the dispatcher checked: the same
//! checks inside, the same answers, the same idempotency scopes as the
//! web's (a request id retried on either surface replays). What comes back
//! is the operation's result or its error.

use std::borrow::Cow;

use uuid::Uuid;

use super::{
    McpPrincipal,
    common::ToolResult,
    later_tools::{self, MessageScheduleArguments, ScheduledUpdateArguments},
    message_edits::{
        self, MessageDeleteArguments, MessageEditArguments, MessageHistoryArguments,
        MessagePreviewHideArguments,
    },
    reactions::{self, MessageReactArguments},
    talk::{self, MarkReadArguments, MessageForwardArguments, MessageSendArguments},
};
pub use super::{
    later_tools::{ScheduledUpdateValue, ScheduledValue},
    message_edits::{EditedMessage, HiddenPreviews, MessageHistoryValue, RemovedMessage},
    reactions::MessageReactionValue,
    talk::{ReadMark, SentMessage},
};
use crate::{
    AppState,
    message_content::MessageContent,
    ops::{ConflictReason, DenyReason, Edge, OpContext, OpError},
};

/// The caller as the commands read it: the account, the credential it came
/// through, and the space dispatch located the target in.
pub(crate) fn principal_of(ctx: &OpContext) -> McpPrincipal {
    let (credential_id, oauth_grant_id, client_name, authentication_class) = match &ctx.edge {
        Edge::Mcp {
            credential_id,
            oauth_grant_id,
            client_name,
            authentication_class,
            ..
        }
        | Edge::PublicApi {
            credential_id,
            oauth_grant_id,
            client_name,
            authentication_class,
            ..
        } => (
            *credential_id,
            *oauth_grant_id,
            client_name.clone(),
            *authentication_class,
        ),
        Edge::Web { .. } => (None, None, "web".to_owned(), "password_session"),
        Edge::Automation { .. } => (None, None, "automation".to_owned(), "automation"),
        Edge::Job { source, .. } => (None, None, (*source).to_owned(), "background_job"),
        Edge::Proposal { .. } => (None, None, "proposal".to_owned(), "proposal"),
    };
    let mut scopes = Vec::new();
    if ctx.caller.can_read {
        scopes.push("read".to_owned());
    }
    if ctx.caller.can_write {
        scopes.push("write".to_owned());
    }
    McpPrincipal {
        credential_id: credential_id.unwrap_or_else(Uuid::nil),
        mcp_credential_id: credential_id,
        oauth_grant_id: oauth_grant_id.or(ctx.caller.via_grant_id),
        actor_id: ctx.caller.account_id,
        actor_kind: ctx.caller.kind,
        // A credential held to one space is that space's, as it was before
        // the operation located the call at the workspace.
        // A DM or group is found in the direct scope.
        project_id: ctx
            .space_id
            .or(ctx.caller.confined_space)
            .or(ctx.direct.map(|_| crate::spaces::DIRECT))
            .unwrap_or_else(Uuid::nil),
        external_agent_connection_id: ctx.caller.connection_id,
        client_name,
        scopes,
        authentication_class,
        account_wide: ctx.caller.confined_space.is_none(),
        project_explicit: true,
    }
}

/// A command's answer as an operation's. `what` names the thing a
/// not-found answer is about.
fn answer<T>(result: ToolResult<T>, what: &'static str) -> Result<T, OpError> {
    match result {
        ToolResult::Ok { value } => Ok(value),
        ToolResult::NotFound { .. } => Err(OpError::not_found(what)),
        ToolResult::InvalidRequest { field, message } => Err(OpError::Invalid {
            field: Cow::Owned(field),
            message: Cow::Owned(message),
        }),
        ToolResult::PermissionDenied {
            message,
            retry_category,
        } => Err(OpError::Denied {
            reason: if retry_category.as_deref() == Some("module") {
                DenyReason::ModuleOff {
                    module: "messages".to_owned(),
                }
            } else {
                DenyReason::Rule
            },
            message: Cow::Owned(message),
        }),
        ToolResult::Conflict { message } => Err(OpError::Conflict {
            reason: if message.contains("clientRequestId") {
                ConflictReason::RequestIdReused
            } else {
                ConflictReason::State { code: "state" }
            },
            message: Cow::Owned(message),
            current: None,
        }),
        ToolResult::TemporarilyUnavailable { .. } => Err(OpError::Unavailable),
    }
}

/// What a new message says: plain text, or structured content.
pub(crate) struct Words {
    pub text: Option<String>,
    pub content: Option<MessageContent>,
}

pub(crate) async fn send(
    state: &AppState,
    ctx: &OpContext,
    conversation_id: Uuid,
    words: Words,
    attachment_ids: Option<Vec<String>>,
    reply_to_message_id: Option<Uuid>,
) -> Result<SentMessage, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessageSendArguments {
        conversation_id: conversation_id.to_string(),
        text: words.text,
        content: words.content,
        attachment_ids,
        reply_to_message_id: reply_to_message_id.map(|id| id.to_string()),
        client_request_id: ctx.request_id.clone(),
    };
    answer(
        talk::message_send(state, &principal, arguments).await,
        "That conversation",
    )
}

pub(crate) async fn forward(
    state: &AppState,
    ctx: &OpContext,
    message_id: Uuid,
    conversation_id: Uuid,
    text: Option<String>,
) -> Result<SentMessage, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessageForwardArguments {
        message_id: message_id.to_string(),
        conversation_id: conversation_id.to_string(),
        text,
        client_request_id: ctx.request_id.clone(),
    };
    answer(
        talk::message_forward(state, &principal, arguments).await,
        "That message or conversation",
    )
}

pub(crate) async fn mark_read(
    state: &AppState,
    ctx: &OpContext,
    conversation_id: Uuid,
    up_to_sequence: Option<i64>,
    unread: Option<bool>,
    from_message_id: Option<Uuid>,
) -> Result<ReadMark, OpError> {
    let principal = principal_of(ctx);
    let arguments = MarkReadArguments {
        conversation_id: conversation_id.to_string(),
        up_to_sequence,
        unread,
        from_message_id: from_message_id.map(|id| id.to_string()),
    };
    answer(
        talk::mark_read(state, &principal, arguments).await,
        "That conversation",
    )
}

pub(crate) async fn edit(
    state: &AppState,
    ctx: &OpContext,
    message_id: Uuid,
    words: Words,
    expected_revision: Option<i32>,
) -> Result<EditedMessage, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessageEditArguments {
        message_id: message_id.to_string(),
        text: words.text,
        content: words.content,
        expected_revision,
        client_request_id: ctx.request_id.clone(),
    };
    answer(
        message_edits::message_edit(state, &principal, arguments).await,
        "That message",
    )
}

pub(crate) async fn delete(
    state: &AppState,
    ctx: &OpContext,
    message_id: Uuid,
) -> Result<RemovedMessage, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessageDeleteArguments {
        message_id: message_id.to_string(),
        client_request_id: None,
    };
    answer(
        message_edits::message_delete(state, &principal, arguments).await,
        "That message",
    )
}

pub(crate) async fn preview_hide(
    state: &AppState,
    ctx: &OpContext,
    message_id: Uuid,
    url: Option<String>,
) -> Result<HiddenPreviews, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessagePreviewHideArguments {
        message_id: message_id.to_string(),
        url,
    };
    answer(
        message_edits::message_preview_hide(state, &principal, arguments).await,
        "A link preview on that message",
    )
}

pub(crate) async fn history(
    state: &AppState,
    ctx: &OpContext,
    message_id: Uuid,
) -> Result<MessageHistoryValue, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessageHistoryArguments {
        message_id: message_id.to_string(),
    };
    answer(
        message_edits::message_history(state, &principal, arguments).await,
        "That message",
    )
}

pub(crate) async fn react(
    state: &AppState,
    ctx: &OpContext,
    message_id: Uuid,
    emoji: String,
    reacted: Option<bool>,
) -> Result<MessageReactionValue, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessageReactArguments {
        message_id: message_id.to_string(),
        emoji,
        reacted,
    };
    answer(
        reactions::message_react(state, &principal, arguments).await,
        "That message",
    )
}

pub(crate) async fn schedule(
    state: &AppState,
    ctx: &OpContext,
    conversation_id: Uuid,
    words: Words,
    send_at: String,
) -> Result<ScheduledValue, OpError> {
    let principal = principal_of(ctx);
    let arguments = MessageScheduleArguments {
        conversation_id: conversation_id.to_string(),
        text: words.text,
        content: words.content,
        send_at,
        client_request_id: ctx.request_id.clone(),
    };
    answer(
        later_tools::message_schedule(state, &principal, arguments).await,
        "That conversation or scheduled message",
    )
}

/// What a change to a scheduled message does.
pub(crate) struct ScheduledChange {
    pub text: Option<String>,
    pub send_at: Option<String>,
    pub send_now: Option<bool>,
    pub cancel: Option<bool>,
}

pub(crate) async fn scheduled_update(
    state: &AppState,
    ctx: &OpContext,
    id: Uuid,
    change: ScheduledChange,
) -> Result<ScheduledUpdateValue, OpError> {
    let principal = principal_of(ctx);
    let arguments = ScheduledUpdateArguments {
        id: id.to_string(),
        text: change.text,
        send_at: change.send_at,
        send_now: change.send_now,
        cancel: change.cancel,
    };
    answer(
        later_tools::scheduled_message_update(state, &principal, arguments).await,
        "That scheduled message",
    )
}

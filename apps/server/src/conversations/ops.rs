//! Messages as operations (K2.7): send, update, delete, react, forward,
//! mark read, history, schedule, change a scheduled message, and hide link
//! previews. Agents call them as tools of the same names (`message_edit`
//! and `mark_read` stay as aliases), scripts through the v2 route, and
//! rules as actions. Each runs the message command the web runs
//! (`mcp::messaging`), which keeps its own idempotency (the web's scopes,
//! so a request id retried on either surface replays) and its own checks
//! inside: who may edit, remove or react, archived conversations, and
//! `conversation_visible_to` on everything it reads.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    mcp::messaging::{
        self, EditedMessage, HiddenPreviews, MessageHistoryValue, MessageReactionValue, ReadMark,
        RemovedMessage, ScheduledChange, ScheduledUpdateValue, ScheduledValue, SentMessage, Words,
    },
    message_content::MessageContent,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::{IdRef, MessageRef},
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<MessageSend>()
        .add::<MessageForward>()
        .add::<MessageUpdate>()
        .add::<MessageDelete>()
        .add::<MessagePreviewHide>()
        .add::<MessageHistory>()
        .add::<MessageReact>()
        .add::<ConversationMarkRead>()
        .add::<ConversationMemberList>()
        .add::<ConversationTaskList>()
        .add::<ConversationFileList>()
        .add::<MessageSchedule>()
        .add::<ScheduledMessageUpdate>();
}

/// A message's thing kind that finds it after it was removed too, for
/// what still applies to a removed message (its history, removing it again).
const ANY_MESSAGE: &str = "message_record";

// ---------------------------------------------------------------------------
// message_send

pub struct MessageSend;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSendInput {
    /// The channel, thread, DM or group: its id or a link to it.
    pub conversation_id: IdRef,
    /// The message as plain text. @handles of people in the space become
    /// mentions and notify them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Structured content instead of text, for exact mentions and task links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// Files to attach (up to 10), uploaded first with file_upload to this
    /// conversation. With files, text may be left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<Vec<IdRef>>,
    /// Reply to this message in the same conversation: it shows quoted
    /// above yours, with a jump back to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<MessageRef>,
}

impl Targeted for MessageSendInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

impl Operation for MessageSend {
    const NAME: &'static str = "message_send";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Send message";
    const DESCRIPTION: &'static str = "Send a message to a channel, thread, DM or group. Write \
        plain text; @handles of people in the space become mentions and notify them. To answer \
        one message there, pass replyToMessageId: it shows quoted above yours.";
    const EMITS: &'static [&'static str] = &["message.created"];

    type Input = MessageSendInput;
    type Output = SentMessage;

    async fn run(call: &mut OpCall<'_>, input: MessageSendInput) -> Result<SentMessage, OpError> {
        let keyed = call.ctx.request_id.is_some();
        let sent = messaging::send(
            call.state,
            call.ctx,
            input.conversation_id.id(),
            Words {
                text: input.text,
                content: input.content,
            },
            input
                .attachment_ids
                .map(|ids| ids.into_iter().map(|id| id.id().to_string()).collect()),
            input.reply_to_message_id.map(MessageRef::id),
        )
        .await?;
        if keyed {
            call.key_honoured();
        }
        call.mark_replayed(sent.idempotent_replay);
        if sent.idempotent_replay {
            // A replay changed nothing: no second audit row.
            call.audited_by_command();
        }
        if let Ok(id) = sent.message_id.parse() {
            call.audit_target("message", id, json!({}));
        }
        Ok(sent)
    }
}

// ---------------------------------------------------------------------------
// message_forward

pub struct MessageForward;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageForwardInput {
    /// The message to forward. You must be able to see it.
    pub message_id: MessageRef,
    /// Where it goes: a channel, thread, DM or group you can post in, other
    /// than the one it's in.
    pub conversation_id: IdRef,
    /// An optional comment sent with it, as plain text (up to 4,000
    /// characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Targeted for MessageForwardInput {
    /// Where it goes; the original is checked inside, as the caller may
    /// read it, and is `not_found` otherwise.
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

impl Operation for MessageForward {
    const NAME: &'static str = "message_forward";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Forward message";
    const DESCRIPTION: &'static str = "Forward a message you can see into another channel, \
        thread, DM or group you can post in. It shows there quoted, with who wrote it and where, \
        and a link back that works for whoever can see the original. Add an optional plain-text \
        comment.";
    const EMITS: &'static [&'static str] = &["message.created"];

    type Input = MessageForwardInput;
    type Output = SentMessage;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageForwardInput,
    ) -> Result<SentMessage, OpError> {
        let keyed = call.ctx.request_id.is_some();
        let sent = messaging::forward(
            call.state,
            call.ctx,
            input.message_id.id(),
            input.conversation_id.id(),
            input.text,
        )
        .await?;
        if keyed {
            call.key_honoured();
        }
        call.mark_replayed(sent.idempotent_replay);
        if sent.idempotent_replay {
            // A replay changed nothing: no second audit row.
            call.audited_by_command();
        }
        if let Ok(id) = sent.message_id.parse() {
            call.audit_target("message", id, json!({}));
        }
        Ok(sent)
    }
}

// ---------------------------------------------------------------------------
// message_update (alias message_edit)

pub struct MessageUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageUpdateInput {
    /// The message: its id, or a link to it.
    pub message_id: MessageRef,
    /// The new text. @handles of people in the space become mentions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Structured content instead of text, for exact mentions and task links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// The revision you read (conversation_read shows `revision` once a
    /// message has been edited; otherwise it's 1). Leave it out to edit
    /// whatever is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i32>,
}

impl Targeted for MessageUpdateInput {
    fn target(&self) -> Target {
        Target::thing(ANY_MESSAGE, self.message_id.id())
    }
}

impl Operation for MessageUpdate {
    const NAME: &'static str = "message_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Edit message";
    const DESCRIPTION: &'static str = "Edit your own message. The old text is kept as a revision \
        that everyone in the conversation can see, and the message shows (edited). Whoever you \
        newly @mention is notified; mentions you take out aren't un-notified. To take down a \
        secret, use message_delete instead: editing keeps the old text in the history.";
    const EMITS: &'static [&'static str] = &["message.updated"];

    type Input = MessageUpdateInput;
    type Output = EditedMessage;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageUpdateInput,
    ) -> Result<EditedMessage, OpError> {
        let keyed = call.ctx.request_id.is_some();
        let message_id = input.message_id.id();
        let edited = messaging::edit(
            call.state,
            call.ctx,
            message_id,
            Words {
                text: input.text,
                content: input.content,
            },
            input.expected_revision,
        )
        .await?;
        if keyed {
            call.key_honoured();
        }
        call.mark_replayed(edited.idempotent_replay);
        if edited.idempotent_replay {
            // A replay changed nothing: no second audit row.
            call.audited_by_command();
        }
        // The command wrote the audit row (with the revision).
        call.audited_by_command();
        Ok(edited)
    }
}

// ---------------------------------------------------------------------------
// message_delete

pub struct MessageDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeleteInput {
    /// The message: its id, or a link to it.
    pub message_id: MessageRef,
}

impl Targeted for MessageDeleteInput {
    fn target(&self) -> Target {
        Target::thing(ANY_MESSAGE, self.message_id.id())
    }
}

impl Operation for MessageDelete {
    const NAME: &'static str = "message_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Remove message";
    const DESCRIPTION: &'static str = "Remove a message for everyone, leaving \"message removed\" \
        in its place. Its text and every earlier version are deleted, it is unpinned, and \
        pending reply requests about it are cancelled. You can remove your own messages; space \
        owners and admins can remove anyone's, for example a leaked token. This can't be undone. \
        Rotate any secret it held.";
    const EMITS: &'static [&'static str] = &["message.removed"];

    type Input = MessageDeleteInput;
    type Output = RemovedMessage;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageDeleteInput,
    ) -> Result<RemovedMessage, OpError> {
        let message_id = input.message_id.id();
        let removed = messaging::delete(call.state, call.ctx, message_id).await?;
        // Removing what is already removed changes nothing; a removal
        // wrote its own audit row.
        call.key_honoured();
        call.audited_by_command();
        Ok(removed)
    }
}

// ---------------------------------------------------------------------------
// message_preview_hide

pub struct MessagePreviewHide;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessagePreviewHideInput {
    /// The message: its id, or a link to it.
    pub message_id: MessageRef,
    /// Only the preview for this link, as written in the message. Left out,
    /// every preview on the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Targeted for MessagePreviewHideInput {
    fn target(&self) -> Target {
        Target::thing("message", self.message_id.id())
    }
}

impl Operation for MessagePreviewHide {
    const NAME: &'static str = "message_preview_hide";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Hide link previews";
    const DESCRIPTION: &'static str = "Hide the link previews under a message, for everyone, as \
        the remove button on a preview does: the one for url, or all of them. Your own messages, \
        or anyone's if you own or administer the space. The message and its links stay.";
    const EMITS: &'static [&'static str] = &["message.updated"];

    type Input = MessagePreviewHideInput;
    type Output = HiddenPreviews;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessagePreviewHideInput,
    ) -> Result<HiddenPreviews, OpError> {
        let message_id = input.message_id.id();
        let hidden = messaging::preview_hide(call.state, call.ctx, message_id, input.url).await?;
        // Hiding what is hidden changes nothing.
        call.key_honoured();
        Ok(hidden)
    }
}

// ---------------------------------------------------------------------------
// message_history

pub struct MessageHistory;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageHistoryInput {
    /// The message: its id, or a link to it.
    pub message_id: MessageRef,
}

impl Targeted for MessageHistoryInput {
    fn target(&self) -> Target {
        Target::thing(ANY_MESSAGE, self.message_id.id())
    }
}

impl Operation for MessageHistory {
    const NAME: &'static str = "message_history";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Message history";
    const DESCRIPTION: &'static str = "Every version of a message, oldest first: who wrote each \
        one, when, and through which app. Versions of a removed message show as redacted.";

    type Input = MessageHistoryInput;
    type Output = MessageHistoryValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageHistoryInput,
    ) -> Result<MessageHistoryValue, OpError> {
        messaging::history(call.state, call.ctx, input.message_id.id()).await
    }
}

// ---------------------------------------------------------------------------
// message_react

pub struct MessageReact;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageReactInput {
    /// The message: its id, or a link to it.
    pub message_id: MessageRef,
    /// One emoji, or its name: 👍 thumbsup, 👎 thumbsdown, ❤️ heart, 😄 smile,
    /// 🎉 tada, 😕 confused, 🚀 rocket, 👀 eyes, ✅ check, ❌ x, 🙏 pray,
    /// 🤔 thinking.
    pub emoji: String,
    /// False to take your reaction away. Defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reacted: Option<bool>,
}

impl Targeted for MessageReactInput {
    fn target(&self) -> Target {
        // A removed message too: the command answers that its reactions
        // are frozen.
        Target::thing(ANY_MESSAGE, self.message_id.id())
    }
}

impl Operation for MessageReact {
    const NAME: &'static str = "message_react";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    // The command checks who may post there, in its own words.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "React to message";
    const DESCRIPTION: &'static str = "Add an emoji reaction to a message, or take yours away \
        with reacted: false. It states a result rather than toggling, so a retry changes \
        nothing. Use 👀 when you start on something and ✅ when it's done, instead of a message \
        that only acknowledges.";
    const EMITS: &'static [&'static str] = &["message.reacted"];

    type Input = MessageReactInput;
    type Output = MessageReactionValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageReactInput,
    ) -> Result<MessageReactionValue, OpError> {
        let message_id = input.message_id.id();
        let reacted =
            messaging::react(call.state, call.ctx, message_id, input.emoji, input.reacted).await?;
        call.key_honoured();
        // The command wrote the audit row.
        call.audited_by_command();
        Ok(reacted)
    }
}

// ---------------------------------------------------------------------------
// conversation_mark_read (alias mark_read)

pub struct ConversationMarkRead;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMarkReadInput {
    /// The conversation: its id, or a link to it.
    pub conversation_id: IdRef,
    /// Mark read up to this sequence number. Defaults to the newest message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to_sequence: Option<i64>,
    /// true: make the conversation unread again from fromMessageId (default:
    /// its newest message) onward, to come back to it later. Nothing gets
    /// marked read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<bool>,
    /// With unread: the first message to show as unread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_message_id: Option<MessageRef>,
}

impl Targeted for ConversationMarkReadInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

impl Operation for ConversationMarkRead {
    const NAME: &'static str = "conversation_mark_read";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Mark read";
    const DESCRIPTION: &'static str = "Mark a conversation read, up to its newest message or a \
        given sequence, so it stops showing as unread in your briefing; its mention \
        notifications are read with it. To come back to something later, pass unread: true: \
        the conversation is unread again from fromMessageId (default: its newest message) \
        onward. Marking unread never marks anything read.";
    const EMITS: &'static [&'static str] = &["conversation.read"];

    type Input = ConversationMarkReadInput;
    type Output = ReadMark;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConversationMarkReadInput,
    ) -> Result<ReadMark, OpError> {
        let marked = messaging::mark_read(
            call.state,
            call.ctx,
            input.conversation_id.id(),
            input.up_to_sequence,
            input.unread,
            input.from_message_id.map(MessageRef::id),
        )
        .await?;
        // Marking the same point again changes nothing.
        call.key_honoured();
        Ok(marked)
    }
}

// ---------------------------------------------------------------------------
// message_schedule

pub struct MessageSchedule;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageScheduleInput {
    /// The channel, thread, DM or group: its id or a link to it.
    pub conversation_id: IdRef,
    /// The message as plain text. @handles of people in the space become
    /// mentions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Structured content instead of text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// When to send it, as RFC 3339, up to a year ahead. It sends as you,
    /// checked then as if you sent it then.
    pub send_at: String,
}

impl Targeted for MessageScheduleInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

impl Operation for MessageSchedule {
    const NAME: &'static str = "message_schedule";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Schedule message";
    const DESCRIPTION: &'static str = "Schedule a message to send later in a channel, thread, DM \
        or group, as you, checked when it sends as if you sent it then. later_list shows what \
        is waiting; change, send or cancel one with scheduled_message_update.";
    const EMITS: &'static [&'static str] = &["scheduled_message.created"];

    type Input = MessageScheduleInput;
    type Output = ScheduledValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageScheduleInput,
    ) -> Result<ScheduledValue, OpError> {
        let keyed = call.ctx.request_id.is_some();
        let mut scheduled = messaging::schedule(
            call.state,
            call.ctx,
            input.conversation_id.id(),
            Words {
                text: input.text,
                content: input.content,
            },
            input.send_at,
        )
        .await?;
        if keyed {
            call.key_honoured();
        }
        call.mark_replayed(scheduled.idempotent_replay);
        if scheduled.idempotent_replay {
            // A replay changed nothing: no second audit row.
            call.audited_by_command();
        }
        // The envelope says it replayed; the value is the first call's.
        scheduled.idempotent_replay = false;
        if let Ok(id) = scheduled.scheduled.id.parse() {
            call.audit_target("scheduled_message", id, json!({}));
        }
        Ok(scheduled)
    }
}

// ---------------------------------------------------------------------------
// scheduled_message_update

pub struct ScheduledMessageUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledMessageUpdateInput {
    /// The scheduled message, from message_schedule or later_list.
    pub id: IdRef,
    /// New text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A new time, as RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_at: Option<String>,
    /// true: send it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_now: Option<bool>,
    /// true: don't send it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
}

impl Targeted for ScheduledMessageUpdateInput {
    /// Only its author finds it.
    fn target(&self) -> Target {
        Target::thing("scheduled_message", self.id.id())
    }
}

impl Operation for ScheduledMessageUpdate {
    const NAME: &'static str = "scheduled_message_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Change scheduled message";
    const DESCRIPTION: &'static str = "Change a message you scheduled: its text or time \
        (sendAt), send it now (sendNow: true), or cancel it (cancel: true). A failed one is \
        scheduled again when you give it a new time.";
    const EMITS: &'static [&'static str] = &["scheduled_message.updated"];

    type Input = ScheduledMessageUpdateInput;
    type Output = ScheduledUpdateValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: ScheduledMessageUpdateInput,
    ) -> Result<ScheduledUpdateValue, OpError> {
        let id = input.id.id();
        // The scheduled-message command keeps no request ids, so a retry
        // after a later edit would write again: replaySafe stays false.
        let updated = messaging::scheduled_update(
            call.state,
            call.ctx,
            id,
            ScheduledChange {
                text: input.text,
                send_at: input.send_at,
                send_now: input.send_now,
                cancel: input.cancel,
            },
        )
        .await?;
        call.audit_target("scheduled_message", id, json!({}));
        Ok(updated)
    }
}

// ---------------------------------------------------------------------------
// conversation_member_list

pub struct ConversationMemberList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMemberListInput {
    /// A private channel or a group, or a thread in one: its id or a link.
    pub conversation_id: IdRef,
}

impl Targeted for ConversationMemberListInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMember {
    #[schemars(with = "String")]
    pub id: uuid::Uuid,
    pub display_name: String,
    pub handle: String,
    /// The account's kind, for showing it.
    pub kind: String,
    /// Changes who's in a private channel.
    pub manager: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMembers {
    /// The channel or group whose members these are, even for a thread.
    #[schemars(with = "String")]
    pub conversation_id: uuid::Uuid,
    pub members: Vec<ConversationMember>,
    /// Whether you can add and remove others (conversation_members).
    pub can_manage: bool,
}

impl Operation for ConversationMemberList {
    const NAME: &'static str = "conversation_member_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List members";
    const DESCRIPTION: &'static str = "Who is in a private channel or a group (or the one a \
        thread is in), which of them manage a private channel, and whether you can add and \
        remove others with conversation_members. An open channel has no member list: everyone \
        in its space reads it.";

    type Input = ConversationMemberListInput;
    type Output = ConversationMembers;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConversationMemberListInput,
    ) -> Result<ConversationMembers, OpError> {
        let caller = call.ctx.caller.account_id;
        let kind = call.ctx.caller.kind;
        let tx = call.tx().await?;
        let row = crate::conversations::read_conversation_access(
            &mut **tx,
            caller,
            input.conversation_id.id(),
        )
        .await?
        .ok_or_else(|| OpError::not_found("That conversation"))?;
        crate::conversations::authorize_conversation_row_for(
            caller,
            kind,
            &row,
            crate::policy::Action::ProjectView,
            true,
        )?;
        let holder = match (row.kind.as_str(), row.parent_kind.as_deref()) {
            ("thread", Some(_)) => row.parent_channel_id.unwrap_or(row.id),
            _ => row.id,
        };
        let has_members = row.private || matches!(row.effective_kind(), "dm" | "group");
        if !has_members {
            return Err(OpError::invalid(
                "conversationId",
                "This channel is open to everyone in its space, so it has no member list.",
            ));
        }
        let members = sqlx::query_as::<_, (uuid::Uuid, String, String, String, bool)>(
            r#"
            SELECT actor.id, membership.display_name, membership.handle, actor.kind,
                   member.manager
            FROM conversation_members member
            JOIN actors actor ON actor.id = member.actor_id
            JOIN project_memberships membership
              ON membership.project_id = member.project_id
             AND membership.actor_id = member.actor_id
            WHERE member.conversation_id = $1
              AND conversation_visible_to($1, $2)
            ORDER BY member.manager DESC, lower(membership.display_name), actor.id
            "#,
        )
        .bind(holder)
        .bind(caller)
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        let is_manager = members
            .iter()
            .any(|(id, _, _, _, manager)| *id == caller && *manager);
        let can_manage = if row.private {
            is_manager || row.allows(caller, kind, crate::policy::Action::MessageModerate)?
        } else {
            row.allows(caller, kind, crate::policy::Action::ChannelCreate)?
        };
        Ok(ConversationMembers {
            conversation_id: holder,
            members: members
                .into_iter()
                .map(
                    |(id, display_name, handle, kind, manager)| ConversationMember {
                        id,
                        display_name,
                        handle,
                        kind,
                        manager,
                    },
                )
                .collect(),
            can_manage,
        })
    }
}

// ---------------------------------------------------------------------------
// conversation_task_list

/// The tasks a conversation is about or made (C2.8's Tasks tab).
pub struct ConversationTaskList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationTaskListInput {
    /// The channel, thread, DM or group: its id or a link.
    pub conversation_id: IdRef,
}

impl Targeted for ConversationTaskListInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTask {
    #[schemars(with = "String")]
    pub id: uuid::Uuid,
    /// Such as AWD-12.
    pub key: String,
    pub title: String,
    /// backlog, unstarted, started, completed or canceled.
    pub status_category: String,
    /// made_from: made from a message here; about: linked to the conversation.
    pub how: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTasks {
    /// Newest link first; archived tasks are left out.
    pub tasks: Vec<ConversationTask>,
}

impl Operation for ConversationTaskList {
    const NAME: &'static str = "conversation_task_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksView);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List a conversation's tasks";
    const DESCRIPTION: &'static str = "The tasks linked to a conversation or made from one of \
        its messages, in the conversation's own space, newest first. task_open opens one.";

    type Input = ConversationTaskListInput;
    type Output = ConversationTasks;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConversationTaskListInput,
    ) -> Result<ConversationTasks, OpError> {
        let caller = call.ctx.caller.account_id;
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let rows = sqlx::query_as::<_, (uuid::Uuid, String, String, String, String)>(
            r#"
            WITH linked AS (
                SELECT link.work_item_id, 'about' AS how, link.created_at
                FROM work_item_links link
                WHERE link.target_kind = 'conversation' AND link.target_id = $1
                UNION ALL
                SELECT link.work_item_id, 'about', link.created_at
                FROM work_item_conversation_links link
                WHERE link.conversation_id = $1
                UNION ALL
                SELECT link.work_item_id, 'made_from', link.created_at
                FROM work_item_links link
                JOIN messages message ON message.id = link.target_id
                WHERE link.target_kind = 'message' AND message.conversation_id = $1
                  AND message.removed_at IS NULL
            ),
            newest AS (
                SELECT DISTINCT ON (work_item_id) work_item_id, how, created_at
                FROM linked
                ORDER BY work_item_id, created_at DESC
            )
            SELECT item.id, project.project_key || '-' || item.number::text, item.title,
                   status.semantic_category, newest.how
            FROM newest
            JOIN task_rows item ON item.id = newest.work_item_id
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            WHERE item.project_id = $3 AND item.archived_at IS NULL
              AND conversation_visible_to($1, $2)
            ORDER BY newest.created_at DESC, item.id
            LIMIT 200
            "#,
        )
        .bind(input.conversation_id.id())
        .bind(caller)
        .bind(space)
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        Ok(ConversationTasks {
            tasks: rows
                .into_iter()
                .map(|(id, key, title, status_category, how)| ConversationTask {
                    id,
                    key,
                    title,
                    status_category,
                    how,
                })
                .collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// conversation_file_list

/// The files sent in a conversation (C2.8's Files tab).
pub struct ConversationFileList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationFileListInput {
    /// The channel, thread, DM or group: its id or a link.
    pub conversation_id: IdRef,
}

impl Targeted for ConversationFileListInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationFile {
    #[schemars(with = "String")]
    pub id: uuid::Uuid,
    pub name: String,
    pub content_type: String,
    pub size: i64,
    /// Set for images that show inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    /// The message it was sent with.
    #[schemars(with = "String")]
    pub message_id: uuid::Uuid,
    pub created_at: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationFiles {
    /// Newest first, at most 200; files of removed messages are left out.
    pub files: Vec<ConversationFile>,
}

impl Operation for ConversationFileList {
    const NAME: &'static str = "conversation_file_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List a conversation's files";
    const DESCRIPTION: &'static str = "The files sent in a conversation, newest first, with \
        the message each came with. file_read opens one.";

    type Input = ConversationFileListInput;
    type Output = ConversationFiles;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConversationFileListInput,
    ) -> Result<ConversationFiles, OpError> {
        let caller = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let rows = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                String,
                String,
                i64,
                Option<i32>,
                Option<i32>,
                uuid::Uuid,
                String,
            ),
        >(
            r#"
            SELECT file.id, file.name, file.content_type, file.byte_size, file.image_width,
                   file.image_height, file.message_id,
                   to_char(file.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
            FROM files file
            JOIN messages message ON message.id = file.message_id
            WHERE file.conversation_id = $1 AND file.removed_at IS NULL
              AND file.attached_at IS NOT NULL AND message.removed_at IS NULL
              AND conversation_visible_to($1, $2)
            ORDER BY file.created_at DESC, file.id
            LIMIT 200
            "#,
        )
        .bind(input.conversation_id.id())
        .bind(caller)
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        Ok(ConversationFiles {
            files: rows
                .into_iter()
                .map(
                    |(id, name, content_type, size, width, height, message_id, created_at)| {
                        ConversationFile {
                            id,
                            name,
                            content_type,
                            size,
                            width,
                            height,
                            message_id,
                            created_at,
                        }
                    },
                )
                .collect(),
        })
    }
}

//! Threads and conversations as operations (K2.8): start and update a
//! thread, make a channel, change a conversation, pin, change a group's
//! members, and let a group's agents continue. Agents call them as tools of
//! the same names (the old `conversation_members` is retired), scripts
//! through the v2 route, and rules as actions. Each runs the command the
//! hand-written tool ran (`mcp::talk`, `mcp::channels`, `mcp::groups`), as
//! the caller the dispatcher checked, which keeps its own checks inside:
//! who may change what, archived conversations, and
//! `conversation_visible_to` on everything it reads. Each command writes its
//! own audit row.
//!
//! Starting a DM or group (`conversation_start`) stays a hand-written tool
//! until DMs have their own scope (K4.24): it acts in the caller's Messages
//! space, which no workspace names.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    mcp::{
        channels::{
            self, ChannelCreateArguments, ConversationUpdateArguments, ConversationValue,
            PinSetArguments, PinValue,
        },
        groups::{
            self, ContinueValue, ConversationContinueArguments, ConversationMembersArguments,
            GroupMembersValue,
        },
        talk::{self, ThreadStartArguments, ThreadUpdateArguments, ThreadValue},
        task_commands::{principal_for, tool_answer},
    },
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::parse_id,
    },
};

/// The Channels module's thread and channel operations.
pub fn register_channel_ops(registry: &mut Registry) {
    registry
        .add::<ThreadStart>()
        .add::<ThreadUpdate>()
        .add::<ChannelCreate>();
}

/// The Messages module's conversation operations.
pub fn register_conversation_ops(registry: &mut Registry) {
    registry
        .add::<ConversationUpdate>()
        .add::<PinSet>()
        .add::<ConversationMembersUpdate>()
        .add::<ConversationContinue>();
}

fn conversation(id: &str) -> Target {
    Target::thing("conversation", parse_id(id).unwrap_or_else(Uuid::nil))
}

/// One operation over a command that audits itself and is retried through
/// the dispatcher's request ids.
macro_rules! conversation_op {
    (
        $op:ident {
            name: $name:literal,
            kind: $kind:ident,
            module: $module:literal,
            permission: $perm:expr,
            destructive: $destructive:literal,
            title: $title:literal,
            description: $description:expr,
            emits: $emits:literal,
            input: $input:ty,
            output: $output:ty,
            run: $command:path
        }
    ) => {
        pub(crate) struct $op;

        impl Operation for $op {
            const NAME: &'static str = $name;
            const KIND: OpKind = OpKind::$kind;
            const MODULE: &'static str = $module;
            const PERMISSION: Option<Perm> = $perm;
            const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
            const DESTRUCTIVE: bool = $destructive;
            const TITLE: &'static str = $title;
            const DESCRIPTION: &'static str = $description;
            const EMITS: &'static [&'static str] = &[$emits];

            type Input = $input;
            type Output = $output;

            async fn run(call: &mut OpCall<'_>, input: $input) -> Result<$output, OpError> {
                let principal = principal_for(call.ctx, None);
                let answer = tool_answer($command(call.state, &principal, input).await)?;
                call.audited_by_command();
                Ok(answer)
            }
        }
    };
}

impl Targeted for ThreadStartArguments {
    fn target(&self) -> Target {
        conversation(&self.conversation_id)
    }
}

/// Starting a thread keeps the caller's request id itself: the thread and
/// its opening message are made in two steps, so a retry finds both again
/// by their keys rather than through the dispatcher's one transaction.
pub(crate) struct ThreadStart;

impl Operation for ThreadStart {
    const NAME: &'static str = "thread_start";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const DESTRUCTIVE: bool = false;
    const TITLE: &'static str = "Start thread";
    const DESCRIPTION: &'static str = "Move a topic into its own thread, usually from the message \
        where it started (fromMessageId), in a channel, DM or group. Optionally post the first \
        message. Starting from the same message again returns the existing thread.";
    const EMITS: &'static [&'static str] = &["thread.started"];

    type Input = ThreadStartArguments;
    type Output = ThreadValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: ThreadStartArguments,
    ) -> Result<ThreadValue, OpError> {
        let key = match call.ctx.request_id.as_deref() {
            Some(id) => Some(crate::idempotency::IdempotencyKey::parse(id).map_err(|_| {
                OpError::Invalid {
                    field: "clientRequestId".into(),
                    message: "Use letters, numbers, dashes or dots, up to 128 characters.".into(),
                }
            })?),
            None => None,
        };
        if key.is_some() {
            call.key_honoured();
        }
        let principal = principal_for(call.ctx, None);
        let (answer, replayed) = talk::thread_start(call.state, &principal, input, key).await;
        let answer = tool_answer(answer)?;
        call.mark_replayed(replayed);
        call.audited_by_command();
        Ok(answer)
    }
}

impl Targeted for ThreadUpdateArguments {
    fn target(&self) -> Target {
        conversation(&self.thread_id)
    }
}

conversation_op!(ThreadUpdate {
    name: "thread_update",
    kind: Change,
    module: "channels",
    permission: Some(Perm::ChannelsPost),
    destructive: false,
    title: "Update thread",
    description: "Rename a thread or set where it stands: a status (Open, In progress, Decided, \
        Done, Dropped, Blocked, or your own) and a one-line outcome. Do this when a discussion \
        lands so nobody has to reread it.",
    emits: "thread.updated",
    input: ThreadUpdateArguments,
    output: ThreadValue,
    run: talk::thread_update
});

impl Targeted for ChannelCreateArguments {
    fn target(&self) -> Target {
        Target::Space(parse_id(&self.project_id).unwrap_or_else(Uuid::nil))
    }
}

conversation_op!(ChannelCreate {
    name: "channel_create",
    kind: Create,
    module: "channels",
    permission: Some(Perm::ChannelsCreate),
    destructive: false,
    title: "Create channel",
    description: "Create a channel in a space, or a private one (private: true) that only its \
        members see, starting with you; add people with conversation_members_update. If an \
        active channel with that name that you can see already exists, it's returned instead \
        (created: false).",
    emits: "channel.created",
    input: ChannelCreateArguments,
    output: ConversationValue,
    run: channels::channel_create
});

impl Targeted for ConversationUpdateArguments {
    fn target(&self) -> Target {
        conversation(&self.conversation_id)
    }
}

conversation_op!(ConversationUpdate {
    name: "conversation_update",
    kind: Change,
    module: "messages",
    permission: Some(Perm::ChannelsPost),
    destructive: true,
    title: "Update conversation",
    description: "Change a channel, thread or group: rename it, set a thread's status and \
        outcome, or archive or restore a channel or thread (archived). Archiving keeps \
        everything and is refused while a run is going there. DMs can't be changed.",
    emits: "conversation.updated",
    input: ConversationUpdateArguments,
    output: ConversationValue,
    run: channels::conversation_update
});

impl Targeted for PinSetArguments {
    fn target(&self) -> Target {
        conversation(&self.conversation_id)
    }
}

conversation_op!(PinSet {
    name: "pin_set",
    kind: Change,
    module: "messages",
    permission: Some(Perm::ChannelsPost),
    destructive: false,
    title: "Pin or unpin",
    description: "Pin or unpin a message, page or decision in a conversation. Pins are the \
        conversation's brief: conversation_open shows them first.",
    emits: "conversation.pinned",
    input: PinSetArguments,
    output: PinValue,
    run: channels::pin_set
});

// ---------------------------------------------------------------------------
// conversation_members_update and conversation_continue keep their own
// request ids (a retry after someone else changed the group never repeats
// the change), so the dispatcher passes the caller's through.

pub(crate) struct ConversationMembersUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MembersInput {
    /// The group, or a thread in it: an ID or link.
    pub conversation_id: String,
    /// People and agents to add: actor IDs or @handles. They can read the
    /// group's whole history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<String>>,
    /// People and agents to remove: actor IDs or @handles. Your own to leave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,
    /// The group's version you last saw. Leave it out to change the current
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for MembersInput {
    fn target(&self) -> Target {
        conversation(&self.conversation_id)
    }
}

impl Operation for ConversationMembersUpdate {
    const NAME: &'static str = "conversation_members_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Change members";
    const DESCRIPTION: &'static str = "Add or remove people and agents in a group or a private \
        channel (pass it, or a thread in it). Whoever you add can read everything said there so \
        far. Remove your own @handle to leave. In a group, people can add and remove anyone and \
        agents can only leave; in a private channel, its managers and the space's moderators \
        change who's in it. DMs never change; other channels are open to everyone in the space.";
    const EMITS: &'static [&'static str] = &["conversation.members_changed"];

    type Input = MembersInput;
    type Output = GroupMembersValue;

    async fn run(call: &mut OpCall<'_>, input: MembersInput) -> Result<GroupMembersValue, OpError> {
        let principal = principal_for(call.ctx, None);
        let keyed = call.ctx.request_id.is_some();
        let value = tool_answer(
            groups::conversation_members(
                call.state,
                &principal,
                ConversationMembersArguments {
                    conversation_id: input.conversation_id,
                    add: input.add,
                    remove: input.remove,
                    expected_version: input.expected_version,
                    client_request_id: call.ctx.request_id.clone(),
                },
            )
            .await,
        )?;
        if keyed {
            call.key_honoured();
        }
        call.mark_replayed(value.idempotent_replay);
        call.audited_by_command();
        Ok(value)
    }
}

pub(crate) struct ConversationContinue;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContinueInput {
    /// The group, or a thread in one: an ID or link.
    pub conversation_id: String,
}

impl Targeted for ContinueInput {
    fn target(&self) -> Target {
        conversation(&self.conversation_id)
    }
}

impl Operation for ConversationContinue {
    const NAME: &'static str = "conversation_continue";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Let agents continue";
    const DESCRIPTION: &'static str = "When the agents in a group (or a thread in one) have \
        used their turns, let them take more, like the Continue button: they get 6 more \
        messages, and the other agents are asked to answer the latest agent message. Pass a \
        clientRequestId so a retry doesn't grant another round.";
    const EMITS: &'static [&'static str] = &["conversation.continued"];

    type Input = ContinueInput;
    type Output = ContinueValue;

    async fn run(call: &mut OpCall<'_>, input: ContinueInput) -> Result<ContinueValue, OpError> {
        let principal = principal_for(call.ctx, None);
        let keyed = call.ctx.request_id.is_some();
        let value = tool_answer(
            groups::conversation_continue(
                call.state,
                &principal,
                ConversationContinueArguments {
                    conversation_id: input.conversation_id,
                    client_request_id: call.ctx.request_id.clone(),
                },
            )
            .await,
        )?;
        if keyed {
            call.key_honoured();
        }
        call.mark_replayed(value.idempotent_replay);
        call.audited_by_command();
        Ok(value)
    }
}

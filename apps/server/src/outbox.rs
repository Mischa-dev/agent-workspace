//! Change notices: the typed events every write path stages in the event
//! log (K3) for open browsers, notifications and webhooks. Each payload
//! names what changed by id (AGENTS.md rule 10); `enqueue` stages it in
//! the change's own transaction with `events::emit`, so a rollback leaves
//! nothing and a commit is heard by every reader of the log.
//!
//! The name is historical: these used to wait in an `outbox_events` table
//! for a publisher. The log replaced both; the payloads stay the web's
//! protocol-1 frame (`OutboxEnvelope`) until the web reads catalogue events.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{domain::RunState, error::AppError};

pub const OUTBOX_SCHEMA_VERSION: i32 = 1;
/// A notice's payload is an event payload, so the log's bound applies.
pub const MAX_OUTBOX_PAYLOAD_BYTES: usize = crate::events::MAX_PAYLOAD_BYTES;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxConversationKind {
    Channel,
    Thread,
    Dm,
    Group,
    /// A thing's comments (C1.3).
    Comments,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxResourceKind {
    Project,
    Conversation,
    Message,
    ReadState,
    AgentRun,
    WorkItem,
    Notification,
    Account,
    Whiteboard,
    Page,
    Collection,
    Sheet,
}

impl OutboxResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Conversation => "conversation",
            Self::Message => "message",
            Self::ReadState => "read_state",
            Self::Account => "account",
            Self::AgentRun => "agent_run",
            Self::WorkItem => "work_item",
            Self::Notification => "notification",
            Self::Whiteboard => "whiteboard",
            Self::Page => "page",
            Self::Collection => "collection",
            Self::Sheet => "sheet",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreatedPayload {
    pub conversation_id: Uuid,
    pub kind: OutboxConversationKind,
    pub version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationUpdatedPayload {
    pub conversation_id: Uuid,
    pub version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplyRequestsUpdatedPayload {
    pub conversation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationArchivedPayload {
    pub conversation_id: Uuid,
    pub version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageCreatedPayload {
    pub conversation_id: Uuid,
    pub message_id: Uuid,
    pub sequence: i64,
    pub author_actor_id: Uuid,
}

/// A message was edited or removed. IDs and the revision only: never put
/// message text in a notice payload, because it is stored in the event log
/// and a removed message's text must vanish everywhere.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageUpdatedPayload {
    pub conversation_id: Uuid,
    pub message_id: Uuid,
    pub revision: i32,
    pub removed: bool,
}

/// Someone added or took away a reaction on a message. IDs only: browsers
/// refetch the reactions over HTTP, so no emoji, counts or names are stored
/// here or sent to anyone.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageReactionsUpdatedPayload {
    pub conversation_id: Uuid,
    pub message_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadStateUpdatedPayload {
    pub conversation_id: Uuid,
    pub actor_id: Uuid,
    pub highest_seen_sequence: i64,
    pub version: i64,
}

/// Someone was removed from a group (or left it). IDs and the group's version
/// only. Delivered only to the removed actor, and on purpose without a
/// conversation scope: they can no longer see the group, and this is how
/// their open browsers learn to drop what they hold of it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMemberRemovedPayload {
    pub conversation_id: Uuid,
    pub actor_id: Uuid,
    pub version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRunUpdatedPayload {
    pub conversation_id: Uuid,
    pub agent_run_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<RunState>,
}

/// A1: a run changed, whoever runs it (the stopgap until K3's log carries
/// `run.*` events). IDs and the state only; `change` names what happened
/// (`opened`, `updated`, `activity`, `finished`, `liveness_changed`,
/// `result_added`, `usage_updated`, `cancel`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunChangedPayload {
    pub run_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
    pub change: String,
    pub state: String,
    pub version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkItemUpdatedPayload {
    pub work_item_id: Uuid,
    pub version: i64,
    /// Where the task sits on the board after the change, so open boards
    /// can move the card in place instead of refetching. IDs only, like the
    /// rest of the outbox: no titles or other text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<WorkItemPlacement>,
}

/// A task's column and neighbours on the board: the cards just before and
/// after it among tasks with the same status and archived state, in board
/// order. Either neighbour is absent at the ends of the column.
#[derive(Clone, Debug, Deserialize, Eq, FromRow, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkItemPlacement {
    pub status_id: Uuid,
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectMembershipUpdatedPayload {
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub version: i64,
}

/// Who manages an agent or app account changed (K1.4): delivered to the
/// manager named or un-named (`actorId`), whose screens refetch what they
/// may manage. Ids only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountManagersChangedPayload {
    /// The manager whose rights changed.
    pub actor_id: Uuid,
    /// The agent or app account.
    pub account_id: Uuid,
    pub workspace_id: Uuid,
    /// Whether they are a manager now.
    pub managing: bool,
}

/// Someone's status changed (C2.15): delivered to each space they're in
/// where it shows, whose screens refetch it. Ids only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountStatusChangedPayload {
    pub account_id: Uuid,
    /// The workspace of a status shown only there; None for one shown
    /// everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<Uuid>,
}

/// Someone's notifications changed (one was added or came back unread). The
/// recipient's ID only: delivered to them alone, and browsers refetch the
/// inbox over HTTP.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationsUpdatedPayload {
    pub actor_id: Uuid,
}

/// A whiteboard's scene or details changed. IDs and counters only: open
/// boards fetch the elements changed since their `sceneSeq` over HTTP. The
/// envelope's project is the board's project, or the workspace's Messages
/// space for a board without one; realtime delivers it to whoever
/// `whiteboard_visible_to` allows, not to that project's members.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardUpdatedPayload {
    pub whiteboard_id: Uuid,
    pub scene_seq: i64,
    pub version: i64,
}

/// A page's content or details changed. The page's ID and version only:
/// open editors fetch the changes they lack over HTTP, so no page text is
/// stored in the outbox or sent to anyone.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageUpdatedPayload {
    pub page_id: Uuid,
    pub version: i64,
}

/// A page's comments or suggestions changed (AWD-53). The page's ID only:
/// open pages fetch the discussion over HTTP.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageDiscussionUpdatedPayload {
    pub page_id: Uuid,
}

/// A collection, its fields or views, or some of its records changed
/// (AWD-90). IDs only: open screens fetch what they show over HTTP, so no
/// record's title or values are stored in the outbox or sent to anyone.
/// `record_ids` names the records a write changed (at most
/// `MAX_COLLECTION_EVENT_RECORDS`); none means the collection itself
/// changed, or more records than that did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionUpdatedPayload {
    pub collection_id: Uuid,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub record_ids: Vec<Uuid>,
}

/// The most records one `collection.updated` names; a larger change names
/// none, and open screens refetch what they show.
pub const MAX_COLLECTION_EVENT_RECORDS: usize = 100;

/// A sheet's cells, tabs or name changed (AWD-54). IDs and the revision
/// only: open screens fetch the sheet again over HTTP when theirs is older.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetUpdatedPayload {
    pub sheet_id: Uuid,
    pub revision: i64,
}

/// Every outbox event type; each has a catalogue entry naming it as a
/// legacy name (`events::catalog`).
pub const LEGACY_EVENT_TYPES: &[&str] = &[
    "conversation.created",
    "conversation.updated",
    "conversation.reply_requests.updated",
    "conversation.archived",
    "conversation.message.created",
    "conversation.message.updated",
    "conversation.message.reactions.updated",
    "conversation.read_state.updated",
    "conversation.member.removed",
    "conversation.agent_run.updated",
    "work_item.updated",
    "project.membership.updated",
    "project.modules.updated",
    "notifications.updated",
    "account.managers.changed",
    "whiteboard.updated",
    "page.updated",
    "page.discussion.updated",
    "collection.updated",
    "sheet.updated",
];

/// The most modules one `project.modules.updated` names.
pub const MAX_MODULE_EVENT_SWITCHES: usize = 200;

/// A space's own module switches changed (M1.2): which modules, on or off.
/// Module ids only; open screens and other processes read the switches
/// again.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectModulesUpdatedPayload {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    /// Module id to on.
    pub changed: std::collections::BTreeMap<String, bool>,
}

/// A workspace's own emoji changed (C2.19): one added, retired or brought
/// back. Sent to each space of the workspace, whose reaction pickers read
/// the list again. Ids only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomEmojiChangedPayload {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub emoji_id: Uuid,
}

/// Someone's draft changed (C2.28): saved or cleared on one of their
/// devices. Their id only, delivered to them alone; their open draft lists
/// read again over HTTP.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDraftChangedPayload {
    pub actor_id: Uuid,
}

/// Someone's keyword alerts changed (C2.13). Their id only, delivered to
/// them alone; their open settings read the list again over HTTP.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationKeywordsChangedPayload {
    pub actor_id: Uuid,
}

/// A workspace's lifecycle changed (H1.24): suspended, resumed, deletion
/// scheduled or kept. Ids and the state only; open screens refetch the
/// session. Delivered in the workspace's own space.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceLifecycleChangedPayload {
    pub workspace_id: Uuid,
    /// The workspace's own space, which carries the envelope.
    pub project_id: Uuid,
    /// `active`, `importing`, `suspended` or `deleting`.
    pub lifecycle: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboxEventPayload {
    ConversationCreated(ConversationCreatedPayload),
    ConversationUpdated(ConversationUpdatedPayload),
    ReplyRequestsUpdated(ReplyRequestsUpdatedPayload),
    ConversationArchived(ConversationArchivedPayload),
    MessageCreated(MessageCreatedPayload),
    MessageUpdated(MessageUpdatedPayload),
    MessageReactionsUpdated(MessageReactionsUpdatedPayload),
    ReadStateUpdated(ReadStateUpdatedPayload),
    ConversationMemberRemoved(ConversationMemberRemovedPayload),
    AgentRunUpdated(AgentRunUpdatedPayload),
    RunChanged(RunChangedPayload),
    WorkItemUpdated(WorkItemUpdatedPayload),
    ProjectMembershipUpdated(ProjectMembershipUpdatedPayload),
    NotificationsUpdated(NotificationsUpdatedPayload),
    AccountManagersChanged(AccountManagersChangedPayload),
    AccountStatusChanged(AccountStatusChangedPayload),
    WhiteboardUpdated(WhiteboardUpdatedPayload),
    PageUpdated(PageUpdatedPayload),
    PageDiscussionUpdated(PageDiscussionUpdatedPayload),
    CollectionUpdated(CollectionUpdatedPayload),
    SheetUpdated(SheetUpdatedPayload),
    ProjectModulesUpdated(ProjectModulesUpdatedPayload),
    CustomEmojiChanged(CustomEmojiChangedPayload),
    MessageDraftChanged(MessageDraftChangedPayload),
    NotificationKeywordsChanged(NotificationKeywordsChangedPayload),
    WorkspaceLifecycleChanged(WorkspaceLifecycleChangedPayload),
}

impl OutboxEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::ConversationCreated(_) => "conversation.created",
            Self::ConversationUpdated(_) => "conversation.updated",
            Self::ReplyRequestsUpdated(_) => "conversation.reply_requests.updated",
            Self::ConversationArchived(_) => "conversation.archived",
            Self::MessageCreated(_) => "conversation.message.created",
            Self::MessageUpdated(_) => "conversation.message.updated",
            Self::MessageReactionsUpdated(_) => "conversation.message.reactions.updated",
            Self::ReadStateUpdated(_) => "conversation.read_state.updated",
            Self::ConversationMemberRemoved(_) => "conversation.member.removed",
            Self::AgentRunUpdated(_) => "conversation.agent_run.updated",
            Self::RunChanged(_) => "run.changed",
            Self::WorkItemUpdated(_) => "work_item.updated",
            Self::ProjectMembershipUpdated(_) => "project.membership.updated",
            Self::NotificationsUpdated(_) => "notifications.updated",
            Self::AccountManagersChanged(_) => "account.managers.changed",
            Self::AccountStatusChanged(_) => "account.status.changed",
            Self::WhiteboardUpdated(_) => "whiteboard.updated",
            Self::PageUpdated(_) => "page.updated",
            Self::PageDiscussionUpdated(_) => "page.discussion.updated",
            Self::CollectionUpdated(_) => "collection.updated",
            Self::SheetUpdated(_) => "sheet.updated",
            Self::ProjectModulesUpdated(_) => "project.modules.updated",
            Self::CustomEmojiChanged(_) => "custom_emoji.changed",
            Self::MessageDraftChanged(_) => "message_draft.changed",
            Self::NotificationKeywordsChanged(_) => "notification_keywords.changed",
            Self::WorkspaceLifecycleChanged(_) => "workspace.lifecycle.changed",
        }
    }

    pub fn conversation_id(&self) -> Option<Uuid> {
        match self {
            Self::ConversationCreated(payload) => Some(payload.conversation_id),
            Self::ConversationUpdated(payload) => Some(payload.conversation_id),
            Self::ReplyRequestsUpdated(payload) => Some(payload.conversation_id),
            Self::ConversationArchived(payload) => Some(payload.conversation_id),
            Self::MessageCreated(payload) => Some(payload.conversation_id),
            Self::MessageUpdated(payload) => Some(payload.conversation_id),
            Self::MessageReactionsUpdated(payload) => Some(payload.conversation_id),
            Self::ReadStateUpdated(payload) => Some(payload.conversation_id),
            Self::AgentRunUpdated(payload) => Some(payload.conversation_id),
            Self::RunChanged(payload) => payload.conversation_id,
            // The removed actor can no longer see the group, so delivery must
            // not depend on that visibility: the event has no conversation
            // scope and goes only to them (see recipient_actor_id).
            Self::ConversationMemberRemoved(_)
            | Self::WorkItemUpdated(_)
            | Self::ProjectMembershipUpdated(_)
            | Self::NotificationsUpdated(_)
            | Self::AccountManagersChanged(_)
            | Self::AccountStatusChanged(_)
            | Self::WhiteboardUpdated(_)
            | Self::PageUpdated(_)
            | Self::PageDiscussionUpdated(_)
            | Self::CollectionUpdated(_)
            | Self::SheetUpdated(_)
            | Self::ProjectModulesUpdated(_)
            | Self::CustomEmojiChanged(_)
            | Self::MessageDraftChanged(_)
            | Self::NotificationKeywordsChanged(_)
            | Self::WorkspaceLifecycleChanged(_) => None,
        }
    }

    fn recipient_actor_id(&self) -> Option<Uuid> {
        match self {
            // Read cursors are private per-Actor state. The durable event is
            // still Project/Conversation scoped for validation and refetch,
            // but realtime delivery must target only the Actor that owns it.
            Self::ReadStateUpdated(payload) => Some(payload.actor_id),
            Self::ConversationMemberRemoved(payload) => Some(payload.actor_id),
            Self::NotificationsUpdated(payload) => Some(payload.actor_id),
            Self::AccountManagersChanged(payload) => Some(payload.actor_id),
            Self::MessageDraftChanged(payload) => Some(payload.actor_id),
            Self::NotificationKeywordsChanged(payload) => Some(payload.actor_id),
            _ => None,
        }
    }

    pub fn resource_kind(&self) -> OutboxResourceKind {
        match self {
            Self::ConversationCreated(_)
            | Self::ConversationUpdated(_)
            | Self::ReplyRequestsUpdated(_)
            | Self::ConversationArchived(_)
            | Self::ConversationMemberRemoved(_) => OutboxResourceKind::Conversation,
            Self::MessageCreated(_)
            | Self::MessageUpdated(_)
            | Self::MessageReactionsUpdated(_) => OutboxResourceKind::Message,
            Self::ReadStateUpdated(_) => OutboxResourceKind::ReadState,
            Self::AgentRunUpdated(_) | Self::RunChanged(_) => OutboxResourceKind::AgentRun,
            Self::WorkItemUpdated(_) => OutboxResourceKind::WorkItem,
            Self::ProjectMembershipUpdated(_) => OutboxResourceKind::Project,
            Self::NotificationsUpdated(_) => OutboxResourceKind::Notification,
            Self::MessageDraftChanged(_) | Self::NotificationKeywordsChanged(_) => {
                OutboxResourceKind::Account
            }
            Self::AccountManagersChanged(_) | Self::AccountStatusChanged(_) => {
                OutboxResourceKind::Account
            }
            Self::WhiteboardUpdated(_) => OutboxResourceKind::Whiteboard,
            Self::PageUpdated(_) => OutboxResourceKind::Page,
            Self::PageDiscussionUpdated(_) => OutboxResourceKind::Page,
            Self::CollectionUpdated(_) => OutboxResourceKind::Collection,
            Self::SheetUpdated(_) => OutboxResourceKind::Sheet,
            Self::ProjectModulesUpdated(_) | Self::CustomEmojiChanged(_) => {
                OutboxResourceKind::Project
            }
            Self::WorkspaceLifecycleChanged(_) => OutboxResourceKind::Project,
        }
    }

    pub fn resource_id(&self) -> Uuid {
        match self {
            Self::ConversationCreated(payload) => payload.conversation_id,
            Self::ConversationUpdated(payload) => payload.conversation_id,
            Self::ReplyRequestsUpdated(payload) => payload.conversation_id,
            Self::ConversationArchived(payload) => payload.conversation_id,
            Self::MessageCreated(payload) => payload.message_id,
            Self::MessageUpdated(payload) => payload.message_id,
            Self::MessageReactionsUpdated(payload) => payload.message_id,
            // Read state is keyed by Conversation and Actor. The envelope
            // already carries the Conversation, so the Actor is its resource ID.
            Self::ReadStateUpdated(payload) => payload.actor_id,
            Self::ConversationMemberRemoved(payload) => payload.conversation_id,
            Self::AgentRunUpdated(payload) => payload.agent_run_id,
            Self::RunChanged(payload) => payload.run_id,
            Self::WorkItemUpdated(payload) => payload.work_item_id,
            Self::ProjectMembershipUpdated(payload) => payload.project_id,
            // Keyed by recipient, like read state.
            Self::NotificationsUpdated(payload) => payload.actor_id,
            Self::AccountManagersChanged(payload) => payload.account_id,
            Self::AccountStatusChanged(payload) => payload.account_id,
            Self::WhiteboardUpdated(payload) => payload.whiteboard_id,
            Self::PageUpdated(payload) => payload.page_id,
            Self::PageDiscussionUpdated(payload) => payload.page_id,
            Self::CollectionUpdated(payload) => payload.collection_id,
            Self::SheetUpdated(payload) => payload.sheet_id,
            Self::ProjectModulesUpdated(payload) => payload.project_id,
            Self::CustomEmojiChanged(payload) => payload.project_id,
            // Keyed by its writer, like notifications.
            Self::MessageDraftChanged(payload) => payload.actor_id,
            Self::NotificationKeywordsChanged(payload) => payload.actor_id,
            Self::WorkspaceLifecycleChanged(payload) => payload.project_id,
        }
    }

    fn validate(&self) -> Result<(), OutboxEventError> {
        if self.conversation_id().is_some_and(|id| id.is_nil()) || self.resource_id().is_nil() {
            return Err(OutboxEventError::MalformedPayload);
        }
        match self {
            Self::ConversationCreated(payload) if payload.version < 1 => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::ConversationUpdated(payload) if payload.version < 1 => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::ConversationArchived(payload) if payload.version < 1 => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::MessageCreated(payload)
                if payload.sequence < 1 || payload.author_actor_id.is_nil() =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::MessageUpdated(payload) if payload.revision < 1 => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::ReadStateUpdated(payload)
                if payload.highest_seen_sequence < 0 || payload.version < 1 =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::ConversationMemberRemoved(payload)
                if payload.actor_id.is_nil() || payload.version < 1 =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::AgentRunUpdated(payload) if payload.agent_run_id.is_nil() => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::RunChanged(payload)
                if payload.version < 1
                    || !(1..=40).contains(&payload.change.len())
                    || !(1..=40).contains(&payload.state.len()) =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::WorkItemUpdated(payload)
                if payload.work_item_id.is_nil() || payload.version < 1 =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::ProjectMembershipUpdated(payload)
                if payload.actor_id.is_nil() || payload.version < 1 =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::WhiteboardUpdated(payload) if payload.scene_seq < 0 || payload.version < 1 => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::PageUpdated(payload) if payload.version < 1 => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::SheetUpdated(payload) if payload.revision < 1 || payload.sheet_id.is_nil() => {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::WorkspaceLifecycleChanged(payload)
                if payload.workspace_id.is_nil()
                    || !matches!(
                        payload.lifecycle.as_str(),
                        "active" | "importing" | "suspended" | "deleting"
                    ) =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::ProjectModulesUpdated(payload)
                if payload.workspace_id.is_nil()
                    || payload.project_id.is_nil()
                    || !(1..=MAX_MODULE_EVENT_SWITCHES).contains(&payload.changed.len())
                    || payload.changed.keys().any(|module| {
                        !(1..=81).contains(&module.len())
                            || !module.bytes().all(|byte| {
                                byte.is_ascii_lowercase()
                                    || byte.is_ascii_digit()
                                    || byte == b'_'
                                    || byte == b'.'
                            })
                    }) =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            Self::CollectionUpdated(payload)
                if payload.record_ids.len() > MAX_COLLECTION_EVENT_RECORDS
                    || payload.record_ids.iter().any(Uuid::is_nil) =>
            {
                Err(OutboxEventError::MalformedPayload)
            }
            _ => Ok(()),
        }
    }

    fn to_json(&self) -> Result<serde_json::Value, OutboxEventError> {
        self.validate()?;
        let value = match self {
            Self::ConversationCreated(payload) => serde_json::to_value(payload),
            Self::ConversationUpdated(payload) => serde_json::to_value(payload),
            Self::ReplyRequestsUpdated(payload) => serde_json::to_value(payload),
            Self::ConversationArchived(payload) => serde_json::to_value(payload),
            Self::MessageCreated(payload) => serde_json::to_value(payload),
            Self::MessageUpdated(payload) => serde_json::to_value(payload),
            Self::MessageReactionsUpdated(payload) => serde_json::to_value(payload),
            Self::ReadStateUpdated(payload) => serde_json::to_value(payload),
            Self::ConversationMemberRemoved(payload) => serde_json::to_value(payload),
            Self::AgentRunUpdated(payload) => serde_json::to_value(payload),
            Self::RunChanged(payload) => serde_json::to_value(payload),
            Self::WorkItemUpdated(payload) => serde_json::to_value(payload),
            Self::ProjectMembershipUpdated(payload) => serde_json::to_value(payload),
            Self::NotificationsUpdated(payload) => serde_json::to_value(payload),
            Self::AccountManagersChanged(payload) => serde_json::to_value(payload),
            Self::AccountStatusChanged(payload) => serde_json::to_value(payload),
            Self::WhiteboardUpdated(payload) => serde_json::to_value(payload),
            Self::PageUpdated(payload) => serde_json::to_value(payload),
            Self::PageDiscussionUpdated(payload) => serde_json::to_value(payload),
            Self::CollectionUpdated(payload) => serde_json::to_value(payload),
            Self::SheetUpdated(payload) => serde_json::to_value(payload),
            Self::ProjectModulesUpdated(payload) => serde_json::to_value(payload),
            Self::CustomEmojiChanged(payload) => serde_json::to_value(payload),
            Self::MessageDraftChanged(payload) => serde_json::to_value(payload),
            Self::NotificationKeywordsChanged(payload) => serde_json::to_value(payload),
            Self::WorkspaceLifecycleChanged(payload) => serde_json::to_value(payload),
        }
        .map_err(|_| OutboxEventError::MalformedPayload)?;
        validate_payload_size(&value)?;
        Ok(value)
    }

    pub(crate) fn from_json(
        event_type: &str,
        value: serde_json::Value,
    ) -> Result<Self, OutboxEventError> {
        validate_payload_size(&value)?;
        let payload = match event_type {
            "conversation.created" => Self::ConversationCreated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.updated" => Self::ConversationUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.reply_requests.updated" => Self::ReplyRequestsUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.archived" => Self::ConversationArchived(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.message.created" => Self::MessageCreated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.message.updated" => Self::MessageUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.message.reactions.updated" => Self::MessageReactionsUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.read_state.updated" => Self::ReadStateUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.member.removed" => Self::ConversationMemberRemoved(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "conversation.agent_run.updated" => Self::AgentRunUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "run.changed" => Self::RunChanged(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "work_item.updated" => Self::WorkItemUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "project.membership.updated" => Self::ProjectMembershipUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "notifications.updated" => Self::NotificationsUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "account.managers.changed" => Self::AccountManagersChanged(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "account.status.changed" => Self::AccountStatusChanged(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "whiteboard.updated" => Self::WhiteboardUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "page.updated" => Self::PageUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "page.discussion.updated" => Self::PageDiscussionUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "collection.updated" => Self::CollectionUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "sheet.updated" => Self::SheetUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "custom_emoji.changed" => Self::CustomEmojiChanged(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "message_draft.changed" => Self::MessageDraftChanged(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "notification_keywords.changed" => Self::NotificationKeywordsChanged(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "project.modules.updated" => Self::ProjectModulesUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "workspace.lifecycle.changed" => Self::WorkspaceLifecycleChanged(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            _ => return Err(OutboxEventError::UnknownEventType),
        };
        payload.validate()?;
        Ok(payload)
    }
}

#[derive(Clone)]
pub struct NewOutboxEvent {
    event_id: Uuid,
    project_id: Uuid,
    payload: OutboxEventPayload,
    payload_json: serde_json::Value,
}

impl NewOutboxEvent {
    pub fn new(project_id: Uuid, payload: OutboxEventPayload) -> Result<Self, OutboxEventError> {
        if project_id.is_nil() {
            return Err(OutboxEventError::MalformedMetadata);
        }
        let payload_json = payload.to_json()?;
        Ok(Self {
            event_id: Uuid::now_v7(),
            project_id,
            payload,
            payload_json,
        })
    }

    pub fn event_id(&self) -> Uuid {
        self.event_id
    }

    pub fn project_id(&self) -> Uuid {
        self.project_id
    }

    pub fn payload(&self) -> &OutboxEventPayload {
        &self.payload
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OutboxEnvelope {
    pub event_id: Uuid,
    pub event_cursor: i64,
    pub version: i32,
    pub project_id: Uuid,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<Uuid>,
    #[serde(skip)]
    pub(crate) recipient_actor_id: Option<Uuid>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub occurred_at: String,
    pub payload: serde_json::Value,
}

impl OutboxEnvelope {
    /// Protocol 1's frame for a logged event (K5.5): the outbox event the log
    /// mirrored, with the same id, rebuilt from the log on every instance's
    /// tail. The cursor is the event's position. None for an event that
    /// never was an outbox event (it has no `legacy_type`) or doesn't decode.
    pub fn from_logged(event: &crate::events::LoggedEvent) -> Option<Self> {
        let event_type = event.legacy_type.as_deref()?;
        let project_id = event.space_id?;
        let decoded = OutboxEventPayload::from_json(event_type, event.payload.clone()).ok()?;
        Some(Self {
            event_id: event.id,
            event_cursor: event.position,
            version: OUTBOX_SCHEMA_VERSION,
            project_id,
            conversation_id: decoded.conversation_id(),
            recipient_actor_id: decoded.recipient_actor_id(),
            event_type: event_type.to_owned(),
            occurred_at: event
                .created_at
                .format("%Y-%m-%dT%H:%M:%S%.6fZ")
                .to_string(),
            payload: decoded.to_json().ok()?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OutboxEventError {
    #[error("outbox event metadata is malformed")]
    MalformedMetadata,
    #[error("outbox event payload is malformed")]
    MalformedPayload,
    #[error("outbox event payload exceeds its bound")]
    PayloadTooLarge,
    #[error("outbox event type is unknown")]
    UnknownEventType,
    #[error("outbox event metadata does not match its payload")]
    MetadataMismatch,
}

pub trait OutboxPublisher {
    type Error;

    fn publish(&self, envelope: &OutboxEnvelope) -> Result<(), Self::Error>;
}

/// Stages the notice in the log, in the change's transaction.
pub async fn enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    event: &NewOutboxEvent,
) -> Result<Uuid, AppError> {
    enqueue_on(transaction, event).await
}

/// [`enqueue`] on a connection the caller holds inside its transaction.
pub async fn enqueue_on(
    connection: &mut sqlx::PgConnection,
    event: &NewOutboxEvent,
) -> Result<Uuid, AppError> {
    crate::events::notice::stage(
        connection,
        event.event_id,
        event.project_id,
        &event.payload,
        &event.payload_json,
    )
    .await
}

pub async fn enqueue_typed(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    payload: OutboxEventPayload,
) -> Result<Uuid, AppError> {
    let event = NewOutboxEvent::new(project_id, payload)
        .map_err(|_| AppError::Internal("construct typed outbox event"))?;
    enqueue(transaction, &event).await
}

/// Enqueue `work_item.updated` with the task's board placement as of this
/// transaction. Every task change goes through here.
pub async fn enqueue_work_item_updated(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    version: i64,
) -> Result<Uuid, AppError> {
    let placement = sqlx::query_as::<_, WorkItemPlacement>(
        r#"
        SELECT
            item.status_id,
            item.archived_at IS NOT NULL AS archived,
            (
                SELECT previous.id FROM task_rows previous
                WHERE previous.project_id = item.project_id
                  AND previous.status_id = item.status_id
                  AND (previous.archived_at IS NULL) = (item.archived_at IS NULL)
                  AND (previous.board_rank, previous.id) < (item.board_rank, item.id)
                ORDER BY previous.board_rank DESC, previous.id DESC
                LIMIT 1
            ) AS after_id,
            (
                SELECT following.id FROM task_rows following
                WHERE following.project_id = item.project_id
                  AND following.status_id = item.status_id
                  AND (following.archived_at IS NULL) = (item.archived_at IS NULL)
                  AND (following.board_rank, following.id) > (item.board_rank, item.id)
                ORDER BY following.board_rank, following.id
                LIMIT 1
            ) AS before_id
        FROM task_rows item
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?;
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
            work_item_id,
            version,
            placement,
        }),
    )
    .await
}

/// Enqueue `collection.updated` for a collection, naming the records that
/// changed (none for the collection itself; too many name none).
pub async fn enqueue_collection_updated(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    collection_id: Uuid,
    record_ids: &[Uuid],
) -> Result<Uuid, AppError> {
    let mut record_ids = record_ids.to_vec();
    record_ids.sort_unstable();
    record_ids.dedup();
    if record_ids.len() > MAX_COLLECTION_EVENT_RECORDS {
        record_ids.clear();
    }
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::CollectionUpdated(CollectionUpdatedPayload {
            collection_id,
            record_ids,
        }),
    )
    .await
}

/// Enqueue `project.modules.updated`: a space's own module switches
/// changed (module id to on).
pub async fn enqueue_project_modules_updated(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    changed: std::collections::BTreeMap<String, bool>,
) -> Result<Uuid, AppError> {
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::ProjectModulesUpdated(ProjectModulesUpdatedPayload {
            workspace_id,
            project_id,
            changed,
        }),
    )
    .await
}

/// Enqueue `sheet.updated` for a sheet at its new revision.
pub async fn enqueue_sheet_updated(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    sheet_id: Uuid,
    revision: i64,
) -> Result<Uuid, AppError> {
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::SheetUpdated(SheetUpdatedPayload { sheet_id, revision }),
    )
    .await
}

pub async fn enqueue_message_created(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    message_id: Uuid,
    sequence: i64,
    author_actor_id: Uuid,
) -> Result<Uuid, AppError> {
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::MessageCreated(MessageCreatedPayload {
            conversation_id,
            message_id,
            sequence,
            author_actor_id,
        }),
    )
    .await
}

pub async fn enqueue_agent_run_updated(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    agent_run_id: Uuid,
) -> Result<Uuid, AppError> {
    let state =
        crate::runs::reads::builtin_state_in(transaction, agent_run_id, conversation_id).await?;
    let state = state.parse::<RunState>()?;
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::AgentRunUpdated(AgentRunUpdatedPayload {
            conversation_id,
            agent_run_id,
            state: Some(state),
        }),
    )
    .await
}

fn validate_payload_size(value: &serde_json::Value) -> Result<(), OutboxEventError> {
    if !value.is_object() {
        return Err(OutboxEventError::MalformedPayload);
    }
    let size = serde_json::to_vec(value)
        .map_err(|_| OutboxEventError::MalformedPayload)?
        .len();
    if size > MAX_OUTBOX_PAYLOAD_BYTES {
        return Err(OutboxEventError::PayloadTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::domain::RunState;

    use super::{
        AgentRunUpdatedPayload, ConversationCreatedPayload, ConversationMemberRemovedPayload,
        MessageCreatedPayload, MessageReactionsUpdatedPayload, MessageUpdatedPayload,
        OUTBOX_SCHEMA_VERSION, OutboxConversationKind, OutboxEnvelope, OutboxEventError,
        OutboxEventPayload, OutboxResourceKind, ProjectMembershipUpdatedPayload,
        ReadStateUpdatedPayload, WorkItemPlacement, WorkItemUpdatedPayload,
    };
    use crate::events::LoggedEvent;
    use uuid::Uuid;

    fn id(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    /// A logged notice, as the tail reads it.
    fn logged(event_type: &str, payload: serde_json::Value) -> LoggedEvent {
        LoggedEvent {
            position: 42,
            id: Uuid::now_v7(),
            workspace_id: id(9),
            space_id: Some(id(1)),
            thing_id: None,
            thing_kind: None,
            access_thing_id: None,
            thing_version: None,
            event_type: "message.created".to_owned(),
            class: "fact".to_owned(),
            changes: Vec::new(),
            audience: "conversation".to_owned(),
            conversation_id: Some(id(2)),
            conversation_restricted: Some(false),
            audience_account_id: None,
            actor_account_id: None,
            via_credential_id: None,
            requested_by_account_id: None,
            run_id: None,
            request_id: None,
            legacy_type: Some(event_type.to_owned()),
            payload,
            created_at: chrono::Utc::now(),
        }
    }

    fn message_notice() -> LoggedEvent {
        logged(
            "conversation.message.created",
            serde_json::json!({
                "conversationId": id(2),
                "messageId": id(3),
                "sequence": 7,
                "authorActorId": id(4),
            }),
        )
    }

    #[test]
    fn typed_message_event_becomes_the_versioned_realtime_envelope() {
        let notice = message_notice();
        assert_eq!(
            OutboxEventPayload::from_json(
                notice.legacy_type.as_deref().unwrap_or_default(),
                notice.payload.clone()
            )
            .expect("typed payload"),
            OutboxEventPayload::MessageCreated(MessageCreatedPayload {
                conversation_id: id(2),
                message_id: id(3),
                sequence: 7,
                author_actor_id: id(4),
            })
        );

        let envelope = OutboxEnvelope::from_logged(&notice).expect("realtime envelope");
        let json = serde_json::to_value(envelope).expect("envelope JSON");
        assert_eq!(json["event_id"], notice.id.to_string());
        assert_eq!(json["event_cursor"], 42);
        assert_eq!(json["version"], OUTBOX_SCHEMA_VERSION);
        assert_eq!(json["conversationId"], id(2).to_string());
        assert_eq!(json["type"], "conversation.message.created");
        assert_eq!(json["payload"]["messageId"], id(3).to_string());
        assert!(json.get("recipient_actor_id").is_none());
    }

    #[test]
    fn validated_read_state_notice_targets_only_its_actor() {
        let notice = logged(
            "conversation.read_state.updated",
            serde_json::json!({
                "conversationId": id(2),
                "actorId": id(4),
                "highestSeenSequence": 9,
                "version": 2,
            }),
        );

        let envelope = OutboxEnvelope::from_logged(&notice).expect("validated read-state envelope");
        assert_eq!(envelope.conversation_id, Some(id(2)));
        assert_eq!(envelope.recipient_actor_id, Some(id(4)));
        let wire = serde_json::to_value(envelope).expect("read-state wire envelope");
        assert_eq!(wire["conversationId"], id(2).to_string());
        assert!(wire.get("recipient_actor_id").is_none());
    }

    #[test]
    fn payloads_cover_conversation_and_read_state_invalidation() {
        let created = OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id: id(2),
            kind: OutboxConversationKind::Channel,
            version: 1,
        });
        assert_eq!(created.event_type(), "conversation.created");
        assert_eq!(created.resource_kind(), OutboxResourceKind::Conversation);
        assert!(created.to_json().is_ok());

        let read = OutboxEventPayload::ReadStateUpdated(ReadStateUpdatedPayload {
            conversation_id: id(2),
            actor_id: id(4),
            highest_seen_sequence: 9,
            version: 2,
        });
        assert_eq!(read.event_type(), "conversation.read_state.updated");
        assert_eq!(read.resource_kind(), OutboxResourceKind::ReadState);
        assert_eq!(read.resource_id(), id(4));
        assert!(read.to_json().is_ok());

        let run = OutboxEventPayload::AgentRunUpdated(AgentRunUpdatedPayload {
            conversation_id: id(2),
            agent_run_id: id(5),
            state: Some(RunState::Working),
        });
        assert_eq!(run.event_type(), "conversation.agent_run.updated");
        assert_eq!(run.resource_kind(), OutboxResourceKind::AgentRun);
        assert_eq!(run.resource_id(), id(5));
        assert!(run.to_json().is_ok());

        let work_item = OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
            work_item_id: id(6),
            version: 3,
            placement: None,
        });
        assert_eq!(work_item.event_type(), "work_item.updated");
        assert_eq!(work_item.resource_kind(), OutboxResourceKind::WorkItem);
        assert_eq!(work_item.resource_id(), id(6));
        assert_eq!(work_item.conversation_id(), None);
        assert!(work_item.to_json().is_ok());

        let membership =
            OutboxEventPayload::ProjectMembershipUpdated(ProjectMembershipUpdatedPayload {
                project_id: id(1),
                actor_id: id(7),
                version: 4,
            });
        assert_eq!(membership.event_type(), "project.membership.updated");
        assert_eq!(membership.resource_kind(), OutboxResourceKind::Project);
        assert_eq!(membership.resource_id(), id(1));
        assert_eq!(membership.conversation_id(), None);
        assert!(membership.to_json().is_ok());

        let legacy_run = OutboxEventPayload::from_json(
            "conversation.agent_run.updated",
            serde_json::json!({
                "conversationId": id(2),
                "agentRunId": id(5)
            }),
        )
        .expect("schema-version-one Agent Run payload without state remains readable");
        let OutboxEventPayload::AgentRunUpdated(legacy_run) = legacy_run else {
            panic!("expected Agent Run payload")
        };
        assert_eq!(legacy_run.state, None);
    }

    #[test]
    fn malformed_notices_have_no_frame() {
        let mut unknown = message_notice();
        unknown.legacy_type = Some("conversation.future.changed".to_owned());
        assert_eq!(
            OutboxEventPayload::from_json("conversation.future.changed", unknown.payload.clone()),
            Err(OutboxEventError::UnknownEventType)
        );
        assert!(OutboxEnvelope::from_logged(&unknown).is_none());

        let mut extra_field = message_notice();
        extra_field.payload["secret"] = serde_json::json!("must not pass through");
        assert_eq!(
            OutboxEventPayload::from_json(
                "conversation.message.created",
                extra_field.payload.clone()
            ),
            Err(OutboxEventError::MalformedPayload)
        );
        assert!(OutboxEnvelope::from_logged(&extra_field).is_none());

        // A catalogue event that never was a notice has no protocol-1 frame.
        let mut catalogue_only = message_notice();
        catalogue_only.legacy_type = None;
        assert!(OutboxEnvelope::from_logged(&catalogue_only).is_none());
    }

    #[test]
    fn invalid_payload_values_are_rejected_before_enqueue() {
        let invalid_message = OutboxEventPayload::MessageCreated(MessageCreatedPayload {
            conversation_id: id(2),
            message_id: id(3),
            sequence: 0,
            author_actor_id: id(4),
        });
        assert_eq!(
            invalid_message.to_json(),
            Err(OutboxEventError::MalformedPayload)
        );

        let invalid_read = OutboxEventPayload::ReadStateUpdated(ReadStateUpdatedPayload {
            conversation_id: id(2),
            actor_id: id(4),
            highest_seen_sequence: -1,
            version: 1,
        });
        assert_eq!(
            invalid_read.to_json(),
            Err(OutboxEventError::MalformedPayload)
        );
    }

    #[test]
    fn work_item_placement_is_optional_and_round_trips() {
        // Events enqueued before placement existed still decode.
        let old = OutboxEventPayload::from_json(
            "work_item.updated",
            serde_json::json!({"workItemId": id(6), "version": 2}),
        )
        .expect("decode an event without placement");
        assert!(
            matches!(old, OutboxEventPayload::WorkItemUpdated(ref payload) if payload.placement.is_none())
        );

        let placed = OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
            work_item_id: id(6),
            version: 3,
            placement: Some(WorkItemPlacement {
                status_id: id(7),
                archived: false,
                after_id: Some(id(8)),
                before_id: None,
            }),
        });
        let json = placed.to_json().expect("encode placement");
        assert_eq!(json["placement"]["statusId"], id(7).to_string());
        assert_eq!(json["placement"]["afterId"], id(8).to_string());
        assert!(json["placement"].get("beforeId").is_none());
        assert_eq!(
            OutboxEventPayload::from_json("work_item.updated", json).expect("decode placement"),
            placed
        );
    }

    #[test]
    fn message_updated_carries_ids_only_and_round_trips() {
        let updated = OutboxEventPayload::MessageUpdated(MessageUpdatedPayload {
            conversation_id: id(2),
            message_id: id(3),
            revision: 2,
            removed: true,
        });
        assert_eq!(updated.event_type(), "conversation.message.updated");
        assert_eq!(updated.resource_kind(), OutboxResourceKind::Message);
        assert_eq!(updated.resource_id(), id(3));
        assert_eq!(updated.conversation_id(), Some(id(2)));
        let json = updated.to_json().expect("message updated payload");
        let mut keys = json
            .as_object()
            .expect("payload object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, ["conversationId", "messageId", "removed", "revision"]);
        assert_eq!(
            OutboxEventPayload::from_json("conversation.message.updated", json),
            Ok(updated)
        );

        let with_text = serde_json::json!({
            "conversationId": id(2),
            "messageId": id(3),
            "revision": 2,
            "removed": false,
            "plainText": "secret",
        });
        assert_eq!(
            OutboxEventPayload::from_json("conversation.message.updated", with_text),
            Err(OutboxEventError::MalformedPayload)
        );
        let invalid = OutboxEventPayload::MessageUpdated(MessageUpdatedPayload {
            conversation_id: id(2),
            message_id: id(3),
            revision: 0,
            removed: false,
        });
        assert_eq!(invalid.to_json(), Err(OutboxEventError::MalformedPayload));
    }

    #[test]
    fn message_reactions_updated_is_conversation_scoped_ids_only() {
        let updated = OutboxEventPayload::MessageReactionsUpdated(MessageReactionsUpdatedPayload {
            conversation_id: id(2),
            message_id: id(3),
        });
        assert_eq!(
            updated.event_type(),
            "conversation.message.reactions.updated"
        );
        assert_eq!(updated.resource_kind(), OutboxResourceKind::Message);
        assert_eq!(updated.resource_id(), id(3));
        assert_eq!(updated.conversation_id(), Some(id(2)));
        let json = updated.to_json().expect("reactions updated payload");
        assert_eq!(
            json,
            serde_json::json!({"conversationId": id(2), "messageId": id(3)})
        );
        assert_eq!(
            OutboxEventPayload::from_json("conversation.message.reactions.updated", json),
            Ok(updated)
        );
        for refused in [
            serde_json::json!({"messageId": id(3)}),
            serde_json::json!({"conversationId": id(2), "messageId": id(3), "emoji": "✅"}),
            serde_json::json!({"conversationId": Uuid::nil(), "messageId": id(3)}),
        ] {
            assert_eq!(
                OutboxEventPayload::from_json("conversation.message.reactions.updated", refused),
                Err(OutboxEventError::MalformedPayload)
            );
        }
    }

    #[test]
    fn member_removed_targets_only_the_removed_actor_without_conversation_scope() {
        let removed =
            OutboxEventPayload::ConversationMemberRemoved(ConversationMemberRemovedPayload {
                conversation_id: id(2),
                actor_id: id(4),
                version: 3,
            });
        assert_eq!(removed.event_type(), "conversation.member.removed");
        assert_eq!(removed.conversation_id(), None);
        assert_eq!(removed.recipient_actor_id(), Some(id(4)));
        assert_eq!(removed.resource_kind(), OutboxResourceKind::Conversation);
        assert_eq!(removed.resource_id(), id(2));
        let json = removed.to_json().expect("member removed payload");
        assert_eq!(
            json,
            serde_json::json!({"conversationId": id(2), "actorId": id(4), "version": 3})
        );
        assert_eq!(
            OutboxEventPayload::from_json("conversation.member.removed", json),
            Ok(removed)
        );
        for refused in [
            serde_json::json!({"conversationId": id(2), "actorId": Uuid::nil(), "version": 3}),
            serde_json::json!({"conversationId": id(2), "actorId": id(4), "version": 0}),
            serde_json::json!({"conversationId": id(2), "actorId": id(4), "version": 3, "name": "x"}),
        ] {
            assert_eq!(
                OutboxEventPayload::from_json("conversation.member.removed", refused),
                Err(OutboxEventError::MalformedPayload)
            );
        }
    }
}

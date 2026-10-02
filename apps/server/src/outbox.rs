use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{domain::RunState, error::AppError};

pub const OUTBOX_SCHEMA_VERSION: i32 = 1;
pub const DEFAULT_MAX_OUTBOX_ATTEMPTS: i32 = 10;
pub const MAX_OUTBOX_PAYLOAD_BYTES: usize = 65_536;
pub const MAX_OUTBOX_WORKER_ID_CHARACTERS: usize = 128;
pub const MAX_OUTBOX_BATCH_SIZE: i64 = 100;
pub const MAX_OUTBOX_LEASE_SECONDS: i32 = 300;
pub const MAX_OUTBOX_ERROR_SUMMARY_CHARACTERS: usize = 2_000;

const RETRY_BASE_SECONDS: u64 = 1;
const RETRY_MAX_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxConversationKind {
    Channel,
    Thread,
    Dm,
    Group,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxResourceKind {
    Project,
    Conversation,
    Message,
    ReadState,
    AgentRun,
    WorkItem,
}

impl OutboxResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Conversation => "conversation",
            Self::Message => "message",
            Self::ReadState => "read_state",
            Self::AgentRun => "agent_run",
            Self::WorkItem => "work_item",
        }
    }

    fn parse(value: &str) -> Result<Self, OutboxEventError> {
        match value {
            "project" => Ok(Self::Project),
            "conversation" => Ok(Self::Conversation),
            "message" => Ok(Self::Message),
            "read_state" => Ok(Self::ReadState),
            "agent_run" => Ok(Self::AgentRun),
            "work_item" => Ok(Self::WorkItem),
            _ => Err(OutboxEventError::MalformedMetadata),
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
/// message text in an outbox payload, because it is stored in outbox_events
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkItemUpdatedPayload {
    pub work_item_id: Uuid,
    pub version: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectMembershipUpdatedPayload {
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub version: i64,
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
    WorkItemUpdated(WorkItemUpdatedPayload),
    ProjectMembershipUpdated(ProjectMembershipUpdatedPayload),
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
            Self::WorkItemUpdated(_) => "work_item.updated",
            Self::ProjectMembershipUpdated(_) => "project.membership.updated",
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
            // The removed actor can no longer see the group, so delivery must
            // not depend on that visibility: the event has no conversation
            // scope and goes only to them (see recipient_actor_id).
            Self::ConversationMemberRemoved(_)
            | Self::WorkItemUpdated(_)
            | Self::ProjectMembershipUpdated(_) => None,
        }
    }

    fn recipient_actor_id(&self) -> Option<Uuid> {
        match self {
            // Read cursors are private per-Actor state. The durable event is
            // still Project/Conversation scoped for validation and refetch,
            // but realtime delivery must target only the Actor that owns it.
            Self::ReadStateUpdated(payload) => Some(payload.actor_id),
            Self::ConversationMemberRemoved(payload) => Some(payload.actor_id),
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
            Self::AgentRunUpdated(_) => OutboxResourceKind::AgentRun,
            Self::WorkItemUpdated(_) => OutboxResourceKind::WorkItem,
            Self::ProjectMembershipUpdated(_) => OutboxResourceKind::Project,
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
            Self::WorkItemUpdated(payload) => payload.work_item_id,
            Self::ProjectMembershipUpdated(payload) => payload.project_id,
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
            Self::WorkItemUpdated(payload) => serde_json::to_value(payload),
            Self::ProjectMembershipUpdated(payload) => serde_json::to_value(payload),
        }
        .map_err(|_| OutboxEventError::MalformedPayload)?;
        validate_payload_size(&value)?;
        Ok(value)
    }

    fn from_json(event_type: &str, value: serde_json::Value) -> Result<Self, OutboxEventError> {
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
            "work_item.updated" => Self::WorkItemUpdated(
                serde_json::from_value(value).map_err(|_| OutboxEventError::MalformedPayload)?,
            ),
            "project.membership.updated" => Self::ProjectMembershipUpdated(
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
    max_attempts: i32,
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
            max_attempts: DEFAULT_MAX_OUTBOX_ATTEMPTS,
        })
    }

    pub fn with_max_attempts(mut self, max_attempts: i32) -> Result<Self, OutboxEventError> {
        if !(1..=100).contains(&max_attempts) {
            return Err(OutboxEventError::MalformedMetadata);
        }
        self.max_attempts = max_attempts;
        Ok(self)
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

#[derive(Clone, FromRow)]
pub struct ClaimedOutboxEvent {
    event_id: Uuid,
    event_cursor: i64,
    schema_version: i32,
    project_id: Uuid,
    conversation_id: Option<Uuid>,
    resource_kind: String,
    resource_id: Uuid,
    event_type: String,
    payload: serde_json::Value,
    attempt_count: i32,
    max_attempts: i32,
    occurred_at: String,
}

impl ClaimedOutboxEvent {
    pub fn event_id(&self) -> Uuid {
        self.event_id
    }

    pub fn event_cursor(&self) -> i64 {
        self.event_cursor
    }

    pub fn attempt_count(&self) -> i32 {
        self.attempt_count
    }

    pub fn max_attempts(&self) -> i32 {
        self.max_attempts
    }

    pub fn decode(&self) -> Result<OutboxEventPayload, OutboxEventError> {
        if self.schema_version != OUTBOX_SCHEMA_VERSION
            || self.event_cursor < 1
            || self.project_id.is_nil()
            || self.resource_id.is_nil()
            || self.attempt_count < 1
            || self.max_attempts < self.attempt_count
        {
            return Err(OutboxEventError::MalformedMetadata);
        }
        let resource_kind = OutboxResourceKind::parse(&self.resource_kind)?;
        let payload = OutboxEventPayload::from_json(&self.event_type, self.payload.clone())?;
        if self.conversation_id != payload.conversation_id()
            || resource_kind != payload.resource_kind()
            || self.resource_id != payload.resource_id()
        {
            return Err(OutboxEventError::MetadataMismatch);
        }
        Ok(payload)
    }

    pub fn envelope(&self) -> Result<OutboxEnvelope, OutboxEventError> {
        let decoded = self.decode()?;
        let conversation_id = decoded.conversation_id();
        let recipient_actor_id = decoded.recipient_actor_id();
        let payload = decoded.to_json()?;
        Ok(OutboxEnvelope {
            event_id: self.event_id,
            event_cursor: self.event_cursor,
            version: self.schema_version,
            project_id: self.project_id,
            conversation_id,
            recipient_actor_id,
            event_type: self.event_type.clone(),
            occurred_at: self.occurred_at.clone(),
            payload,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxFailureKind {
    MalformedEvent,
    Serialization,
    PublicationUnavailable,
}

impl OutboxFailureKind {
    pub fn operator_summary(self) -> &'static str {
        match self {
            Self::MalformedEvent => "outbox event is malformed",
            Self::Serialization => "outbox event serialization failed",
            Self::PublicationUnavailable => "realtime publication was unavailable",
        }
    }

    pub fn is_permanent(self) -> bool {
        matches!(self, Self::MalformedEvent | Self::Serialization)
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

pub async fn enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    event: &NewOutboxEvent,
) -> Result<Uuid, AppError> {
    sqlx::query(
        r#"
        INSERT INTO outbox_events (
            event_id, schema_version, project_id, conversation_id,
            resource_kind, resource_id, event_type, payload, max_attempts
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(event.event_id)
    .bind(OUTBOX_SCHEMA_VERSION)
    .bind(event.project_id)
    .bind(event.payload.conversation_id())
    .bind(event.payload.resource_kind().as_str())
    .bind(event.payload.resource_id())
    .bind(event.payload.event_type())
    .bind(&event.payload_json)
    .bind(event.max_attempts)
    .execute(&mut **transaction)
    .await?;
    Ok(event.event_id)
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
    let state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM agent_runs WHERE id = $1 AND conversation_id = $2",
    )
    .bind(agent_run_id)
    .bind(conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
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

pub async fn claim_ready(
    transaction: &mut Transaction<'_, Postgres>,
    worker_id: &str,
    batch_size: i64,
    lease_seconds: i32,
) -> Result<Vec<ClaimedOutboxEvent>, AppError> {
    validate_worker_options(worker_id, batch_size, lease_seconds)?;

    // A worker may die after claiming its final permitted attempt. Once that
    // lease expires there is no legal next claim, so finalize the row here
    // instead of leaving it permanently unpublished and non-terminal.
    sqlx::query(
        r#"
        UPDATE outbox_events
        SET failed_at = now(),
            lease_owner = NULL,
            lease_expires_at = NULL,
            last_error_summary = 'outbox publication lease expired after final attempt',
            updated_at = now()
        WHERE published_at IS NULL
          AND failed_at IS NULL
          AND attempt_count >= max_attempts
          AND lease_expires_at <= now()
        "#,
    )
    .execute(&mut **transaction)
    .await?;

    let mut claimed = sqlx::query_as::<_, ClaimedOutboxEvent>(
        r#"
        WITH ready AS MATERIALIZED (
            SELECT event_id
            FROM outbox_events
            WHERE published_at IS NULL
              AND failed_at IS NULL
              AND available_at <= now()
              AND attempt_count < max_attempts
              AND (lease_expires_at IS NULL OR lease_expires_at <= now())
            ORDER BY event_cursor
            FOR UPDATE SKIP LOCKED
            LIMIT $1
        )
        UPDATE outbox_events event
        SET lease_owner = $2,
            lease_expires_at = now() + make_interval(secs => $3::double precision),
            attempt_count = event.attempt_count + 1,
            updated_at = now()
        FROM ready
        WHERE event.event_id = ready.event_id
        RETURNING event.event_id,
                  event.event_cursor,
                  event.schema_version,
                  event.project_id,
                  event.conversation_id,
                  event.resource_kind,
                  event.resource_id,
                  event.event_type,
                  event.payload,
                  event.attempt_count,
                  event.max_attempts,
                  to_char(
                      event.created_at AT TIME ZONE 'UTC',
                      'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'
                  ) AS occurred_at
        "#,
    )
    .bind(batch_size)
    .bind(worker_id)
    .bind(lease_seconds)
    .fetch_all(&mut **transaction)
    .await?;
    claimed.sort_by_key(|event| event.event_cursor);
    Ok(claimed)
}

pub async fn mark_published(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    worker_id: &str,
) -> Result<(), AppError> {
    validate_worker_options(worker_id, 1, 1)?;
    let updated = sqlx::query(
        r#"
        UPDATE outbox_events
        SET published_at = now(),
            lease_owner = NULL,
            lease_expires_at = NULL,
            last_error_summary = NULL,
            updated_at = now()
        WHERE event_id = $1
          AND lease_owner = $2
          AND published_at IS NULL
          AND failed_at IS NULL
        "#,
    )
    .bind(event_id)
    .bind(worker_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict("outbox_event", "claim lost"));
    }
    Ok(())
}

pub async fn record_failure(
    transaction: &mut Transaction<'_, Postgres>,
    claim: &ClaimedOutboxEvent,
    worker_id: &str,
    failure: OutboxFailureKind,
) -> Result<(), AppError> {
    validate_worker_options(worker_id, 1, 1)?;
    let retry_seconds = i32::try_from(retry_delay(claim.attempt_count).as_secs())
        .map_err(|_| AppError::Internal("calculate outbox retry"))?;
    let permanent = failure.is_permanent();
    let updated = sqlx::query(
        r#"
        UPDATE outbox_events
        SET last_error_summary = $3,
            failed_at = CASE
                WHEN $5 OR attempt_count >= max_attempts THEN now()
                ELSE failed_at
            END,
            available_at = CASE
                WHEN $5 OR attempt_count >= max_attempts THEN available_at
                ELSE now() + make_interval(secs => $4::double precision)
            END,
            lease_owner = NULL,
            lease_expires_at = NULL,
            updated_at = now()
        WHERE event_id = $1
          AND lease_owner = $2
          AND published_at IS NULL
          AND failed_at IS NULL
        "#,
    )
    .bind(claim.event_id)
    .bind(worker_id)
    .bind(failure.operator_summary())
    .bind(retry_seconds)
    .bind(permanent)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict("outbox_event", "claim lost"));
    }
    Ok(())
}

pub fn retry_delay(attempt_count: i32) -> Duration {
    let exponent = u32::try_from(attempt_count.saturating_sub(1))
        .unwrap_or(0)
        .min(31);
    Duration::from_secs(
        RETRY_BASE_SECONDS
            .saturating_mul(1_u64 << exponent)
            .min(RETRY_MAX_SECONDS),
    )
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

fn validate_worker_options(
    worker_id: &str,
    batch_size: i64,
    lease_seconds: i32,
) -> Result<(), AppError> {
    let worker_id_is_valid = !worker_id.is_empty()
        && worker_id.chars().count() <= MAX_OUTBOX_WORKER_ID_CHARACTERS
        && worker_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        });
    if !worker_id_is_valid
        || !(1..=MAX_OUTBOX_BATCH_SIZE).contains(&batch_size)
        || !(1..=MAX_OUTBOX_LEASE_SECONDS).contains(&lease_seconds)
    {
        return Err(AppError::Internal("invalid outbox worker options"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::domain::RunState;

    use super::{
        AgentRunUpdatedPayload, ClaimedOutboxEvent, ConversationCreatedPayload,
        ConversationMemberRemovedPayload, MAX_OUTBOX_ERROR_SUMMARY_CHARACTERS,
        MessageCreatedPayload, MessageReactionsUpdatedPayload, MessageUpdatedPayload,
        OUTBOX_SCHEMA_VERSION, OutboxConversationKind, OutboxEventError, OutboxEventPayload,
        OutboxFailureKind, OutboxResourceKind, ProjectMembershipUpdatedPayload,
        ReadStateUpdatedPayload, WorkItemUpdatedPayload, retry_delay,
    };
    use std::time::Duration;
    use uuid::Uuid;

    fn id(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn message_claim() -> ClaimedOutboxEvent {
        ClaimedOutboxEvent {
            event_id: Uuid::now_v7(),
            event_cursor: 42,
            schema_version: OUTBOX_SCHEMA_VERSION,
            project_id: id(1),
            conversation_id: Some(id(2)),
            resource_kind: OutboxResourceKind::Message.as_str().to_owned(),
            resource_id: id(3),
            event_type: "conversation.message.created".to_owned(),
            payload: serde_json::json!({
                "conversationId": id(2),
                "messageId": id(3),
                "sequence": 7,
                "authorActorId": id(4),
            }),
            attempt_count: 1,
            max_attempts: 10,
            occurred_at: "2026-07-31T15:00:00.000000Z".to_owned(),
        }
    }

    #[test]
    fn typed_message_event_becomes_the_versioned_realtime_envelope() {
        let claim = message_claim();
        assert_eq!(
            claim.decode().expect("typed payload"),
            OutboxEventPayload::MessageCreated(MessageCreatedPayload {
                conversation_id: id(2),
                message_id: id(3),
                sequence: 7,
                author_actor_id: id(4),
            })
        );

        let envelope = claim.envelope().expect("realtime envelope");
        let json = serde_json::to_value(envelope).expect("envelope JSON");
        assert_eq!(json["event_id"], claim.event_id().to_string());
        assert_eq!(json["event_cursor"], 42);
        assert_eq!(json["version"], 1);
        assert_eq!(json["conversationId"], id(2).to_string());
        assert_eq!(json["type"], "conversation.message.created");
        assert_eq!(json["payload"]["messageId"], id(3).to_string());
        assert!(json.get("recipient_actor_id").is_none());
    }

    #[test]
    fn validated_read_state_claim_targets_only_its_actor() {
        let mut claim = message_claim();
        claim.resource_kind = OutboxResourceKind::ReadState.as_str().to_owned();
        claim.resource_id = id(4);
        claim.event_type = "conversation.read_state.updated".to_owned();
        claim.payload = serde_json::json!({
            "conversationId": id(2),
            "actorId": id(4),
            "highestSeenSequence": 9,
            "version": 2,
        });

        let envelope = claim.envelope().expect("validated read-state envelope");
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
    fn malformed_poison_events_are_isolated_at_decode_time() {
        let mut unknown = message_claim();
        unknown.event_type = "conversation.future.changed".to_owned();
        assert_eq!(unknown.decode(), Err(OutboxEventError::UnknownEventType));

        let mut extra_field = message_claim();
        extra_field.payload["secret"] = serde_json::json!("must not pass through");
        assert_eq!(
            extra_field.decode(),
            Err(OutboxEventError::MalformedPayload)
        );

        let mut mismatched = message_claim();
        mismatched.resource_id = id(9);
        assert_eq!(mismatched.decode(), Err(OutboxEventError::MetadataMismatch));
    }

    #[test]
    fn retry_backoff_is_bounded_and_error_summaries_are_redacted_categories() {
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(2));
        assert_eq!(retry_delay(9), Duration::from_secs(256));
        assert_eq!(retry_delay(10), Duration::from_secs(300));
        assert_eq!(retry_delay(i32::MAX), Duration::from_secs(300));

        for failure in [
            OutboxFailureKind::MalformedEvent,
            OutboxFailureKind::Serialization,
            OutboxFailureKind::PublicationUnavailable,
        ] {
            let summary = failure.operator_summary();
            assert!(summary.chars().count() <= MAX_OUTBOX_ERROR_SUMMARY_CHARACTERS);
            assert!(!summary.contains("payload"));
            assert!(!summary.contains("token"));
        }
        assert!(OutboxFailureKind::MalformedEvent.is_permanent());
        assert!(OutboxFailureKind::Serialization.is_permanent());
        assert!(!OutboxFailureKind::PublicationUnavailable.is_permanent());
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

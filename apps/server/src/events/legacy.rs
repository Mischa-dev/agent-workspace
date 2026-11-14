//! The outbox's events, mirrored into the log (K3.2, K3 4.3 step 1). Every
//! `outbox::enqueue` stages the same event (same id) in the log, so every
//! outbox writer is in the log from the first slice. The log's type is the
//! catalogue's name; the outbox's name stays as `legacy_type`, which the
//! web's realtime envelope reads until W1 moves to catalogue names.

use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{Audience, NewEvent, catalog};
use crate::{error::AppError, outbox::OutboxEventPayload};

/// Stage the log's copy of one outbox event.
pub async fn mirror(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    project_id: Uuid,
    payload: &OutboxEventPayload,
    payload_json: &serde_json::Value,
) -> Result<Uuid, AppError> {
    use OutboxEventPayload as P;
    let conversation = |id: Uuid| Audience::ConversationOf(id);
    // (type, thing kind, thing id, access thing, audience, version, actor)
    let (ty, kind, id, access, audience, version, actor): (
        &'static super::EventType,
        &'static str,
        Uuid,
        Option<Uuid>,
        Audience,
        Option<i64>,
        Option<Uuid>,
    ) = match payload {
        P::ConversationCreated(p) => (
            &catalog::CONVERSATION_CREATED,
            "conversation",
            p.conversation_id,
            None,
            conversation(p.conversation_id),
            Some(p.version),
            None,
        ),
        P::ConversationUpdated(p) => (
            &catalog::CONVERSATION_UPDATED,
            "conversation",
            p.conversation_id,
            None,
            conversation(p.conversation_id),
            Some(p.version),
            None,
        ),
        P::ReplyRequestsUpdated(p) => (
            &catalog::CONVERSATION_REPLY_REQUESTS_CHANGED,
            "conversation",
            p.conversation_id,
            None,
            conversation(p.conversation_id),
            None,
            None,
        ),
        P::ConversationArchived(p) => (
            &catalog::CONVERSATION_ARCHIVED,
            "conversation",
            p.conversation_id,
            None,
            conversation(p.conversation_id),
            Some(p.version),
            None,
        ),
        P::MessageCreated(p) => (
            &catalog::MESSAGE_CREATED,
            "message",
            p.message_id,
            Some(p.conversation_id),
            conversation(p.conversation_id),
            None,
            Some(p.author_actor_id),
        ),
        P::MessageUpdated(p) => (
            if p.removed {
                &catalog::MESSAGE_REMOVED
            } else {
                &catalog::MESSAGE_EDITED
            },
            "message",
            p.message_id,
            Some(p.conversation_id),
            conversation(p.conversation_id),
            Some(i64::from(p.revision)),
            None,
        ),
        P::MessageReactionsUpdated(p) => (
            &catalog::MESSAGE_REACTIONS_CHANGED,
            "message",
            p.message_id,
            Some(p.conversation_id),
            conversation(p.conversation_id),
            None,
            None,
        ),
        P::ReadStateUpdated(p) => (
            &catalog::CONVERSATION_READ,
            "conversation",
            p.conversation_id,
            None,
            Audience::Account(p.actor_id),
            Some(p.version),
            Some(p.actor_id),
        ),
        P::ConversationMemberRemoved(p) => (
            &catalog::CONVERSATION_MEMBER_REMOVED,
            "conversation",
            p.conversation_id,
            None,
            Audience::Account(p.actor_id),
            Some(p.version),
            None,
        ),
        P::AgentRunUpdated(p) => (
            &catalog::RUN_UPDATED,
            "run",
            p.agent_run_id,
            Some(p.conversation_id),
            conversation(p.conversation_id),
            None,
            None,
        ),
        P::WorkItemUpdated(p) => (
            &catalog::TASK_PLACEMENT_CHANGED,
            "task",
            p.work_item_id,
            None,
            Audience::Space,
            Some(p.version),
            None,
        ),
        P::ProjectMembershipUpdated(p) => (
            &catalog::SPACE_MEMBER_CHANGED,
            "space",
            p.project_id,
            None,
            Audience::Space,
            Some(p.version),
            None,
        ),
        P::NotificationsUpdated(p) => (
            &catalog::NOTIFICATION_CHANGED,
            "account",
            p.actor_id,
            None,
            Audience::Account(p.actor_id),
            None,
            None,
        ),
        // A workspace board has no project of its own: whoever can see the
        // board hears it, checked per event (as realtime does today).
        P::WhiteboardUpdated(p) => (
            &catalog::WHITEBOARD_SCENE_CHANGED,
            "whiteboard",
            p.whiteboard_id,
            None,
            Audience::Thing,
            Some(p.version),
            None,
        ),
        P::PageUpdated(p) => (
            &catalog::PAGE_CONTENT_SAVED,
            "page",
            p.page_id,
            None,
            Audience::Space,
            Some(p.version),
            None,
        ),
        P::PageDiscussionUpdated(p) => (
            &catalog::PAGE_DISCUSSION_CHANGED,
            "page",
            p.page_id,
            None,
            Audience::Space,
            None,
            None,
        ),
        P::CollectionUpdated(p) => (
            &catalog::COLLECTION_UPDATED,
            "collection",
            p.collection_id,
            None,
            Audience::Space,
            None,
            None,
        ),
        P::SheetUpdated(p) => (
            &catalog::SHEET_UPDATED,
            "sheet",
            p.sheet_id,
            None,
            Audience::Space,
            Some(p.revision),
            None,
        ),
    };
    // A notification's subject is the account, which is not a thing.
    let thing = (kind != "account").then_some((id, kind));
    let payload_json = if payload_json.is_object() {
        payload_json.clone()
    } else {
        json!({})
    };
    super::emit(
        transaction,
        NewEvent {
            id: event_id,
            ty,
            workspace_id: None,
            space_id: Some(project_id),
            thing,
            access_thing: access,
            version,
            changes: &[],
            audience,
            payload: payload_json,
            actor,
            legacy_type: Some(payload.event_type()),
        },
    )
    .await
}

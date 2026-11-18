//! Change notices in the log: `outbox::enqueue` stages each typed notice
//! here as a catalogue event. The log's type is the catalogue's name; the
//! notice's own name stays as `legacy_type`, which the web's protocol-1
//! frame reads until the web moves to catalogue names, and which the
//! notifier decodes the payload by.

use serde_json::json;

use uuid::Uuid;

use super::{Audience, NewEvent, catalog};
use crate::{error::AppError, outbox::OutboxEventPayload};

/// Stage one notice as an event in the change's transaction.
pub async fn stage(
    connection: &mut sqlx::PgConnection,
    event_id: Uuid,
    project_id: Uuid,
    payload: &OutboxEventPayload,
    payload_json: &serde_json::Value,
) -> Result<Uuid, AppError> {
    use OutboxEventPayload as P;
    // A task's notice is the task's own fact (K3.7): created, or changed
    // with what changed, from the activity this transaction wrote for it.
    // A move or a comment alone stays the board's placement signal.
    let task = match payload {
        P::WorkItemUpdated(p) => Some(task_change(connection, project_id, p.work_item_id).await?),
        _ => None,
    };
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
            if p.state.is_some_and(|state| state.is_terminal()) {
                &catalog::RUN_FINISHED
            } else {
                &catalog::RUN_UPDATED
            },
            "run",
            p.agent_run_id,
            Some(p.conversation_id),
            conversation(p.conversation_id),
            None,
            None,
        ),
        // A1's run record (build 1G): finished or still going.
        P::RunChanged(p) => (
            if crate::runs::RunState::terminal_keys().contains(&p.state.as_str()) {
                &catalog::RUN_FINISHED
            } else {
                &catalog::RUN_UPDATED
            },
            "run",
            p.run_id,
            p.conversation_id,
            p.conversation_id.map_or(Audience::Space, conversation),
            Some(p.version),
            None,
        ),
        P::WorkItemUpdated(p) => (
            task.as_ref()
                .map_or(&catalog::TASK_PLACEMENT_CHANGED, |(ty, _, _)| *ty),
            "task",
            p.work_item_id,
            None,
            Audience::Space,
            Some(p.version),
            task.as_ref().and_then(|(_, _, actor)| *actor),
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
        P::ProjectModulesUpdated(p) => (
            &catalog::SPACE_MODULES_CHANGED,
            "space",
            p.project_id,
            None,
            Audience::Space,
            None,
            None,
        ),
        P::WorkspaceLifecycleChanged(p) => (
            &catalog::WORKSPACE_LIFECYCLE_CHANGED,
            "workspace",
            p.workspace_id,
            None,
            Audience::Workspace,
            None,
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
        P::AccountManagersChanged(p) => (
            &catalog::ACCOUNT_MANAGERS_CHANGED,
            "account",
            p.account_id,
            None,
            Audience::Account(p.actor_id),
            None,
            None,
        ),
        P::MessageDraftChanged(p) => (
            &catalog::MESSAGE_DRAFT_CHANGED,
            "account",
            p.actor_id,
            None,
            Audience::Account(p.actor_id),
            None,
            Some(p.actor_id),
        ),
        P::NotificationKeywordsChanged(p) => (
            &catalog::NOTIFICATION_KEYWORDS_CHANGED,
            "account",
            p.actor_id,
            None,
            Audience::Account(p.actor_id),
            None,
            Some(p.actor_id),
        ),
        P::SidebarChanged(p) => (
            &catalog::SIDEBAR_CHANGED,
            "account",
            p.actor_id,
            None,
            Audience::Account(p.actor_id),
            None,
            Some(p.actor_id),
        ),
        P::SavedSearchesChanged(p) => (
            &catalog::SAVED_SEARCHES_CHANGED,
            "account",
            p.actor_id,
            None,
            Audience::Account(p.actor_id),
            None,
            Some(p.actor_id),
        ),
        P::DmRulesChanged(p) => (
            &catalog::DM_RULES_CHANGED,
            "account",
            p.actor_id,
            None,
            Audience::Account(p.actor_id),
            None,
            Some(p.actor_id),
        ),
        P::AccountStatusChanged(p) => (
            &catalog::ACCOUNT_STATUS_CHANGED,
            "account",
            p.account_id,
            None,
            Audience::Workspace,
            None,
            Some(p.account_id),
        ),
        P::AccountProfileChanged(p) => (
            &catalog::ACCOUNT_PROFILE_CHANGED,
            "account",
            p.account_id,
            None,
            Audience::Workspace,
            None,
            Some(p.account_id),
        ),
        P::CustomEmojiChanged(p) => (
            &catalog::CUSTOM_EMOJI_CHANGED,
            "custom_emoji",
            p.emoji_id,
            None,
            Audience::Workspace,
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
        P::DashboardUpdated(p) => (
            &catalog::DASHBOARD_UPDATED,
            "dashboard",
            p.dashboard_id,
            None,
            Audience::Space,
            Some(p.version),
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
    let changes = task
        .as_ref()
        .map(|(_, changes, _)| changes.iter().map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    super::emit(
        connection,
        NewEvent {
            id: event_id,
            ty,
            workspace_id: None,
            space_id: Some(project_id),
            thing,
            access_thing: access,
            version,
            changes: &changes,
            audience,
            payload: payload_json,
            actor,
            legacy_type: Some(payload.event_type()),
        },
    )
    .await
}

/// What this transaction did to a task, from the activity it wrote: the
/// task's created or changed fact with the keys of what changed, or the
/// placement signal when it wrote no activity (a move, a comment). The
/// activity's actor is who acted when the edge didn't say.
async fn task_change(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<(&'static super::EventType, Vec<String>, Option<Uuid>), AppError> {
    let activities = sqlx::query_as::<_, (String, Option<serde_json::Value>, Uuid)>(
        "SELECT kind, payload -> 'changedFields', actor_id FROM work_item_activity \
         WHERE project_id = $1 AND work_item_id = $2 AND created_at = now() ORDER BY id",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(&mut *connection)
    .await?;
    let actor = activities.first().map(|(_, _, actor)| *actor);
    if activities
        .iter()
        .any(|(kind, _, _)| kind == "work_item_created")
    {
        return Ok((&catalog::TASK_CREATED, Vec::new(), actor));
    }
    if activities.is_empty() {
        return Ok((&catalog::TASK_PLACEMENT_CHANGED, Vec::new(), None));
    }
    let mut changed = activities
        .iter()
        .filter_map(|(_, fields, _)| fields.as_ref().and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for (kind, _, _) in &activities {
        match kind.as_str() {
            "work_item_archived" => changed.push("archived".to_owned()),
            "work_item_restored" => changed.push("restored".to_owned()),
            _ => {}
        }
    }
    changed.sort();
    changed.dedup();
    changed.truncate(64);
    Ok((&catalog::TASK_UPDATED, changed, actor))
}

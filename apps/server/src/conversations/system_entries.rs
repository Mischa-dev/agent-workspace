//! System entries (C2.20): a line in a conversation's timeline that says what
//! happened there, such as "Lena made this into a page". An entry carries ids
//! and kinds only (rule 10); the timeline resolves the actor's name, and the
//! web reads the thing's title live with the reader's own access, so a thing
//! the reader can't open shows no title.

use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    outbox::{ConversationUpdatedPayload, OutboxEventPayload, enqueue_typed},
};

/// What a system entry says.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemEvent {
    /// Someone made a thing (a task, page, record, sheet row, sticky…) from
    /// a message here.
    #[serde(rename_all = "camelCase")]
    MadeInto {
        actor_id: Uuid,
        /// The thing's kind, as its module's manifest names it (`page`,
        /// `task`, `record`…).
        thing_kind: String,
        thing_id: Uuid,
        /// The message it was made from, when it was one message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<Uuid>,
    },
}

/// Posts a system entry in `conversation`, inside the caller's transaction,
/// after the caller has checked the actor may act there. Returns the entry's
/// sequence. Everyone with the conversation open sees it arrive.
pub(crate) async fn post(
    tx: &mut Transaction<'_, Postgres>,
    conversation: Uuid,
    event: &SystemEvent,
) -> Result<i64, AppError> {
    let sequence = crate::domain::allocate_conversation_sequence(tx, conversation).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (id, conversation_id, sequence, kind, system_event)
        VALUES ($1, $2, $3, 'system', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation)
    .bind(sequence)
    .bind(serde_json::to_value(event).map_err(|_| AppError::Internal("encode a system entry"))?)
    .execute(&mut **tx)
    .await?;
    announce(tx, conversation).await?;
    Ok(sequence)
}

/// Tells everyone with `conversation` open that its timeline changed.
async fn announce(tx: &mut Transaction<'_, Postgres>, conversation: Uuid) -> Result<(), AppError> {
    let (project, version) = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT project_id, version FROM conversations WHERE id = $1 -- visibility: checked by the caller",
    )
    .bind(conversation)
    .fetch_one(&mut **tx)
    .await?;
    enqueue_typed(
        tx,
        project,
        OutboxEventPayload::ConversationUpdated(ConversationUpdatedPayload {
            conversation_id: conversation,
            version,
        }),
    )
    .await?;
    Ok(())
}

/// A task made from a message (C2.20): links the task back to the message,
/// which must be one in `space` that `actor` can read, and with `say_so`
/// says so in the message's conversation. The one writer of these links for
/// every edge (the web's New task from this message, and task_create's
/// fromMessageId). Returns false, having written nothing, when the message
/// isn't one the actor can read there.
pub(crate) async fn task_made_from_message(
    tx: &mut Transaction<'_, Postgres>,
    space: Uuid,
    task: Uuid,
    actor: Uuid,
    message: Uuid,
    say_so: bool,
) -> Result<bool, AppError> {
    let conversation = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO work_item_links (
            work_item_id, project_id, target_kind, target_id, created_by_actor_id
        )
        SELECT $1, $2, 'message', message.id, $3
        FROM messages message
        WHERE message.id = $4 AND message.project_id = $2
          AND message.removed_at IS NULL
          AND conversation_visible_to(message.conversation_id, $3)
        ON CONFLICT (work_item_id, target_kind, target_id)
            DO UPDATE SET created_at = work_item_links.created_at
        RETURNING (SELECT conversation_id FROM messages WHERE id = $4 -- visibility: checked in this statement
        )
        "#,
    )
    .bind(task)
    .bind(space)
    .bind(actor)
    .bind(message)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(conversation) = conversation else {
        return Ok(false);
    };
    if !say_so {
        // The message's chip changes (attach_made_into).
        announce(tx, conversation).await?;
    } else {
        post(
            tx,
            conversation,
            &SystemEvent::MadeInto {
                actor_id: actor,
                thing_kind: "task".to_owned(),
                thing_id: task,
                message_id: Some(message),
            },
        )
        .await?;
    }
    Ok(true)
}

/// A thing a message was made into (C2.20), as a chip under the message:
/// its kind and id only. The web reads its title with the reader's access.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MadeIntoView {
    pub kind: String,
    pub id: Uuid,
}

/// Puts on each message of `entries` (a page of `conversation`'s timeline,
/// already read with the viewer's access) what it was made into: the tasks
/// linked to it, which only a reader who sees tasks in the space is told
/// of, and every other thing a system entry here says was made from it.
pub(crate) async fn attach_made_into<'e, E>(
    executor: E,
    conversation: Uuid,
    entries: &mut [super::TimelineEntryView],
    sees_tasks: bool,
) -> Result<(), AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    let messages = entries
        .iter()
        .filter_map(|entry| entry.message.as_ref())
        .filter(|message| message.removed.is_none())
        .map(|message| message.id)
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query_as::<_, (Uuid, String, Uuid)>(
        r#"
        SELECT made.message_id, made.kind, made.id
        FROM (
            SELECT link.target_id AS message_id, 'task' AS kind,
                   link.work_item_id AS id, link.created_at
            FROM work_item_links link
            JOIN task_rows item ON item.id = link.work_item_id
            WHERE $3 AND link.target_kind = 'message' AND link.target_id = ANY($1)
            UNION ALL
            SELECT (entry.system_event->>'messageId')::uuid,
                   entry.system_event->>'thingKind',
                   (entry.system_event->>'thingId')::uuid, entry.created_at
            FROM conversation_entries entry
            WHERE entry.conversation_id = $2 AND entry.kind = 'system'
              AND entry.system_event->>'type' = 'made_into'
              AND entry.system_event->>'thingKind' <> 'task'
              AND (entry.system_event->>'messageId')::uuid = ANY($1)
        ) made
        ORDER BY made.created_at, made.id
        "#,
    )
    .bind(&messages)
    .bind(conversation)
    .bind(sees_tasks)
    .fetch_all(executor)
    .await?;
    for message in entries
        .iter_mut()
        .filter_map(|entry| entry.message.as_mut())
    {
        for (_, kind, id) in rows.iter().filter(|(from, ..)| *from == message.id) {
            if !message
                .made_into
                .iter()
                .any(|made| made.id == *id && made.kind == *kind)
            {
                message.made_into.push(MadeIntoView {
                    kind: kind.clone(),
                    id: *id,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_event_is_ids_and_kinds() {
        let event = SystemEvent::MadeInto {
            actor_id: Uuid::nil(),
            thing_kind: "page".to_owned(),
            thing_id: Uuid::nil(),
            message_id: None,
        };
        let value = serde_json::to_value(&event).expect("encode");
        assert_eq!(value["type"], "made_into");
        assert_eq!(value["thingKind"], "page");
        assert!(value.get("messageId").is_none());
        assert_eq!(
            serde_json::from_value::<SystemEvent>(value).expect("decode"),
            event
        );
    }
}

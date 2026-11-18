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
    Ok(sequence)
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

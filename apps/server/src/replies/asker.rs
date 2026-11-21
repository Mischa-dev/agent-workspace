//! A2.15: who asked. An agent says what it acts on (`context`: a reply
//! request addressed to it, one of its runs, or an event it heard) and the
//! call is made for that request's asker, the run's requester or the
//! event's actor: every write records them (K1.25), and a role held to its
//! requester acts only within what they may do. Answering a reply request
//! (claiming, sending, passing) is made for its asker without being told.
//!
//! Only the agent itself can name its context: a reply request addressed
//! to someone else, another account's run or an event the caller wasn't
//! shown is refused, so nobody can write as if someone else had asked.

use serde::Deserialize;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{error::AppError, ops::OpError};

/// What a call says it acts on (`context` on any operation).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Context {
    pub reply_request_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
}

const SHAPE: &str = "context names one of replyRequestId, runId or eventId, by id.";

/// `context` as a tool lists it. Every tool takes it, hand-written ones
/// and operations alike.
pub fn schema() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "What this call acts on, so it is made for whoever asked. Name one, by id.",
        "properties": {
            "replyRequestId": {
                "type": "string",
                "format": "uuid",
                "description": "A reply request addressed to you.",
            },
            "runId": {
                "type": "string",
                "format": "uuid",
                "description": "One of your runs.",
            },
            "eventId": {
                "type": "string",
                "format": "uuid",
                "description": "An event you were shown.",
            },
        },
        "additionalProperties": false,
    })
}

/// `tool` with `context` among its arguments, unless it already lists one.
pub fn listed_with_context(mut tool: rmcp::model::Tool) -> rmcp::model::Tool {
    let mut input = std::sync::Arc::unwrap_or_clone(tool.input_schema);
    if let Some(properties) = input
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    {
        properties.entry("context").or_insert_with(schema);
    }
    tool.input_schema = std::sync::Arc::new(input);
    tool
}

/// The asker `context` names for `caller`, or a refusal naming `context`.
pub async fn resolve(
    connection: &mut PgConnection,
    caller: Uuid,
    context: Value,
) -> Result<Option<Uuid>, OpError> {
    let context: Context =
        serde_json::from_value(context).map_err(|_| OpError::invalid("context", SHAPE))?;
    let named = [
        context.reply_request_id.is_some(),
        context.run_id.is_some(),
        context.event_id.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count();
    if named != 1 {
        return Err(OpError::invalid("context", SHAPE));
    }
    let found = if let Some(request) = context.reply_request_id {
        of_reply_request(connection, caller, request).await
    } else if let Some(run) = context.run_id {
        of_run(connection, caller, run).await
    } else if let Some(event) = context.event_id {
        of_event(connection, caller, event).await
    } else {
        Ok(None)
    }
    .map_err(|error| OpError::logged("context", &error))?;
    match found {
        Some(asker) => Ok(asker),
        None => Err(OpError::invalid(
            "context",
            "That isn't a reply request addressed to you, one of your runs or an event you \
             were told of.",
        )),
    }
}

/// The asker of a reply request addressed to `caller`: who wrote the
/// message it asks about. `None` when it isn't addressed to them.
pub async fn of_reply_request(
    connection: &mut PgConnection,
    caller: Uuid,
    request: Uuid,
) -> Result<Option<Option<Uuid>>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT message.author_actor_id
        FROM reply_requests request
        JOIN messages message ON message.id = request.message_id
        WHERE request.id = $1 AND request.agent_actor_id = $2
          AND conversation_visible_to(request.conversation_id, $2)
        "#,
    )
    .bind(request)
    .bind(caller)
    .fetch_optional(&mut *connection)
    .await?
    .map(|author| (author != caller).then_some(author)))
}

/// The requester of one of `caller`'s runs.
async fn of_run(
    connection: &mut PgConnection,
    caller: Uuid,
    run: Uuid,
) -> Result<Option<Option<Uuid>>, AppError> {
    Ok(crate::runs::store::get(&mut *connection, run)
        .await?
        .filter(|run| run.account_id == caller)
        .map(|run| run.requested_by_account_id.filter(|asker| *asker != caller)))
}

/// The actor of an event `caller` may be told of.
async fn of_event(
    connection: &mut PgConnection,
    caller: Uuid,
    event: Uuid,
) -> Result<Option<Option<Uuid>>, AppError> {
    Ok(crate::events::heard_actor(connection, caller, event)
        .await?
        .map(|actor| actor.filter(|actor| *actor != caller)))
}

/// What answering reply request `request_id` acts as: the current call,
/// made for the request's asker when it is addressed to `caller`. Only
/// recording hangs on it, so a failed lookup leaves the call as it was.
pub async fn acting_for_request(
    state: &crate::AppState,
    caller: Uuid,
    request_id: &str,
) -> crate::access::acting::Acting {
    let mut acting = crate::access::acting::current();
    let Some(request) = crate::ops::refs::parse_id(request_id) else {
        return acting;
    };
    if acting.requested_by.is_some() {
        return acting;
    }
    let Ok(mut connection) = crate::db::scoped_tx(&state.pool).await else {
        return acting;
    };
    if let Ok(Some(asker)) = of_reply_request(&mut connection, caller, request).await {
        acting.requested_by = asker;
    }
    let _ = connection.commit().await;
    acting
}

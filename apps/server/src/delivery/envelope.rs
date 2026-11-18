//! The envelope (A2 3.4), the same in every transport: for a webhook it is
//! the Standard Webhooks body. It carries ids, kinds and the names needed to
//! route, never message text, page bodies or file contents: the agent reads
//! those with its own access.

use std::collections::HashMap;

use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{connections::Connection, cursor, read::Delivered, reason::Reason};
use crate::error::AppError;

/// The id an event goes by outside: `evt_<uuid>`.
pub fn event_id(id: Uuid) -> String {
    format!("evt_{id}")
}

/// Names the envelopes of one batch need, read once for the batch.
#[derive(Debug, Default)]
pub struct Lookups {
    workspaces: HashMap<Uuid, String>,
    accounts: HashMap<Uuid, String>,
}

impl Lookups {
    pub async fn for_batch(
        connection: &mut PgConnection,
        delivered: &[Delivered],
    ) -> Result<Self, AppError> {
        let mut workspaces = delivered
            .iter()
            .map(|item| item.event.workspace_id)
            .collect::<Vec<_>>();
        workspaces.sort_unstable();
        workspaces.dedup();
        let mut accounts = delivered
            .iter()
            .flat_map(|item| {
                [
                    item.event.actor_account_id,
                    item.event.requested_by_account_id,
                ]
            })
            .flatten()
            .collect::<Vec<_>>();
        accounts.sort_unstable();
        accounts.dedup();
        let workspaces = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, slug FROM organizations WHERE id = ANY($1)",
        )
        .bind(&workspaces)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .collect();
        let accounts = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, account_kind FROM actors WHERE id = ANY($1)",
        )
        .bind(&accounts)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .collect();
        Ok(Self {
            workspaces,
            accounts,
        })
    }
}

/// The envelope of one delivered event.
pub fn envelope(item: &Delivered, delivery: &Connection, lookups: &Lookups) -> Value {
    let event = &item.event;
    let account = |id: Uuid| {
        json!({
            "accountId": id,
            // For display and routing only, never a permission.
            "kind": lookups.accounts.get(&id).map_or("person", String::as_str),
        })
    };
    let mut body = json!({
        "type": event.event_type,
        "timestamp": event.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "id": event_id(event.id),
        "cursor": cursor::encode(&delivery.cursor_key, delivery.id, event.position),
        "workspace": {
            "id": event.workspace_id,
            "slug": lookups.workspaces.get(&event.workspace_id),
        },
        "recipient": { "accountId": delivery.account_id },
        "reason": item.reason.key(),
        "actor": event.actor_account_id.map(account),
        "requestedBy": event.requested_by_account_id.map(account),
        "data": data(item),
    });
    if item.reason == Reason::Own {
        body["self"] = Value::Bool(true);
    }
    body
}

/// What the event is about: its payload (ids and small values only, by the
/// log's rule) with where it happened.
fn data(item: &Delivered) -> Value {
    let event = &item.event;
    let mut data = match &event.payload {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    if let (Some(id), Some(kind)) = (event.thing_id, event.thing_kind.as_deref()) {
        data.entry("thing")
            .or_insert_with(|| json!({ "kind": kind, "id": id }));
    }
    if let Some(space) = event.space_id {
        data.entry("spaceId").or_insert(json!(space));
    }
    if let Some(conversation) = event.conversation_id {
        data.entry("conversationId").or_insert(json!(conversation));
    }
    if !event.changes.is_empty() {
        data.entry("changes").or_insert(json!(event.changes));
    }
    if let Some(version) = event.thing_version {
        data.entry("version").or_insert(json!(version));
    }
    Value::Object(data)
}

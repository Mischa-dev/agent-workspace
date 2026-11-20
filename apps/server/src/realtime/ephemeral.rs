//! The ephemeral bus (K5 3.6, K5.5): frames that are never stored and may
//! be lost, sent to every instance on the `ephemeral` channel. The sender
//! routes its own frames at once and skips them when they come back by
//! `from`; a frame sent inside a transaction (`publish_on_commit`) has no
//! `from` and is heard everywhere once it commits, the sender included.
//!
//! Nothing correct depends on a frame arriving: a lost delta is refetched
//! with its message, a lost revocation still closes the socket at the next
//! revalidation (15 s), a lost invalidation lets a cache run out (10 s).
//! With LISTEN off frames reach only this instance's sockets until the live
//! contract carries them (K5.19).

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{RealtimeEvent, RealtimeHub};
use crate::{AppState, error::AppError};

/// The bus's wire version.
const VERSION: u8 = 1;
/// PostgreSQL refuses a notification over 8,000 bytes; frames are packed
/// and split to stay under this.
const MAX_PAYLOAD_BYTES: usize = 7_500;
/// A streamed delta's text is cut into pieces of at most this many bytes,
/// each its own frame; the browser appends them in order.
const MAX_DELTA_PIECE_BYTES: usize = 6_000;

/// A cache every instance keeps and a write elsewhere makes stale.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheName {
    /// `modules::gate`'s module switches.
    Modules,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "frame")]
pub enum EphemeralFrame {
    /// Streamed agent text (`message.delta`), for the conversation's
    /// sockets. `payload` is the frame's payload as protocol 1 sends it.
    StreamDelta {
        space: Uuid,
        conversation: Option<Uuid>,
        payload: serde_json::Value,
        /// The conversation's root and whether it is restricted, so the
        /// live contract can route it with no lookup (K5.19); None from
        /// older instances, or when it couldn't be read.
        #[serde(default)]
        route: Option<(Uuid, bool)>,
    },
    /// A signed-out session: its sockets close.
    SessionRevoked { session: Uuid },
    /// Drop a cache's copy for a workspace (all of it when None).
    Invalidate {
        cache: CacheName,
        workspace: Option<Uuid>,
    },
    /// Someone came online or went offline (K5.10), for the workspaces
    /// they are in.
    Presence {
        account: Uuid,
        workspaces: Vec<Uuid>,
        online: bool,
    },
    /// Someone is typing in a conversation, heard as the conversation is:
    /// its space's readers, or its members when restricted.
    Typing {
        space: Uuid,
        root: Uuid,
        restricted: bool,
        conversation: Uuid,
        account: Uuid,
        until: String,
    },
    /// A watcher's cursor or selection in a document, for its other
    /// watchers.
    Awareness {
        document: super::documents::DocumentRef,
        connection: Uuid,
        account: Uuid,
        state: serde_json::Value,
    },
    /// A live document's change, for its watchers to apply without a sync.
    DocumentUpdate {
        document: super::documents::DocumentRef,
        generation: Uuid,
        version: i64,
        update: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct Envelope {
    v: u8,
    from: Option<Uuid>,
    frames: Vec<EphemeralFrame>,
}

/// Routes `frame` to this instance now and sends it to the others. A
/// failed send is logged, never returned: the frame is only a shortcut.
pub async fn publish(state: &AppState, frame: EphemeralFrame) {
    route(&state.realtime, &frame);
    super::contract::publish::send_ephemeral(state, &frame).await;
    for payload in packed(Some(state.realtime.origin()), split(frame)) {
        let sent = crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query("SELECT pg_notify(realtime_channel('ephemeral'), $1)")
                .bind(payload)
                .execute(&mut *connection)
                .await
        })
        .await;
        if let Err(error) = sent {
            tracing::warn!(
                error_category = AppError::from(error).log_category(),
                "ephemeral frame not sent to other instances"
            );
        }
    }
}

/// Sends `frame` to every instance, this one included, once `transaction`
/// commits; nothing is sent if it rolls back.
pub async fn publish_on_commit(
    transaction: &mut PgConnection,
    frame: EphemeralFrame,
) -> Result<(), AppError> {
    for payload in packed(None, split(frame)) {
        sqlx::query("SELECT pg_notify(realtime_channel('ephemeral'), $1)")
            .bind(payload)
            .execute(&mut *transaction)
            .await?;
    }
    Ok(())
}

/// Closes the session's sockets on every instance.
pub async fn revoke_session(state: &AppState, session: Uuid) {
    publish(state, EphemeralFrame::SessionRevoked { session }).await;
}

/// Streams agent text to the conversation's sockets on every instance.
pub async fn stream_delta(state: &AppState, event: RealtimeEvent) {
    let route = match delta_route(state, &event).await {
        Ok(route) => route,
        Err(error) => {
            tracing::warn!(
                error_category = error.log_category(),
                "streamed text's conversation not read; live contract connections miss it"
            );
            None
        }
    };
    publish(
        state,
        EphemeralFrame::StreamDelta {
            space: event.project_id,
            conversation: event.conversation_id,
            payload: event.payload,
            route,
        },
    )
    .await;
}

/// A streamed reply's conversation root and restriction, as the agent
/// running it sees the conversation.
async fn delta_route(
    state: &AppState,
    event: &RealtimeEvent,
) -> Result<Option<(Uuid, bool)>, AppError> {
    let (Some(conversation), Some(run)) = (
        event.conversation_id,
        event
            .payload
            .get("agent_run_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|run| Uuid::parse_str(run).ok()),
    ) else {
        return Ok(None);
    };
    // The run's own account, on any host (a built-in run, an assistant's or
    // a linked host's).
    let Some(account_id) = crate::runs::store::get(crate::db::scoped(&state.pool), run)
        .await?
        .map(|run| run.account_id)
    else {
        return Ok(None);
    };
    Ok(sqlx::query_as::<_, (Uuid, bool)>(
        r#"
        SELECT COALESCE(conversation.parent_channel_id, conversation.id),
               COALESCE(parent.visibility, conversation.visibility) <> 'project'
        FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1
          AND COALESCE(conversation.project_id, direct_scope_id()) = $2
          AND conversation_visible_to(conversation.id, $3)
        "#,
    )
    .bind(conversation)
    .bind(event.project_id)
    .bind(account_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?)
}

/// A notification from the bus (the wake listener calls this). Frames this
/// hub sent itself were routed when they were sent.
pub fn receive(hub: &RealtimeHub, payload: &str) {
    let Ok(envelope) = serde_json::from_str::<Envelope>(payload) else {
        tracing::warn!("ephemeral notification not understood");
        return;
    };
    if envelope.v != VERSION || envelope.from == Some(hub.origin()) {
        return;
    }
    for frame in &envelope.frames {
        route(hub, frame);
    }
}

fn route(hub: &RealtimeHub, frame: &EphemeralFrame) {
    hub.contract()
        .publish_local(super::contract::publish::of_ephemeral(frame));
    match frame {
        EphemeralFrame::StreamDelta {
            space,
            conversation,
            payload,
            ..
        } => hub.publish(RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "message.delta",
            project_id: *space,
            conversation_id: *conversation,
            payload: payload.clone(),
        }),
        EphemeralFrame::SessionRevoked { session } => hub.revoke_session(*session),
        EphemeralFrame::Invalidate {
            cache: CacheName::Modules,
            workspace,
        } => match workspace {
            Some(workspace) => crate::modules::gate::forget(*workspace),
            None => crate::modules::gate::forget_all(),
        },
        EphemeralFrame::Presence { .. }
        | EphemeralFrame::Typing { .. }
        | EphemeralFrame::Awareness { .. }
        | EphemeralFrame::DocumentUpdate { .. } => hub.live(frame.clone()),
    }
}

/// A delta too long for one notification becomes several, in order.
fn split(frame: EphemeralFrame) -> Vec<EphemeralFrame> {
    let EphemeralFrame::StreamDelta {
        space,
        conversation,
        payload,
        route,
    } = &frame
    else {
        return vec![frame];
    };
    let Some(delta) = payload.get("delta").and_then(serde_json::Value::as_str) else {
        return vec![frame];
    };
    if delta.len() <= MAX_DELTA_PIECE_BYTES {
        return vec![frame];
    }
    let mut pieces = Vec::new();
    let mut rest = delta;
    while !rest.is_empty() {
        let mut end = rest.len().min(MAX_DELTA_PIECE_BYTES);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (piece, tail) = rest.split_at(end);
        let mut payload = payload.clone();
        payload["delta"] = serde_json::Value::String(piece.to_owned());
        pieces.push(EphemeralFrame::StreamDelta {
            space: *space,
            conversation: *conversation,
            payload,
            route: *route,
        });
        rest = tail;
    }
    pieces
}

/// Frames packed into as few notifications as fit, in order. A frame that
/// can't fit even alone is dropped (and logged).
fn packed(from: Option<Uuid>, frames: Vec<EphemeralFrame>) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut batch: Vec<EphemeralFrame> = Vec::new();
    let encode = |frames: Vec<EphemeralFrame>| {
        serde_json::to_string(&Envelope {
            v: VERSION,
            from,
            frames,
        })
        .unwrap_or_default()
    };
    for frame in frames {
        let mut candidate = batch.clone();
        candidate.push(frame.clone());
        if encode(candidate.clone()).len() <= MAX_PAYLOAD_BYTES {
            batch = candidate;
            continue;
        }
        if !batch.is_empty() {
            payloads.push(encode(std::mem::take(&mut batch)));
        }
        if encode(vec![frame.clone()]).len() <= MAX_PAYLOAD_BYTES {
            batch.push(frame);
        } else {
            tracing::warn!("ephemeral frame too large for the bus; dropped");
        }
    }
    if !batch.is_empty() {
        payloads.push(encode(batch));
    }
    payloads
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn delta(text: &str) -> EphemeralFrame {
        EphemeralFrame::StreamDelta {
            space: Uuid::now_v7(),
            conversation: Some(Uuid::now_v7()),
            payload: json!({"message_id": Uuid::now_v7(), "agent_run_id": Uuid::now_v7(), "delta": text}),
            route: None,
        }
    }

    #[test]
    fn a_long_delta_is_split_in_order_on_character_boundaries() {
        let text = "é".repeat(10_000);
        let pieces = split(delta(&text));
        assert!(pieces.len() >= 4);
        let joined: String = pieces
            .iter()
            .map(|piece| match piece {
                EphemeralFrame::StreamDelta { payload, .. } => {
                    payload["delta"].as_str().expect("delta").to_owned()
                }
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(joined, text);
        for payload in packed(Some(Uuid::now_v7()), pieces) {
            assert!(payload.len() <= MAX_PAYLOAD_BYTES, "{}", payload.len());
        }
    }

    #[test]
    fn small_frames_share_a_notification() {
        let frames = (0..5)
            .map(|_| EphemeralFrame::SessionRevoked {
                session: Uuid::now_v7(),
            })
            .collect::<Vec<_>>();
        let payloads = packed(None, frames.clone());
        assert_eq!(payloads.len(), 1);
        let envelope: Envelope = serde_json::from_str(&payloads[0]).expect("envelope");
        assert_eq!(envelope.frames, frames);
    }

    #[test]
    fn a_hub_skips_its_own_frames() {
        let hub = RealtimeHub::new(8);
        let mut revoked = hub.subscribe_session_revocations();
        let session = Uuid::now_v7();
        let own = packed(
            Some(hub.origin()),
            vec![EphemeralFrame::SessionRevoked { session }],
        );
        receive(&hub, &own[0]);
        assert!(revoked.try_recv().is_err());
        let other = packed(
            Some(Uuid::now_v7()),
            vec![EphemeralFrame::SessionRevoked { session }],
        );
        receive(&hub, &other[0]);
        assert_eq!(revoked.try_recv().ok(), Some(session));
    }
}

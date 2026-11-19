//! Delivery (A2): telling accounts what happened, from the event log.
//!
//! An account is told through its connections (`connections`). What a
//! connection hears is read from the log with one function (`read`), which
//! re-checks access per event, so every transport agrees: a poll or
//! long-poll (`GET /api/v1/events`, or the `event_list` operation), a
//! stream (the same address with `Accept: text/event-stream`), or a webhook
//! push (`push`). One cursor (`cursor`) works for all of them and for every
//! workspace the connection reaches. What can be heard is the catalogue
//! (`catalogue`), and every event says why the account hears it
//! (`reason`). Envelopes carry ids, never content (`envelope`).

pub mod catalogue;
pub mod connections;
pub mod consent;
pub mod cursor;
pub mod envelope;
pub mod http;
pub mod ops;
pub mod push;
pub mod read;
pub mod reason;
pub mod sign;

use rmcp::schemars::{self, JsonSchema};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{AppState, access::Caller, error::AppError};

/// How long a long-poll may wait.
pub const MAX_WAIT: Duration = Duration::from_secs(30);
/// How often a waiting read looks at the log's head.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// One read for a caller.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// The events, oldest first, each in the envelope every transport uses.
    #[schemars(with = "Vec<serde_json::Map<String, Value>>")]
    pub events: Vec<Value>,
    /// Pass it back as `cursor` to read on from here.
    pub next_cursor: String,
    /// More are waiting: read again now.
    pub has_more: bool,
    /// The connection read through.
    #[schemars(with = "String")]
    pub connection_id: Uuid,
    /// Each event's place in the log, for the older feed's `sequence`.
    #[serde(skip)]
    #[schemars(skip)]
    pub positions: Vec<i64>,
}

/// A read of a caller's connection: after `cursor` (else where it last
/// read), up to `limit` events, waiting up to `wait` for the first. No
/// transaction is held while it waits.
pub async fn poll(
    state: &AppState,
    caller: &Caller,
    named: Option<Uuid>,
    cursor: Option<&str>,
    limit: i64,
    wait: Duration,
) -> Result<Page, AppError> {
    let wait = wait.min(MAX_WAIT);
    let (delivery, place) = crate::db::scoped_one(&state.pool, async |connection| {
        let delivery = connections::for_caller(connection, caller, named).await?;
        let kept_from = oldest(connection).await? - 1;
        let place = match cursor.map(str::trim).filter(|cursor| !cursor.is_empty()) {
            Some(cursor) => {
                let mut place = cursor::decode_place(&delivery.cursor_key, delivery.id, cursor)
                    .ok_or(AppError::Validation("cursor"))?;
                // A2.10: once the pauses that held events change (one is
                // resumed, even while another stays), the read goes back to
                // where they start: what is no longer held comes now, what
                // still is is held again, and what passed meanwhile comes
                // again once.
                if let Some(held) = place.held
                    && cursor::paused_fingerprint(
                        &crate::statuses::paused_in(connection, delivery.account_id).await?,
                    ) != held.paused
                {
                    place = cursor::Place {
                        after: held.at,
                        held: None,
                    };
                }
                let after = place.after;
                if after < kept_from {
                    return Err(AppError::Conflict(
                        "cursor_expired",
                        "This cursor is older than the events kept. Call briefing to catch up, then read without a cursor.",
                    ));
                }
                connections::mark_read(
                    connection,
                    delivery.id,
                    place.held.map_or(after, |held| held.at),
                )
                    .await?;
                place
            }
            // Where it last read, unless those events are gone: then from
            // the oldest kept, which is where a read without a cursor
            // recovers after `cursor_expired`.
            None if delivery.read_cursor < kept_from => {
                connections::mark_read(connection, delivery.id, kept_from).await?;
                cursor::Place {
                    after: kept_from,
                    held: None,
                }
            }
            None => cursor::Place {
                after: delivery.read_cursor,
                held: None,
            },
        };
        Ok::<_, AppError>((delivery, place))
    })
    .await?;
    let deadline = tokio::time::Instant::now() + wait;
    let mut seen_head = None;
    loop {
        let found = crate::db::scoped_one(&state.pool, async |connection| {
            let scope = read::Scope {
                recipient: delivery.account_id,
                workspaces: connections::reach(connection, &delivery).await?,
                space: caller.confined_space,
                limits: caller.limits.clone(),
            };
            let subscriptions = connections::subscriptions(connection, &delivery).await?;
            let batch =
                read::read_batch(connection, &subscriptions, &scope, place.after, limit).await?;
            let lookups = envelope::Lookups::for_batch(connection, &batch.delivered).await?;
            let head = crate::events::head(connection).await?;
            Ok::<_, AppError>((batch, lookups, head))
        })
        .await?;
        let (batch, lookups, head) = found;
        let now = tokio::time::Instant::now();
        if !batch.delivered.is_empty() || batch.has_more || now >= deadline {
            let found = batch.held.map(|at| cursor::Held {
                at,
                paused: batch.paused,
            });
            // Each event's own cursor keeps what was held before it.
            let held_before =
                |position: i64| place.held.or(found.filter(|held| held.at < position));
            return Ok(Page {
                events: batch
                    .delivered
                    .iter()
                    .map(|item| {
                        envelope::envelope(
                            item,
                            &delivery,
                            &lookups,
                            held_before(item.event.position),
                        )
                    })
                    .collect(),
                next_cursor: cursor::encode_place(
                    &delivery.cursor_key,
                    delivery.id,
                    cursor::Place {
                        after: batch.next,
                        held: place.held.or(found),
                    },
                ),
                has_more: batch.has_more,
                connection_id: delivery.id,
                positions: batch
                    .delivered
                    .iter()
                    .map(|item| item.event.position)
                    .collect(),
            });
        }
        // Nothing yet: wait for the log to move, holding no transaction.
        seen_head.get_or_insert(head);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            tokio::time::sleep(LOOK_EVERY.min(left)).await;
            let moved = crate::db::scoped_one(&state.pool, async |connection| {
                crate::events::head(connection).await
            })
            .await?;
            if Some(moved) != seen_head {
                seen_head = Some(moved);
                break;
            }
        }
    }
}

/// The oldest position the log still keeps (1 when nothing was dropped).
async fn oldest(connection: &mut sqlx::PgConnection) -> Result<i64, AppError> {
    Ok(
        sqlx::query_scalar::<_, Option<i64>>("SELECT min(position) FROM events")
            .fetch_one(&mut *connection)
            .await?
            .unwrap_or(1),
    )
}

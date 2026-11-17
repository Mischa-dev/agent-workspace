//! `GET /api/v1/events` (A2 3.5): poll, long-poll and stream on one
//! address, and `GET /api/v1/events/catalogue`.
//!
//! JSON by default: `{"events":[…],"nextCursor":"…","hasMore":false}`, with
//! `cursor`, `limit` (at most 100), `wait` (seconds, at most 30) and
//! `connection` (one of the caller's connections; else its default). With
//! `Accept: text/event-stream`, a stream: one SSE event per delivered event
//! (`id` is its cursor, `event` its type), resumed from `Last-Event-ID` or
//! `cursor`. A stream lasts five minutes and the client reconnects with
//! `Last-Event-ID`, which also re-checks its credential; keep-alives come
//! every 15 seconds.

use std::{convert::Infallible, time::Duration};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, header::ACCEPT},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::{Stream, stream};
use serde::Deserialize;
use uuid::Uuid;

use crate::{AppState, access::Caller, error::AppError};

/// How long one stream stays open before the client reconnects.
const STREAM_FOR: Duration = Duration::from_secs(300);
/// How long a stream waits for events per read.
const STREAM_WAIT: Duration = Duration::from_secs(15);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/events", get(events_route))
        .route("/api/v1/events/catalogue", get(catalogue_route))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventsQuery {
    cursor: Option<String>,
    limit: Option<i64>,
    wait: Option<u64>,
    connection: Option<Uuid>,
}

async fn events_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Response {
    let caller = match crate::ops::web::caller(&state, &headers, false).await {
        Ok((caller, _)) => caller,
        Err(response) => return response,
    };
    if !caller.can_read {
        return AppError::Forbidden.into_response();
    }
    let streamed = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"));
    let resume = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cursor = resume.or(query.cursor);
    let limit = query.limit.unwrap_or(super::read::MAX_LIMIT);
    if streamed {
        // Check the cursor and connection now, so a bad one is a 4xx and
        // not a stream that ends at once.
        if let Err(error) = super::poll(
            &state,
            &caller,
            query.connection,
            cursor.as_deref(),
            1,
            Duration::ZERO,
        )
        .await
        {
            return error.into_response();
        }
        return Sse::new(stream_events(state, caller, query.connection, cursor))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response();
    }
    let wait = Duration::from_secs(query.wait.unwrap_or(0));
    match super::poll(
        &state,
        &caller,
        query.connection,
        cursor.as_deref(),
        limit,
        wait,
    )
    .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => error.into_response(),
    }
}

fn stream_events(
    state: AppState,
    caller: Caller,
    connection: Option<Uuid>,
    cursor: Option<String>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let until = tokio::time::Instant::now() + STREAM_FOR;
    let opening = Event::default()
        .retry(Duration::from_secs(5))
        .comment("stream open");
    stream::unfold((cursor, vec![opening]), move |(mut cursor, mut queued)| {
        let state = state.clone();
        let caller = caller.clone();
        async move {
            loop {
                if let Some(event) = queued.pop() {
                    return Some((Ok(event), (cursor, queued)));
                }
                if tokio::time::Instant::now() >= until {
                    return None;
                }
                let page = super::poll(
                    &state,
                    &caller,
                    connection,
                    cursor.as_deref(),
                    super::read::MAX_LIMIT,
                    STREAM_WAIT,
                )
                .await
                .ok()?;
                cursor = Some(page.next_cursor);
                // Popped from the end, so queued newest first.
                queued = page
                    .events
                    .iter()
                    .rev()
                    .filter_map(|event| {
                        Event::default()
                            .id(event["cursor"].as_str().unwrap_or_default())
                            .event(event["type"].as_str().unwrap_or("event"))
                            .json_data(event)
                            .ok()
                    })
                    .collect();
            }
        }
    })
}

async fn catalogue_route() -> impl IntoResponse {
    Json(serde_json::json!({ "events": super::catalogue::ENTRIES }))
}

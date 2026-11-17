//! The live side of the contract, in process (K5.19): connections that
//! opened with a ticket hear the frames published to any audience their
//! ticket names. Nothing here reads the database: a ticket says what its
//! connection may hear, and the database side publishes every frame with
//! the audiences it is for.
//!
//! The router keeps the last few thousand durable frames, so a connection
//! whose ticket starts a little behind the newest frame catches up without
//! a gap; further behind, it is told to resume through `api`. That window
//! is the only thing it keeps.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::{
        State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;
use uuid::Uuid;

use super::ticket::{self, Audience, KeyringKeys, PublicKeys, Ticket};
use crate::AppState;

/// Durable frames the router keeps for connections that start a little
/// behind.
const WINDOW: usize = 2_048;
/// How long a connection has to say `hello` with its ticket.
const HELLO_WAIT: Duration = Duration::from_secs(5);
/// How long a connection told its ticket is stale has to send a new one.
pub const STALE_GRACE: Duration = Duration::from_secs(15);
/// The largest frame a client sends (a `hello` or `ticket` carries one).
const MAX_CLIENT_FRAME_BYTES: usize = 64 * 1024;
/// A publish older than this is refused (replays).
const PUBLISH_MAX_AGE_SECONDS: i64 = 60;
/// The header a publish's signature comes in.
pub const SIGNATURE_HEADER: &str = "x-live-signature";

/// One published item: a frame for some audiences, or a control message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Publish {
    /// `frame` (a JSON text, sent as it is) to every connection in any of
    /// `audiences`. A durable frame has its log position and goes out in
    /// position order, once.
    Frame {
        audiences: Vec<Audience>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "optional_position"
        )]
        position: Option<i64>,
        /// Not to this account's own connections (its own typing).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        except_account: Option<Uuid>,
        frame: String,
    },
    /// What these audiences may hear changed: their connections fetch a
    /// new ticket.
    #[serde(rename = "ticket.stale")]
    Stale { audiences: Vec<Audience> },
    /// A signed-out session: its connections close.
    Revoke { session: Uuid },
}

mod optional_position {
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(clippy::ref_option)]
    pub fn serialize<S: Serializer>(value: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(position) => serializer.collect_str(position),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<i64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| text.parse().map_err(serde::de::Error::custom))
            .transpose()
    }
}

/// A publish request's body.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishBody {
    /// Unix seconds when it was sent.
    pub sent_at: i64,
    pub items: Vec<Publish>,
}

/// Recent durable frames, by position.
type Window = VecDeque<(i64, Arc<Publish>)>;

#[derive(Clone, Debug)]
enum Delivery {
    Item(Arc<Publish>),
    /// This instance is stopping: reconnect elsewhere.
    Drain,
}

/// The in-process live role's routing: every connection hears every
/// delivery and keeps those for its audiences.
#[derive(Clone)]
pub struct LiveRouter {
    sender: broadcast::Sender<Delivery>,
    recent: Arc<Mutex<Window>>,
    /// The position this router has every durable frame after (until the
    /// next is published); `i64::MAX` until it knows.
    start: Arc<AtomicI64>,
    /// Fed by publishes from the database side (an outside publisher, with
    /// `AGENT_WORKSPACE_LIVE_URL`), not by this instance's own tail.
    external: bool,
    draining: Arc<AtomicBool>,
}

impl LiveRouter {
    pub fn new(capacity: usize, external: bool) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            recent: Arc::new(Mutex::new(VecDeque::with_capacity(WINDOW))),
            start: Arc::new(AtomicI64::new(i64::MAX)),
            external,
            draining: Arc::new(AtomicBool::new(false)),
        }
    }

    /// This instance's feed starts after `position` (its tail's start).
    pub fn begin(&self, position: i64) {
        let _ = self
            .start
            .compare_exchange(i64::MAX, position, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Whether frames come from publishes rather than this instance.
    pub fn external(&self) -> bool {
        self.external
    }

    /// Route one item to this instance's connections.
    pub fn publish(&self, item: Publish) {
        let item = Arc::new(item);
        if let Publish::Frame {
            position: Some(position),
            ..
        } = item.as_ref()
        {
            let mut recent = self
                .recent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if recent.back().is_some_and(|(last, _)| last >= position) {
                // Already routed (a publish sent twice).
                return;
            }
            // The first frame an outside publisher sends starts what this
            // router knows.
            self.begin(position - 1);
            if recent.len() == WINDOW
                && let Some((dropped, _)) = recent.pop_front()
            {
                self.start.store(dropped, Ordering::SeqCst);
            }
            recent.push_back((*position, Arc::clone(&item)));
        }
        let _ = self.sender.send(Delivery::Item(item));
    }

    /// Route what this instance's own feeds produce, unless the frames come
    /// from an outside publisher instead.
    pub fn publish_local(&self, items: impl IntoIterator<Item = Publish>) {
        if self.external {
            return;
        }
        for item in items {
            self.publish(item);
        }
    }

    /// Every connection is told to reconnect elsewhere; none opens.
    pub fn drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
        let _ = self.sender.send(Delivery::Drain);
    }

    /// The durable frames after `from`, and whether the window reaches
    /// back to it (None when it doesn't: resume through `api`).
    fn after(&self, from: i64) -> Option<Vec<Arc<Publish>>> {
        let recent = self
            .recent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if from < self.start.load(Ordering::SeqCst) && self.start.load(Ordering::SeqCst) != i64::MAX
        {
            return None;
        }
        Some(
            recent
                .iter()
                .filter(|(position, _)| *position > from)
                .map(|(_, item)| Arc::clone(item))
                .collect(),
        )
    }
}

/// The live side's routes, on any instance with `live`.
pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/live/v1/socket", axum::routing::get(socket))
        .route("/live/v1/publish", axum::routing::post(publish))
}

/// `GET /live/v1/socket`: a connection that says `hello` with its ticket.
pub async fn socket(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    let router = state.realtime.contract().clone();
    let receiver = router.sender.subscribe();
    if router.draining.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    upgrade
        .max_frame_size(MAX_CLIENT_FRAME_BYTES)
        .max_message_size(MAX_CLIENT_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            let keys = KeyringKeys(&state.config);
            serve(socket, &router, receiver, &keys).await;
        })
        .into_response()
}

/// Why a connection closes, as its close code: 4401 means fetch a new
/// ticket first.
const CLOSE_TICKET: u16 = 4401;
const CLOSE_RESTART: u16 = 1012;

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

async fn send(socket: &mut WebSocket, text: String) -> bool {
    socket.send(Message::Text(text.into())).await.is_ok()
}

fn hears(ticket: &Ticket, audiences: &[Audience]) -> bool {
    audiences
        .iter()
        .any(|audience| ticket.audiences.contains(audience))
}

/// A client frame naming a ticket (`hello` or `ticket`).
fn ticket_in(text: &str, kind: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    (value.get("type")?.as_str()? == kind)
        .then(|| value.get("ticket")?.as_str().map(str::to_owned))
        .flatten()
}

async fn serve(
    mut socket: WebSocket,
    router: &LiveRouter,
    mut receiver: broadcast::Receiver<Delivery>,
    keys: &(dyn PublicKeys + Sync),
) {
    let now = || chrono::Utc::now().timestamp();
    let hello = match tokio::time::timeout(HELLO_WAIT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => ticket_in(&text, "hello"),
        _ => None,
    };
    let Some(mut ticket) = hello.and_then(|token| ticket::verify(&token, keys, now())) else {
        let _ = send(
            &mut socket,
            json!({"type": "error", "reason": "ticket"}).to_string(),
        )
        .await;
        close(&mut socket, CLOSE_TICKET, "ticket").await;
        return;
    };
    let mut last = ticket.from;
    // What it missed between its ticket and now, from the window.
    let Some(missed) = router.after(last) else {
        let frame =
            json!({"type": "resync", "reason": "resume_through_api", "position": last.to_string()});
        let _ = send(&mut socket, frame.to_string()).await;
        close(&mut socket, CLOSE_RESTART, "resume_through_api").await;
        return;
    };
    for item in missed {
        if let Publish::Frame {
            audiences,
            position: Some(position),
            except_account,
            frame,
        } = item.as_ref()
        {
            if *position <= last {
                continue;
            }
            last = *position;
            if hears(&ticket, audiences)
                && *except_account != Some(ticket.account)
                && !send(&mut socket, frame.clone()).await
            {
                return;
            }
        }
    }
    let ready = json!({"type": "ready", "contract": "live.1", "position": last.to_string()});
    if !send(&mut socket, ready.to_string()).await {
        return;
    }
    let mut stale_until: Option<tokio::time::Instant> = None;
    loop {
        let expires = tokio::time::Instant::now()
            + Duration::from_secs(u64::try_from(ticket.expires - now()).unwrap_or(0));
        let deadline = stale_until.map_or(expires, |stale| stale.min(expires));
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let Some(token) = ticket_in(&text, "ticket") else {
                        continue;
                    };
                    match ticket::verify(&token, keys, now()) {
                        Some(fresh)
                            if fresh.account == ticket.account && fresh.session == ticket.session =>
                        {
                            last = last.max(fresh.from);
                            ticket = fresh;
                            stale_until = None;
                            let accepted = json!({"type": "ticket.accepted", "position": last.to_string()});
                            if !send(&mut socket, accepted.to_string()).await {
                                return;
                            }
                        }
                        _ => {
                            close(&mut socket, CLOSE_TICKET, "ticket").await;
                            return;
                        }
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if socket.send(Message::Pong(payload)).await.is_err() {
                        return;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
            delivery = receiver.recv() => match delivery {
                Ok(Delivery::Item(item)) => match item.as_ref() {
                    Publish::Frame { audiences, position, except_account, frame } => {
                        if let Some(position) = position {
                            if *position <= last {
                                continue;
                            }
                            last = *position;
                        }
                        if hears(&ticket, audiences)
                            && *except_account != Some(ticket.account)
                            && !send(&mut socket, frame.clone()).await
                        {
                            return;
                        }
                    }
                    Publish::Stale { audiences } => {
                        if hears(&ticket, audiences) && stale_until.is_none() {
                            stale_until = Some(tokio::time::Instant::now() + STALE_GRACE);
                            if !send(&mut socket, json!({"type": "ticket.stale"}).to_string()).await {
                                return;
                            }
                        }
                    }
                    Publish::Revoke { session } => {
                        if *session == ticket.session {
                            close(&mut socket, CLOSE_TICKET, "signed_out").await;
                            return;
                        }
                    }
                },
                Ok(Delivery::Drain) => {
                    let frame = json!({"type": "reconnect", "afterMs": 1_000, "reason": "server_draining"});
                    let _ = send(&mut socket, frame.to_string()).await;
                    close(&mut socket, CLOSE_RESTART, "server_draining").await;
                    return;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let frame = json!({"type": "resync", "reason": "too_far_behind", "position": last.to_string()});
                    let _ = send(&mut socket, frame.to_string()).await;
                    close(&mut socket, CLOSE_RESTART, "too_far_behind").await;
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            () = tokio::time::sleep_until(deadline) => {
                let _ = send(&mut socket, json!({"type": "ticket.expired"}).to_string()).await;
                close(&mut socket, CLOSE_TICKET, "ticket").await;
                return;
            }
        }
    }
}

/// `POST /live/v1/publish`: frames from an outside publisher, signed with
/// the ticket key. Refused unless this instance's frames come from one.
pub async fn publish(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let router = state.realtime.contract();
    if !router.external() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let signed = headers
        .get(SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|signature| {
            ticket::verify_body(signature, &body, &KeyringKeys(&state.config))
        });
    if !signed {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(body) = serde_json::from_slice::<PublishBody>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if (chrono::Utc::now().timestamp() - body.sent_at).abs() > PUBLISH_MAX_AGE_SECONDS {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    for item in body.items {
        router.publish(item);
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(position: i64) -> Publish {
        Publish::Frame {
            audiences: vec![Audience::Workspace(Uuid::nil())],
            position: Some(position),
            except_account: None,
            frame: format!("{{\"position\":{position}}}"),
        }
    }

    #[test]
    fn the_window_catches_up_or_says_resume() {
        let router = LiveRouter::new(16, false);
        assert_eq!(router.after(0).map(|items| items.len()), Some(0));
        router.begin(0);
        for position in 1..=3 {
            router.publish(frame(position));
        }
        // Published twice: kept once.
        router.publish(frame(3));
        assert_eq!(router.after(1).map(|items| items.len()), Some(2));
        for position in 4..=(WINDOW as i64 + 10) {
            router.publish(frame(position));
        }
        assert!(router.after(1).is_none(), "fell out of the window");
        assert!(router.after(WINDOW as i64).is_some());
        // A router that started after the ticket's position can't vouch for
        // what came before it.
        let late = LiveRouter::new(16, false);
        late.begin(50);
        assert!(late.after(49).is_none());
        assert_eq!(late.after(50).map(|items| items.len()), Some(0));
    }

    #[test]
    fn publishes_read_as_written() {
        let items = vec![
            frame(9_007_199_254_740_993),
            Publish::Stale {
                audiences: vec![Audience::Account(Uuid::now_v7())],
            },
            Publish::Revoke {
                session: Uuid::now_v7(),
            },
        ];
        let text = serde_json::to_string(&PublishBody {
            sent_at: 1,
            items: items.clone(),
        })
        .expect("body");
        assert!(
            text.contains("\"9007199254740993\""),
            "positions are strings"
        );
        assert!(text.contains("\"type\":\"ticket.stale\""));
        let read: PublishBody = serde_json::from_str(&text).expect("read");
        assert_eq!(read.items, items);
    }
}

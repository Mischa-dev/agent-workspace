use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket, close_code},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use sqlx::FromRow;
use tokio::{
    sync::broadcast,
    time::{MissedTickBehavior, interval},
};
use uuid::Uuid;

use super::{
    subscriptions::{self, Hearing, Route, RouteAudience, SubscriptionSet},
    wire::{self, WireQuery},
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, load_authenticated_actor, origin_is_allowed},
    error::AppError,
    outbox::{OUTBOX_SCHEMA_VERSION, OutboxEnvelope, OutboxPublisher},
    policy::{Action, MembershipRole, RoleGrant},
};

const SESSION_REVALIDATION_INTERVAL: Duration = Duration::from_secs(15);
const REALTIME_RESYNC_CLOSE_REASON: &str = "realtime lag; reconnect and refetch";
/// Why a draining instance closes its sockets: reconnect (to another
/// instance) and refetch.
pub const SERVER_DRAINING_CLOSE_REASON: &str = "server_draining";
const MAX_REALTIME_FRAME_BYTES: usize = 70_000;
const MAX_REALTIME_CLIENT_FRAME_BYTES: usize = 4_096;
const MAX_EPHEMERAL_PAYLOAD_BYTES: usize = 65_536;

/// Compatibility event for the M0/M1 provider stream.
///
/// Durable M2 state changes use [`OutboxEnvelope`]. Only bounded streaming
/// `message.delta` events are accepted through this compatibility path.

#[derive(Clone, Debug, Serialize)]
pub struct RealtimeEvent {
    pub event_id: Uuid,
    pub kind: &'static str,
    pub project_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug)]
pub(super) enum RealtimeDelivery {
    Durable {
        envelope: OutboxEnvelope,
        /// Who hears it, from the log's audience (K5.8).
        route: Route,
    },
    Ephemeral(RealtimeEvent),
    /// What this account may hear changed (K5.8): its connections work
    /// out their sets again before routing anything after it.
    AccessChanged(Uuid),
    /// Presence, typing, awareness and live document changes (K5.10),
    /// for protocol 2 connections only.
    Live(Box<super::ephemeral::EphemeralFrame>),
    /// Every socket closes with 1012 and this reason, to reconnect and
    /// refetch: the tail skipped ahead (K5.5), or this instance is
    /// draining (K5.7).
    Restart(&'static str),
}

impl RealtimeDelivery {
    fn event_id(&self) -> Uuid {
        match self {
            Self::Durable { envelope, .. } => envelope.event_id,
            Self::Ephemeral(event) => event.event_id,
            Self::Restart(_) | Self::AccessChanged(_) | Self::Live(_) => Uuid::nil(),
        }
    }

    fn project_id(&self) -> Uuid {
        match self {
            Self::Durable { envelope, .. } => envelope.project_id,
            Self::Ephemeral(event) => event.project_id,
            Self::Restart(_) | Self::AccessChanged(_) | Self::Live(_) => Uuid::nil(),
        }
    }

    #[cfg(test)]
    fn conversation_id(&self) -> Option<Uuid> {
        match self {
            Self::Durable { envelope, .. } => envelope.conversation_id,
            Self::Ephemeral(event) => event.conversation_id,
            Self::Restart(_) | Self::AccessChanged(_) | Self::Live(_) => None,
        }
    }

    fn recipient_actor_id(&self) -> Option<Uuid> {
        match self {
            Self::Durable { envelope, .. } => envelope.recipient_actor_id,
            Self::Ephemeral(_) | Self::Restart(_) | Self::AccessChanged(_) | Self::Live(_) => None,
        }
    }

    fn is_visible_to_actor(&self, actor_id: Uuid) -> bool {
        self.recipient_actor_id()
            .is_none_or(|recipient_actor_id| recipient_actor_id == actor_id)
    }

    fn to_wire_json(&self) -> Result<String, RealtimePublishError> {
        let json = match self {
            Self::Durable { envelope, .. } => serde_json::to_string(envelope),
            Self::Ephemeral(event) => serde_json::to_string(event),
            Self::Restart(_) | Self::AccessChanged(_) | Self::Live(_) => {
                return Err(RealtimePublishError::Serialization);
            }
        }
        .map_err(|_| RealtimePublishError::Serialization)?;
        if json.len() > MAX_REALTIME_FRAME_BYTES {
            return Err(RealtimePublishError::FrameTooLarge);
        }
        Ok(json)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RealtimePublishError {
    #[error("realtime event metadata is invalid")]
    InvalidMetadata,
    #[error("realtime event payload exceeds its bound")]
    FrameTooLarge,
    #[error("realtime event serialization failed")]
    Serialization,
}

/// This instance's sockets. Durable events reach it from the instance's
/// tail (`deliver_logged`, K5.5), ephemeral ones from this instance or, over
/// the bus, from the others (`realtime::ephemeral`).
#[derive(Clone)]
pub struct RealtimeHub {
    sender: broadcast::Sender<RealtimeDelivery>,
    session_revocations: broadcast::Sender<Uuid>,
    /// Who this hub is on the ephemeral bus, so it skips its own frames.
    origin: Uuid,
    /// Set when this instance drains: no new sockets.
    draining: Arc<AtomicBool>,
    /// The live contract's connections here (K5.19).
    contract: super::contract::LiveRouter,
}

impl RealtimeHub {
    pub fn new(capacity: usize) -> Self {
        Self::with_contract(capacity, false)
    }

    /// A hub whose contract connections hear an outside publisher
    /// (`AGENT_WORKSPACE_LIVE_URL`) rather than this instance's own feeds.
    pub fn with_contract(capacity: usize, external: bool) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let (session_revocations, _) = broadcast::channel(capacity);
        Self {
            sender,
            session_revocations,
            origin: Uuid::now_v7(),
            draining: Arc::new(AtomicBool::new(false)),
            contract: super::contract::LiveRouter::new(capacity.max(1_024), external),
        }
    }

    /// The live contract's connections on this instance.
    pub fn contract(&self) -> &super::contract::LiveRouter {
        &self.contract
    }

    pub fn origin(&self) -> Uuid {
        self.origin
    }

    /// A logged event, from this instance's tail, to its sockets as protocol
    /// 1's frame. Events that were never outbox events have no frame yet.
    pub fn deliver_logged(&self, event: &crate::events::LoggedEvent) {
        self.contract
            .publish_local(super::contract::publish::of_logged(event));
        if let Some(account) = access_changed(event) {
            let _ = self.sender.send(RealtimeDelivery::AccessChanged(account));
            return;
        }
        let Some(envelope) = OutboxEnvelope::from_logged(event) else {
            return;
        };
        let Some(route) = Route::of_logged(event) else {
            tracing::warn!(
                event_id = %envelope.event_id,
                event_position = envelope.event_cursor,
                "logged event has no complete audience"
            );
            return;
        };
        if let Err(error) = self.publish_routed(&envelope, route) {
            tracing::warn!(
                event_id = %envelope.event_id,
                event_position = envelope.event_cursor,
                error_category = %error,
                "logged event has no valid realtime frame"
            );
        }
    }

    /// The tail skipped ahead: every socket here closes to refetch.
    pub fn resync(&self) {
        let _ = self
            .sender
            .send(RealtimeDelivery::Restart(REALTIME_RESYNC_CLOSE_REASON));
    }

    /// This instance is stopping (K5.7): every socket here closes with 1012
    /// so its browser reconnects to another instance, and no new one opens.
    pub fn drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
        self.contract.drain();
        let _ = self
            .sender
            .send(RealtimeDelivery::Restart(SERVER_DRAINING_CLOSE_REASON));
    }

    /// A `message.delta` to this instance's sockets only; use
    /// `realtime::ephemeral::publish` to reach every instance's.
    pub fn publish(&self, event: RealtimeEvent) {
        if !ephemeral_event_is_valid(&event) {
            tracing::warn!(
                event_id = %event.event_id,
                event_kind = event.kind,
                "dropped invalid ephemeral realtime event"
            );
            return;
        }
        let _ = self.sender.send(RealtimeDelivery::Ephemeral(event));
    }

    /// Presence, typing, awareness or a document change to this instance's
    /// protocol 2 sockets; use `realtime::ephemeral::publish` to reach
    /// every instance's.
    pub(crate) fn live(&self, frame: super::ephemeral::EphemeralFrame) {
        let _ = self.sender.send(RealtimeDelivery::Live(Box::new(frame)));
    }

    /// Closes this instance's sockets for the session; use
    /// `realtime::ephemeral::revoke_session` to close every instance's.
    pub fn revoke_session(&self, session_id: Uuid) {
        let _ = self.session_revocations.send(session_id);
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<RealtimeDelivery> {
        self.sender.subscribe()
    }

    pub(crate) fn subscribe_session_revocations(&self) -> broadcast::Receiver<Uuid> {
        self.session_revocations.subscribe()
    }
}

impl RealtimeHub {
    fn publish_routed(
        &self,
        envelope: &OutboxEnvelope,
        route: Route,
    ) -> Result<(), RealtimePublishError> {
        let delivery = checked_delivery(envelope, route)?;
        // Validate and bound the exact frame before making it observable.
        delivery.to_wire_json()?;
        // No connected browser is still a successful broadcast operation:
        // reconnect correctness comes from HTTP refetch, not socket replay.
        let _ = self.sender.send(delivery);
        Ok(())
    }
}

impl OutboxPublisher for RealtimeHub {
    type Error = RealtimePublishError;

    /// An envelope with no log row behind it (tests): routed as narrowly as
    /// its fields allow, a conversation as restricted.
    fn publish(&self, envelope: &OutboxEnvelope) -> Result<(), Self::Error> {
        self.publish_routed(envelope, envelope_route(envelope))
    }
}

/// An envelope's route from its own fields alone, a conversation as
/// restricted (fail closed).
fn envelope_route(envelope: &OutboxEnvelope) -> Route {
    let audience = match (envelope.recipient_actor_id, envelope.conversation_id) {
        (Some(account), _) => RouteAudience::Account(account),
        (None, Some(root)) => RouteAudience::Conversation {
            root,
            restricted: true,
        },
        (None, None) => RouteAudience::Space,
    };
    Route {
        workspace: Uuid::nil(),
        space: Some(envelope.project_id),
        audience,
        event_type: envelope.event_type.clone(),
        about_account: None,
    }
}

pub(super) fn checked_delivery(
    envelope: &OutboxEnvelope,
    route: Route,
) -> Result<RealtimeDelivery, RealtimePublishError> {
    {
        let payload_conversation_id = envelope
            .payload
            .get("conversationId")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .filter(|value| !value.is_nil());
        let payload_actor_id = envelope
            .payload
            .get("actorId")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .filter(|value| !value.is_nil());
        // Actor-targeted events reach only the actor named in their payload.
        let actor_targeted_event = matches!(
            envelope.event_type.as_str(),
            "conversation.read_state.updated"
                | "conversation.member.removed"
                | "notifications.updated"
                | "account.managers.changed"
                | "message_draft.changed"
                | "notification_keywords.changed"
                | "sidebar.changed"
                | "saved_searches.changed"
                | "follows.changed"
                | "inbox_views.changed"
                | "dm_rules.changed"
        );
        let invalid_actor_scope = if actor_targeted_event {
            payload_actor_id.is_none() || payload_actor_id != envelope.recipient_actor_id
        } else {
            envelope.recipient_actor_id.is_some()
        };
        let supported_event_type = matches!(
            envelope.event_type.as_str(),
            "conversation.created"
                | "conversation.updated"
                | "conversation.reply_requests.updated"
                | "conversation.archived"
                | "conversation.message.created"
                | "conversation.message.updated"
                | "conversation.message.reactions.updated"
                | "conversation.read_state.updated"
                | "conversation.member.removed"
                | "conversation.member.added"
                | "conversation.agent_run.updated"
                | "work_item.updated"
                | "project.membership.updated"
                | "notifications.updated"
                | "account.managers.changed"
                | "account.status.changed"
                | "account.profile_changed"
                | "whiteboard.updated"
                | "page.updated"
                | "page.discussion.updated"
                | "collection.updated"
                | "sheet.updated"
                | "dashboard.updated"
                | "project.modules.updated"
                | "custom_emoji.changed"
                | "guards.changed"
                | "message_draft.changed"
                | "notification_keywords.changed"
                | "sidebar.changed"
                | "saved_searches.changed"
                | "follows.changed"
                | "inbox_views.changed"
                | "dm_rules.changed"
                | "workspace.lifecycle.changed"
        );
        let project_scoped_event = matches!(
            envelope.event_type.as_str(),
            "work_item.updated"
                | "project.membership.updated"
                | "notifications.updated"
                | "account.managers.changed"
                | "account.status.changed"
                | "account.profile_changed"
                | "whiteboard.updated"
                | "page.updated"
                | "page.discussion.updated"
                | "collection.updated"
                | "sheet.updated"
                | "dashboard.updated"
                | "project.modules.updated"
                | "custom_emoji.changed"
                | "guards.changed"
                | "message_draft.changed"
                | "notification_keywords.changed"
                | "sidebar.changed"
                | "saved_searches.changed"
                | "follows.changed"
                | "inbox_views.changed"
                | "dm_rules.changed"
                | "workspace.lifecycle.changed"
        );
        let invalid_conversation_scope = if project_scoped_event {
            envelope.conversation_id.is_some() || envelope.payload.get("conversationId").is_some()
        } else if envelope.event_type == "conversation.member.removed" {
            // The removed actor can no longer see the group, so this event
            // is never conversation-scoped; it still names the group.
            envelope.conversation_id.is_some() || payload_conversation_id.is_none()
        } else {
            envelope.conversation_id.is_none()
                || payload_conversation_id != envelope.conversation_id
        };
        let invalid_membership_project_scope = envelope.event_type == "project.membership.updated"
            && envelope
                .payload
                .get("projectId")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                != Some(envelope.project_id);
        if envelope.event_id.is_nil()
            || envelope.event_cursor < 1
            || envelope.version != OUTBOX_SCHEMA_VERSION
            || envelope.project_id.is_nil()
            || envelope.conversation_id.is_some_and(|id| id.is_nil())
            || invalid_conversation_scope
            || invalid_membership_project_scope
            || (envelope.event_type == "whiteboard.updated"
                && envelope
                    .payload
                    .get("whiteboardId")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .is_none_or(|id| id.is_nil()))
            || (matches!(
                envelope.event_type.as_str(),
                "page.updated" | "page.discussion.updated"
            ) && envelope
                .payload
                .get("pageId")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .is_none_or(|id| id.is_nil()))
            || (envelope.event_type == "collection.updated"
                && envelope
                    .payload
                    .get("collectionId")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .is_none_or(|id| id.is_nil()))
            || (envelope.event_type == "sheet.updated"
                && envelope
                    .payload
                    .get("sheetId")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .is_none_or(|id| id.is_nil()))
            || envelope
                .recipient_actor_id
                .is_some_and(|actor_id| actor_id.is_nil())
            || invalid_actor_scope
            || !supported_event_type
            || envelope.event_type.is_empty()
            || envelope.event_type.len() > 120
            || envelope.occurred_at.is_empty()
            || envelope.occurred_at.len() > 40
        {
            return Err(RealtimePublishError::InvalidMetadata);
        }
        Ok(RealtimeDelivery::Durable {
            envelope: envelope.clone(),
            route,
        })
    }
}

fn ephemeral_event_is_valid(event: &RealtimeEvent) -> bool {
    if event.event_id.is_nil()
        || event.project_id.is_nil()
        || event.conversation_id.is_some_and(|id| id.is_nil())
        || event.kind != "message.delta"
    {
        return false;
    }
    let payload_is_bounded = serde_json::to_vec(&event.payload)
        .map(|payload| payload.len() <= MAX_EPHEMERAL_PAYLOAD_BYTES)
        .unwrap_or(false);
    payload_is_bounded
        && serde_json::to_vec(event)
            .map(|frame| frame.len() <= MAX_REALTIME_FRAME_BYTES)
            .unwrap_or(false)
}

pub async fn websocket(
    State(state): State<AppState>,
    Query(wire_query): Query<WireQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let protocol = wire_query.protocol();
    if !origin_is_allowed(&state.config, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let principal = match load_authenticated_actor(&state, &headers).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(error) => return error.into_response(),
    };
    let receiver = state.realtime.subscribe();
    let session_revocations = state.realtime.subscribe_session_revocations();
    // Checked after subscribing: a drain that starts from here on reaches
    // this socket's receiver, so no socket opens past it (K5.7).
    if state.realtime.draining.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    upgrade
        .max_frame_size(MAX_REALTIME_CLIENT_FRAME_BYTES)
        .max_message_size(MAX_REALTIME_CLIENT_FRAME_BYTES)
        .on_upgrade(move |socket| {
            serve_socket(
                socket,
                state,
                headers,
                principal,
                receiver,
                session_revocations,
                protocol,
            )
        })
        .into_response()
}

async fn serve_socket(
    mut socket: WebSocket,
    state: AppState,
    headers: HeaderMap,
    original_principal: AuthenticatedActor,
    mut receiver: broadcast::Receiver<RealtimeDelivery>,
    mut session_revocations: broadcast::Receiver<Uuid>,
    protocol: u8,
) {
    let mut revalidation = interval(SESSION_REVALIDATION_INTERVAL);
    revalidation.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Authentication was checked immediately before the upgrade. Consuming
    // the eager first tick prevents a redundant query on socket startup.
    revalidation.tick().await;
    // What it may hear, worked out once now and again when access changes
    // (K5.8), with a backstop every few minutes.
    let mut subscription = match subscribe(&state, &original_principal).await {
        Ok(subscription) => subscription,
        Err(error) => {
            tracing::error!(
                error_category = error.log_category(),
                "realtime subscription failed"
            );
            close_socket(&mut socket).await;
            return;
        }
    };
    let mut backstop = interval(SUBSCRIPTION_BACKSTOP_INTERVAL);
    backstop.set_missed_tick_behavior(MissedTickBehavior::Delay);
    backstop.tick().await;
    // Protocol 2: what it missed since `hello`'s position, then `ready`.
    // Live events were already arriving in `receiver`; those the replay
    // covered are dropped by position (catch up, then live).
    let mut last_position = 0_i64;
    if protocol == 2 {
        let resume_from = wait_for_hello(&mut socket).await;
        let mut sent = 0_usize;
        let mut position = resume_from.unwrap_or(0);
        let resumed = loop {
            match replay(
                &state,
                &original_principal,
                &mut subscription,
                resume_from,
                position,
            )
            .await
            {
                Ok(Replay::Batch {
                    frames,
                    events,
                    through,
                }) => {
                    for frame in frames {
                        if socket.send(Message::Text(frame.into())).await.is_err() {
                            return;
                        }
                    }
                    sent += events;
                    position = through;
                }
                Ok(Replay::Done { head, resumed }) => {
                    position = head;
                    break resumed;
                }
                Err(error) => {
                    tracing::error!(
                        error_category = error.log_category(),
                        "realtime resume failed"
                    );
                    close_socket(&mut socket).await;
                    return;
                }
            }
        };
        last_position = position;
        let ready = wire::ready(position, resumed, sent);
        if socket.send(Message::Text(ready.into())).await.is_err() {
            return;
        }
    }
    // Presence (K5.10): the connection is a row any instance can count.
    let connection_id = Uuid::now_v7();
    let account = original_principal.actor_id;
    match super::presence::open(&state.pool, connection_id, account, state.realtime.origin()).await
    {
        Ok(true) => announce(&state, account, &subscription, true).await,
        Ok(false) => {}
        Err(error) => tracing::warn!(
            error_category = error.log_category(),
            "realtime presence not recorded"
        ),
    }
    let mut live = LiveState::default();

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) if protocol == 2 => {
                        if let Some(frame) = wire::ClientFrame::parse(&text)
                            && let Err(error) = client_frame(
                                &state,
                                &original_principal,
                                connection_id,
                                &mut live,
                                frame,
                            )
                            .await
                        {
                            tracing::warn!(
                                error_category = error.log_category(),
                                "realtime client frame failed"
                            );
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
            received = receiver.recv() => {
                let event = match received {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            skipped_events = skipped,
                            "realtime client lagged; forcing durable-state resynchronization"
                        );
                        if protocol == 2 {
                            let frame = wire::resync("too_far_behind", last_position);
                            let _ = socket.send(Message::Text(frame.into())).await;
                        }
                        close_socket_to_restart(&mut socket, REALTIME_RESYNC_CLOSE_REASON).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if let RealtimeDelivery::Restart(reason) = event {
                    if protocol == 2 {
                        let frame = if reason == SERVER_DRAINING_CLOSE_REASON {
                            wire::reconnect(RECONNECT_AFTER_MS, reason)
                        } else {
                            wire::resync("too_far_behind", last_position)
                        };
                        let _ = socket.send(Message::Text(frame.into())).await;
                    }
                    close_socket_to_restart(&mut socket, reason).await;
                    break;
                }
                if let RealtimeDelivery::Live(frame) = &event {
                    if protocol == 2 {
                        let mut closed = false;
                        for text in live_frames(frame, &subscription, &live, connection_id, account) {
                            if socket.send(Message::Text(text.into())).await.is_err() {
                                closed = true;
                                break;
                            }
                        }
                        if closed {
                            break;
                        }
                    }
                    continue;
                }
                if let RealtimeDelivery::Durable { envelope, .. } = &event
                    && protocol == 2
                    && envelope.event_cursor <= last_position
                {
                    // The replay sent it already.
                    continue;
                }
                let access_may_change = refreshes(&subscription, &event);
                match refresh_if_changed(&state, &original_principal, &mut subscription, &event).await {
                    Ok(Some(access)) if protocol == 2 => {
                        if socket.send(Message::Text(access.into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(
                            error_category = error.log_category(),
                            "realtime subscription failed"
                        );
                        close_socket(&mut socket).await;
                        break;
                    }
                }
                // The documents it watches are checked again too: a
                // document's updates are routed by the watch alone.
                if access_may_change && live.recheck(&state, account).await.is_err() {
                    close_socket(&mut socket).await;
                    break;
                }
                let heard = match hears(&state, &original_principal, &subscription, &event)
                    .await
                {
                    Ok(heard) => heard,
                    Err(error) => {
                        // Fail closed: it reconnects and works out its set afresh.
                        tracing::error!(
                            error_category = error.log_category(),
                            event_id = %event.event_id(),
                            project_id = %event.project_id(),
                            "realtime subscription failed"
                        );
                        close_socket(&mut socket).await;
                        break;
                    }
                };
                if !heard {
                    continue;
                }
                let json = match frame(&event, protocol) {
                    Ok(json) => json,
                    Err(error) => {
                        tracing::error!(
                            event_id = %event.event_id(),
                            project_id = %event.project_id(),
                            error_category = %error,
                            "realtime event failed bounded serialization"
                        );
                        continue;
                    }
                };
                if socket.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
                if let RealtimeDelivery::Durable { envelope, .. } = &event {
                    last_position = last_position.max(envelope.event_cursor);
                }
            }
            revoked = session_revocations.recv() => {
                match revoked {
                    Ok(session_id) if session_id == original_principal.session_id => {
                        close_socket(&mut socket).await;
                        break;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        match current_principal(&state, &headers, &original_principal).await {
                            Ok(Some(_)) => {}
                            Ok(None) | Err(_) => {
                                close_socket(&mut socket).await;
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {}
                }
            }
            _ = backstop.tick() => {
                if live.recheck(&state, account).await.is_err() {
                    close_socket(&mut socket).await;
                    break;
                }
                match subscribe(&state, &original_principal).await {
                    Ok(fresh) => {
                        let access = wire::access(&subscription, &fresh);
                        subscription = fresh;
                        if let (2, Some(access)) = (protocol, access)
                            && socket.send(Message::Text(access.into())).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        close_socket(&mut socket).await;
                        break;
                    }
                }
            }
            _ = revalidation.tick() => {
                let _ = super::presence::renew(&state.pool, connection_id).await;
                match current_principal(&state, &headers, &original_principal).await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        close_socket(&mut socket).await;
                        break;
                    }
                    Err(error) => {
                        tracing::error!(
                            error_category = error.log_category(),
                            "realtime session revalidation failed"
                        );
                        close_socket(&mut socket).await;
                        break;
                    }
                }
            }
        }
    }
    if let Ok(true) = super::presence::close(&state.pool, connection_id, account).await {
        announce(&state, account, &subscription, false).await;
    }
}

/// What a protocol 2 connection keeps for presence, typing and watching.
#[derive(Default)]
pub(super) struct LiveState {
    watched: std::collections::BTreeSet<super::documents::DocumentRef>,
    last_typing: Option<tokio::time::Instant>,
}

impl LiveState {
    /// Drops the documents `account` can no longer open, so their updates
    /// stop reaching it.
    async fn recheck(&mut self, state: &AppState, account: Uuid) -> Result<(), AppError> {
        let mut kept = std::collections::BTreeSet::new();
        for document in std::mem::take(&mut self.watched) {
            if super::documents::can_view(&state.pool, account, document).await? {
                kept.insert(document);
            }
        }
        self.watched = kept;
        Ok(())
    }
}

/// Tells the account's workspaces it came online or went offline.
async fn announce(state: &AppState, account: Uuid, subscription: &SubscriptionSet, online: bool) {
    let mut workspaces = subscription.workspaces.iter().copied().collect::<Vec<_>>();
    workspaces.sort();
    super::ephemeral::publish(
        state,
        super::ephemeral::EphemeralFrame::Presence {
            account,
            workspaces,
            online,
        },
    )
    .await;
}

/// A protocol 2 client's own frame: typing, watching a document, or its
/// cursor in one it watches. Each is checked as it comes; one that isn't
/// allowed is dropped.
async fn client_frame(
    state: &AppState,
    principal: &AuthenticatedActor,
    connection: Uuid,
    live: &mut LiveState,
    frame: wire::ClientFrame,
) -> Result<(), AppError> {
    use super::ephemeral::{EphemeralFrame, publish};
    let account = principal.actor_id;
    match frame {
        wire::ClientFrame::Typing { conversation } => {
            let now = tokio::time::Instant::now();
            if live
                .last_typing
                .is_some_and(|last| now.duration_since(last) < wire::TYPING_EVERY)
            {
                return Ok(());
            }
            live.last_typing = Some(now);
            let Some((space, root, restricted)) = sqlx::query_as::<_, (Uuid, Uuid, bool)>(
                r#"
                SELECT COALESCE(conversation.project_id, direct_scope_id()),
                       COALESCE(conversation.parent_channel_id, conversation.id),
                       COALESCE(parent.visibility, conversation.visibility) <> 'project'
                FROM conversations conversation
                LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
                WHERE conversation.id = $1 AND conversation.archived_at IS NULL
                  AND conversation_visible_to(conversation.id, $2)
                "#,
            )
            .bind(conversation)
            .bind(account)
            .fetch_optional(crate::db::scoped(&state.pool))
            .await?
            else {
                return Ok(());
            };
            let until = (chrono::Utc::now() + wire::TYPING_SHOWS_FOR)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            publish(
                state,
                EphemeralFrame::Typing {
                    space,
                    root,
                    restricted,
                    conversation,
                    account,
                    until,
                },
            )
            .await;
        }
        wire::ClientFrame::Watch { document } => {
            if live.watched.len() < wire::MAX_WATCHED
                && super::documents::can_view(&state.pool, account, document).await?
            {
                live.watched.insert(document);
            }
        }
        wire::ClientFrame::Unwatch { document } => {
            live.watched.remove(&document);
        }
        wire::ClientFrame::Awareness {
            document,
            state: cursor,
        } => {
            if live.watched.contains(&document)
                && cursor.to_string().len() <= wire::MAX_AWARENESS_BYTES
            {
                publish(
                    state,
                    EphemeralFrame::Awareness {
                        document,
                        connection,
                        account,
                        state: cursor,
                    },
                )
                .await;
            }
        }
        wire::ClientFrame::Hello { .. } | wire::ClientFrame::Other => {}
    }
    Ok(())
}

/// The protocol 2 frames a live signal becomes for this connection, routed
/// by its set and the documents it watches, with no query.
pub(super) fn live_frames(
    frame: &super::ephemeral::EphemeralFrame,
    subscription: &SubscriptionSet,
    live: &LiveState,
    connection: Uuid,
    account: Uuid,
) -> Vec<String> {
    use super::ephemeral::EphemeralFrame;
    match frame {
        EphemeralFrame::Presence {
            account: who,
            workspaces,
            online,
        } => {
            if *who == account {
                return Vec::new();
            }
            workspaces
                .iter()
                .filter(|workspace| subscription.workspaces.contains(workspace))
                .map(|workspace| wire::presence(*workspace, *who, *online))
                .collect()
        }
        EphemeralFrame::Typing {
            space,
            root,
            restricted,
            conversation,
            account: who,
            until,
        } => {
            let route = Route {
                workspace: Uuid::nil(),
                space: Some(*space),
                audience: RouteAudience::Conversation {
                    root: *root,
                    restricted: *restricted,
                },
                event_type: String::new(),
                about_account: None,
            };
            if *who == account || subscription.hears(&route) != Hearing::Yes {
                return Vec::new();
            }
            vec![wire::typing(*conversation, *who, until)]
        }
        EphemeralFrame::Awareness {
            document,
            connection: from,
            account: who,
            state,
        } => {
            if *from == connection || !live.watched.contains(document) {
                return Vec::new();
            }
            vec![wire::awareness(*document, *from, *who, state)]
        }
        EphemeralFrame::DocumentUpdate {
            document,
            generation,
            version,
            update,
        } => {
            if !live.watched.contains(document) {
                return Vec::new();
            }
            vec![wire::document_update(
                *document,
                *generation,
                *version,
                update,
            )]
        }
        _ => Vec::new(),
    }
}

async fn current_principal(
    state: &AppState,
    headers: &HeaderMap,
    original: &AuthenticatedActor,
) -> Result<Option<AuthenticatedActor>, AppError> {
    let Some(current) = load_authenticated_actor(state, headers).await? else {
        return Ok(None);
    };
    if current.session_id != original.session_id
        || current.user_id != original.user_id
        || current.actor_id != original.actor_id
    {
        return Ok(None);
    }
    Ok(Some(current))
}

/// How long a connection's set may go without being worked out again,
/// whatever the log says (K5.8's backstop).
const SUBSCRIPTION_BACKSTOP_INTERVAL: Duration = Duration::from_secs(300);

/// The connection's set, by the realtime policy for each space it holds.
async fn subscribe(
    state: &AppState,
    principal: &AuthenticatedActor,
) -> Result<SubscriptionSet, AppError> {
    subscription_for(state, principal.actor_id).await
}

/// What `account` may hear now, by the realtime policy.
pub(super) async fn subscription_for(
    state: &AppState,
    account: Uuid,
) -> Result<SubscriptionSet, AppError> {
    subscriptions::compute(&state.pool, account, |role, space| {
        realtime_policy_allows(account, role, true, space).unwrap_or(false)
    })
    .await
}

/// How long a protocol 2 client waits before reconnecting to another
/// instance when this one drains (it adds its own jitter).
const RECONNECT_AFTER_MS: u64 = 1_000;
/// Events one resume batch reads.
const REPLAY_BATCH: i64 = 500;

/// The account an `account.access_changed` signal is about.
pub(super) fn access_changed(event: &crate::events::LoggedEvent) -> Option<Uuid> {
    (event.event_type == crate::events::catalog::ACCOUNT_ACCESS_CHANGED.name)
        .then_some(event.audience_account_id)
        .flatten()
}

/// The frame a contract connection gets for a durable delivery: protocol
/// 2's `event`.
pub(super) fn contract_frame(event: &RealtimeDelivery) -> Result<String, RealtimePublishError> {
    frame(event, 2)
}

/// The frame a delivery is sent as in the connection's protocol.
fn frame(event: &RealtimeDelivery, protocol: u8) -> Result<String, RealtimePublishError> {
    let json = event.to_wire_json()?;
    if protocol != 2 {
        return Ok(json);
    }
    Ok(match event {
        RealtimeDelivery::Durable { envelope, .. } => wire::event(envelope.event_cursor, &json),
        RealtimeDelivery::Ephemeral(delta) => {
            wire::stream_delta(delta.conversation_id, &delta.payload)
        }
        RealtimeDelivery::Restart(_)
        | RealtimeDelivery::AccessChanged(_)
        | RealtimeDelivery::Live(_) => {
            return Err(RealtimePublishError::Serialization);
        }
    })
}

/// A protocol 2 connection's `hello`: where it would resume from, if it
/// says. Anything else first, or nothing in time, means no resume.
async fn wait_for_hello(socket: &mut WebSocket) -> Option<i64> {
    let first = tokio::time::timeout(wire::HELLO_WAIT, socket.recv())
        .await
        .ok()??
        .ok()?;
    let Message::Text(text) = first else {
        return None;
    };
    match wire::ClientFrame::parse(&text)? {
        wire::ClientFrame::Hello { resume_from } => resume_from,
        _ => None,
    }
}

/// One step of a resume.
pub(super) enum Replay {
    /// Frames this connection may hear, from the log, through `through`.
    Batch {
        frames: Vec<String>,
        events: usize,
        through: i64,
    },
    /// Caught up to the head (or not resumable: `resumed` false).
    Done { head: i64, resumed: bool },
}

/// The next batch of what the connection missed after `position`, routed
/// by its set as live events are (access changes in the batch recompute
/// it, in order). Each batch reads in a transaction of its own, closed
/// before any frame is sent.
pub(super) async fn replay(
    state: &AppState,
    principal: &AuthenticatedActor,
    subscription: &mut SubscriptionSet,
    resume_from: Option<i64>,
    position: i64,
) -> Result<Replay, AppError> {
    let (head, batch) = {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        let head = crate::events::head(&mut connection).await?;
        let Some(from) =
            resume_from.filter(|from| *from <= head && head - *from <= wire::RESUME_LIMIT)
        else {
            return Ok(Replay::Done {
                head,
                resumed: false,
            });
        };
        if position.max(from) >= head {
            return Ok(Replay::Done {
                head,
                resumed: true,
            });
        }
        let batch = crate::events::tail(&mut connection, position.max(from), REPLAY_BATCH).await?;
        (head, batch)
    };
    let Some(through) = batch.last().map(|event| event.position) else {
        return Ok(Replay::Done {
            head,
            resumed: true,
        });
    };
    let mut frames = Vec::new();
    let mut events = 0;
    for logged in &batch {
        if let Some(account) = access_changed(logged) {
            let signal = RealtimeDelivery::AccessChanged(account);
            if let Some(access) =
                refresh_if_changed(state, principal, subscription, &signal).await?
            {
                frames.push(access);
            }
            continue;
        }
        let Some(envelope) = OutboxEnvelope::from_logged(logged) else {
            continue;
        };
        let Some(route) = Route::of_logged(logged) else {
            continue;
        };
        let Ok(delivery) = checked_delivery(&envelope, route) else {
            continue;
        };
        if let Some(access) = refresh_if_changed(state, principal, subscription, &delivery).await? {
            frames.push(access);
        }
        if hears(state, principal, subscription, &delivery).await?
            && let Ok(frame) = frame(&delivery, 2)
        {
            frames.push(frame);
            events += 1;
        }
    }
    Ok(Replay::Batch {
        frames,
        events,
        through,
    })
}

#[derive(Debug, FromRow)]
struct RealtimeMembership {
    role: String,
    enabled: bool,
}

/// A streamed delta's check: the account's membership of its space now,
/// and its conversation by the one gate.
async fn ephemeral_is_authorized(
    state: &AppState,
    principal: &AuthenticatedActor,
    delta: &RealtimeEvent,
) -> Result<bool, AppError> {
    // A DM or group (and threads under one) has no space: its participants
    // hear it, by `conversation_visible_to` alone.
    if delta.project_id == crate::spaces::DIRECT {
        let Some(conversation_id) = delta.conversation_id else {
            return Ok(false);
        };
        return sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
            .bind(conversation_id)
            .bind(principal.actor_id)
            .fetch_one(crate::db::scoped(&state.pool))
            .await
            .map_err(AppError::from);
    }
    let membership = sqlx::query_as::<_, RealtimeMembership>(
        r#"
        SELECT pm.role, pm.enabled
        FROM project_memberships pm
        JOIN projects p ON p.id = pm.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = pm.actor_id
         AND om.enabled
        WHERE pm.actor_id = $1
          AND pm.project_id = $2
          AND p.archived_at IS NULL
          AND (
              $3::uuid IS NULL
              OR EXISTS (
                  SELECT 1
                  FROM conversations c
                  WHERE c.id = $3
                    AND c.project_id = pm.project_id
                    AND conversation_visible_to(c.id, pm.actor_id)
              )
          )
        "#,
    )
    .bind(principal.actor_id)
    .bind(delta.project_id)
    .bind(delta.conversation_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?;
    let Some(membership) = membership else {
        return Ok(false);
    };
    realtime_policy_allows(
        principal.actor_id,
        &membership.role,
        membership.enabled,
        delta.project_id,
    )
}

/// Before routing an access change about the connection, its set is
/// worked out again; the answer is protocol 2's `access` frame, when
/// anything changed.
async fn refresh_if_changed(
    state: &AppState,
    principal: &AuthenticatedActor,
    subscription: &mut SubscriptionSet,
    event: &RealtimeDelivery,
) -> Result<Option<String>, AppError> {
    if !refreshes(subscription, event) {
        return Ok(None);
    }
    let fresh = subscribe(state, principal).await?;
    let access = wire::access(subscription, &fresh);
    *subscription = fresh;
    Ok(access)
}

/// Whether `event` may change what the connection's account can reach.
fn refreshes(subscription: &SubscriptionSet, event: &RealtimeDelivery) -> bool {
    match event {
        RealtimeDelivery::Durable { route, .. } => subscription.changed_by(route),
        RealtimeDelivery::AccessChanged(account) => *account == subscription.account,
        _ => false,
    }
}

async fn hears(
    state: &AppState,
    principal: &AuthenticatedActor,
    subscription: &SubscriptionSet,
    event: &RealtimeDelivery,
) -> Result<bool, AppError> {
    if !event.is_visible_to_actor(principal.actor_id) {
        return Ok(false);
    }
    match event {
        RealtimeDelivery::Durable { route, .. } => match subscription.hears(route) {
            Hearing::Yes => Ok(true),
            Hearing::No => Ok(false),
            Hearing::IfBoardVisible(board) => {
                crate::whiteboards::visible_to(&state.pool, board, principal.actor_id).await
            }
        },
        // Streamed words carry no audience and are never in the log: each
        // is checked as it comes, as before K5.8.
        RealtimeDelivery::Ephemeral(delta) => {
            ephemeral_is_authorized(state, principal, delta).await
        }
        RealtimeDelivery::Restart(_)
        | RealtimeDelivery::AccessChanged(_)
        | RealtimeDelivery::Live(_) => Ok(false),
    }
}

fn realtime_policy_allows(
    actor_id: Uuid,
    role: &str,
    membership_enabled: bool,
    project_id: Uuid,
) -> Result<bool, AppError> {
    // Subscribing needs only "Open projects", which every role holds,
    // custom ones included, so the built-in role decides it.
    Ok(crate::access::membership_allows(
        actor_id,
        project_id,
        RoleGrant::built_in(parse_membership_role(role)?),
        membership_enabled,
        Action::RealtimeSubscribe.permission(),
    ))
}

fn parse_membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("unknown Project membership role")),
    }
}

async fn close_socket(socket: &mut WebSocket) {
    let _ = socket.send(Message::Close(None)).await;
}

async fn close_socket_to_restart(socket: &mut WebSocket, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: close_code::RESTART,
            reason: reason.into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::http::{HeaderMap, HeaderValue};
    use tokio::sync::broadcast;
    use uuid::Uuid;

    use super::{
        RealtimeDelivery, RealtimeEvent, RealtimeHub, RealtimePublishError, realtime_policy_allows,
    };
    use crate::{
        auth::origin_is_allowed,
        config::AppConfig,
        outbox::{OUTBOX_SCHEMA_VERSION, OutboxEnvelope, OutboxPublisher},
    };

    #[test]
    fn websocket_origin_uses_the_exact_configured_allowlist() {
        let config = AppConfig::from_values(&BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            "postgres://example.invalid/db".to_owned(),
        )]))
        .expect("config");
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:5173"));
        assert!(origin_is_allowed(&config, &headers));
        headers.insert(
            "origin",
            HeaderValue::from_static("http://127.0.0.1:5173.attacker.example"),
        );
        assert!(!origin_is_allowed(&config, &headers));
    }

    #[test]
    fn realtime_delivery_uses_subscription_policy_for_each_event() {
        let event = RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "message.delta",
            project_id: Uuid::now_v7(),
            conversation_id: Some(Uuid::now_v7()),
            payload: serde_json::json!({"message_id": Uuid::now_v7(), "delta": "test"}),
        };
        let actor_id = Uuid::now_v7();

        assert!(
            realtime_policy_allows(actor_id, "viewer", true, event.project_id,)
                .expect("viewer policy")
        );
        assert!(
            realtime_policy_allows(actor_id, "member", true, event.project_id,)
                .expect("external member policy")
        );
        assert!(
            !realtime_policy_allows(actor_id, "owner", false, event.project_id,)
                .expect("disabled membership policy")
        );
        assert!(
            !realtime_policy_allows(actor_id, "worker", true, event.project_id,)
                .expect("undelegated Worker policy")
        );
    }

    #[tokio::test]
    async fn durable_outbox_envelope_preserves_scope_and_wire_contract() {
        let hub = RealtimeHub::new(4);
        let mut receiver = hub.subscribe();
        let conversation_id = Uuid::now_v7();
        let envelope = OutboxEnvelope {
            event_id: Uuid::now_v7(),
            event_cursor: 41,
            version: OUTBOX_SCHEMA_VERSION,
            project_id: Uuid::now_v7(),
            conversation_id: Some(conversation_id),
            recipient_actor_id: None,
            event_type: "conversation.message.created".to_owned(),
            occurred_at: "2026-07-31T15:00:00.000000Z".to_owned(),
            payload: serde_json::json!({
                "conversationId": conversation_id,
                "messageId": Uuid::now_v7(),
                "sequence": 2,
                "authorActorId": Uuid::now_v7(),
            }),
        };

        OutboxPublisher::publish(&hub, &envelope).expect("publish durable envelope");
        let delivery = receiver.recv().await.expect("durable delivery");
        assert_eq!(delivery.conversation_id(), Some(conversation_id));
        assert_eq!(delivery.event_id(), envelope.event_id);
        let wire: serde_json::Value =
            serde_json::from_str(&delivery.to_wire_json().expect("bounded realtime wire JSON"))
                .expect("decode realtime wire JSON");
        assert_eq!(wire["event_cursor"], 41);
        assert_eq!(wire["version"], OUTBOX_SCHEMA_VERSION);
        assert_eq!(wire["conversationId"], conversation_id.to_string());
        assert_eq!(wire["type"], "conversation.message.created");
        assert!(wire.get("kind").is_none());
    }

    #[test]
    fn conversation_updates_and_reply_requests_reach_subscribers() {
        let hub = RealtimeHub::new(2);
        let mut receiver = hub.subscribe();
        let conversation_id = Uuid::now_v7();
        let project_id = Uuid::now_v7();
        for (event_cursor, event_type, payload) in [
            (
                41,
                "conversation.updated",
                serde_json::json!({ "conversationId": conversation_id, "version": 2 }),
            ),
            (
                42,
                "conversation.reply_requests.updated",
                serde_json::json!({ "conversationId": conversation_id }),
            ),
        ] {
            let envelope = OutboxEnvelope {
                event_id: Uuid::now_v7(),
                event_cursor,
                version: OUTBOX_SCHEMA_VERSION,
                project_id,
                conversation_id: Some(conversation_id),
                recipient_actor_id: None,
                event_type: event_type.to_owned(),
                occurred_at: "2026-09-26T15:00:00Z".to_owned(),
                payload,
            };
            OutboxPublisher::publish(&hub, &envelope).expect("publish conversation update");
            let delivery = receiver.try_recv().expect("conversation update delivery");
            assert_eq!(delivery.conversation_id(), Some(conversation_id));
            let wire: serde_json::Value =
                serde_json::from_str(&delivery.to_wire_json().expect("bounded wire JSON"))
                    .expect("decode wire JSON");
            assert_eq!(wire["type"], event_type);
            assert_eq!(wire["conversationId"], conversation_id.to_string());
        }
    }

    #[test]
    fn message_updates_are_conversation_scoped() {
        let hub = RealtimeHub::new(2);
        let mut receiver = hub.subscribe();
        let conversation_id = Uuid::now_v7();
        let message_id = Uuid::now_v7();
        for (event_type, payload) in [
            (
                "conversation.message.updated",
                serde_json::json!({
                    "conversationId": conversation_id,
                    "messageId": message_id,
                    "revision": 2,
                    "removed": true,
                }),
            ),
            (
                "conversation.message.reactions.updated",
                serde_json::json!({
                    "conversationId": conversation_id,
                    "messageId": message_id,
                }),
            ),
        ] {
            let envelope = OutboxEnvelope {
                event_id: Uuid::now_v7(),
                event_cursor: 43,
                version: OUTBOX_SCHEMA_VERSION,
                project_id: Uuid::now_v7(),
                conversation_id: Some(conversation_id),
                recipient_actor_id: None,
                event_type: event_type.to_owned(),
                occurred_at: "2026-10-01T15:00:00Z".to_owned(),
                payload,
            };
            OutboxPublisher::publish(&hub, &envelope).expect("publish message update");
            let delivery = receiver.try_recv().expect("message update delivery");
            assert_eq!(delivery.conversation_id(), Some(conversation_id));

            // Never project-wide, never targeted, never about another conversation.
            let mut unscoped = envelope.clone();
            unscoped.conversation_id = None;
            assert!(OutboxPublisher::publish(&hub, &unscoped).is_err());
            let mut targeted = envelope.clone();
            targeted.recipient_actor_id = Some(Uuid::now_v7());
            assert!(OutboxPublisher::publish(&hub, &targeted).is_err());
            let mut mismatched = envelope;
            mismatched.payload["conversationId"] = serde_json::json!(Uuid::now_v7());
            assert!(OutboxPublisher::publish(&hub, &mismatched).is_err());
        }
    }

    #[test]
    fn durable_publication_without_a_connected_browser_is_successful() {
        let hub = RealtimeHub::new(1);
        let conversation_id = Uuid::now_v7();
        let envelope = OutboxEnvelope {
            event_id: Uuid::now_v7(),
            event_cursor: 1,
            version: OUTBOX_SCHEMA_VERSION,
            project_id: Uuid::now_v7(),
            conversation_id: Some(conversation_id),
            recipient_actor_id: None,
            event_type: "conversation.created".to_owned(),
            occurred_at: "2026-07-31T15:00:00Z".to_owned(),
            payload: serde_json::json!({
                "conversationId": conversation_id,
                "kind": "channel",
                "version": 1,
            }),
        };

        OutboxPublisher::publish(&hub, &envelope)
            .expect("no subscriber does not make durable publication fail");
    }

    #[test]
    fn durable_project_scoped_work_item_event_has_no_conversation() {
        let hub = RealtimeHub::new(2);
        let mut receiver = hub.subscribe();
        let work_item_id = Uuid::now_v7();
        let envelope = OutboxEnvelope {
            event_id: Uuid::now_v7(),
            event_cursor: 2,
            version: OUTBOX_SCHEMA_VERSION,
            project_id: Uuid::now_v7(),
            conversation_id: None,
            recipient_actor_id: None,
            event_type: "work_item.updated".to_owned(),
            occurred_at: "2026-07-31T15:00:00Z".to_owned(),
            payload: serde_json::json!({
                "workItemId": work_item_id,
                "version": 1,
            }),
        };

        OutboxPublisher::publish(&hub, &envelope).expect("publish Work Item invalidation");
        let delivery = receiver.try_recv().expect("Work Item delivery");
        assert_eq!(delivery.conversation_id(), None);
        let wire: serde_json::Value = serde_json::from_str(
            &delivery
                .to_wire_json()
                .expect("bounded Work Item wire JSON"),
        )
        .expect("decode Work Item wire JSON");
        assert!(wire["conversationId"].is_null());
        assert_eq!(wire["payload"]["workItemId"], work_item_id.to_string());
    }

    #[test]
    fn membership_update_is_project_scoped_and_cannot_cross_projects() {
        let hub = RealtimeHub::new(2);
        let mut receiver = hub.subscribe();
        let project_id = Uuid::now_v7();
        let actor_id = Uuid::now_v7();
        let envelope = OutboxEnvelope {
            event_id: Uuid::now_v7(),
            event_cursor: 3,
            version: OUTBOX_SCHEMA_VERSION,
            project_id,
            conversation_id: None,
            recipient_actor_id: None,
            event_type: "project.membership.updated".to_owned(),
            occurred_at: "2026-09-26T15:00:00Z".to_owned(),
            payload: serde_json::json!({
                "projectId": project_id,
                "actorId": actor_id,
                "version": 2,
            }),
        };
        OutboxPublisher::publish(&hub, &envelope).expect("publish membership update");
        let delivery = receiver.try_recv().expect("membership update delivery");
        assert_eq!(delivery.conversation_id(), None);
        let wire: serde_json::Value =
            serde_json::from_str(&delivery.to_wire_json().expect("bounded wire JSON"))
                .expect("decode wire JSON");
        assert_eq!(wire["type"], "project.membership.updated");
        assert_eq!(wire["payload"]["projectId"], project_id.to_string());

        let other_project = OutboxEnvelope {
            project_id: Uuid::now_v7(),
            ..envelope.clone()
        };
        assert_eq!(
            OutboxPublisher::publish(&hub, &other_project),
            Err(RealtimePublishError::InvalidMetadata)
        );
        let conversation_scoped = OutboxEnvelope {
            conversation_id: Some(Uuid::now_v7()),
            ..envelope
        };
        assert_eq!(
            OutboxPublisher::publish(&hub, &conversation_scoped),
            Err(RealtimePublishError::InvalidMetadata)
        );
    }

    #[test]
    fn durable_publication_rejects_missing_scope_and_oversized_frames() {
        let hub = RealtimeHub::new(1);
        let conversation_id = Uuid::now_v7();
        let base = OutboxEnvelope {
            event_id: Uuid::now_v7(),
            event_cursor: 1,
            version: OUTBOX_SCHEMA_VERSION,
            project_id: Uuid::now_v7(),
            conversation_id: Some(conversation_id),
            recipient_actor_id: None,
            event_type: "conversation.created".to_owned(),
            occurred_at: "2026-07-31T15:00:00Z".to_owned(),
            payload: serde_json::json!({
                "conversationId": conversation_id,
                "kind": "channel",
                "version": 1
            }),
        };
        let missing_scope = OutboxEnvelope {
            conversation_id: Some(Uuid::nil()),
            ..base.clone()
        };
        assert_eq!(
            OutboxPublisher::publish(&hub, &missing_scope),
            Err(RealtimePublishError::InvalidMetadata)
        );
        let absent_scope = OutboxEnvelope {
            conversation_id: None,
            payload: serde_json::json!({
                "kind": "channel",
                "version": 1
            }),
            ..base.clone()
        };
        assert_eq!(
            OutboxPublisher::publish(&hub, &absent_scope),
            Err(RealtimePublishError::InvalidMetadata)
        );

        let work_item_with_conversation = OutboxEnvelope {
            conversation_id: Some(conversation_id),
            event_type: "work_item.updated".to_owned(),
            payload: serde_json::json!({
                "workItemId": Uuid::now_v7(),
                "version": 1
            }),
            ..base.clone()
        };
        assert_eq!(
            OutboxPublisher::publish(&hub, &work_item_with_conversation),
            Err(RealtimePublishError::InvalidMetadata)
        );

        let oversized = OutboxEnvelope {
            payload: serde_json::json!({
                "conversationId": conversation_id,
                "value": "x".repeat(70_000),
            }),
            ..base
        };
        assert_eq!(
            OutboxPublisher::publish(&hub, &oversized),
            Err(RealtimePublishError::FrameTooLarge)
        );
    }

    #[test]
    fn read_state_delivery_is_visible_only_to_its_actor_and_scope_is_not_serialized() {
        let recipient = Uuid::now_v7();
        let other_project_member = Uuid::now_v7();
        let conversation_id = Uuid::now_v7();
        let envelope = OutboxEnvelope {
            event_id: Uuid::now_v7(),
            event_cursor: 1,
            version: OUTBOX_SCHEMA_VERSION,
            project_id: Uuid::now_v7(),
            conversation_id: Some(conversation_id),
            recipient_actor_id: Some(recipient),
            event_type: "conversation.read_state.updated".to_owned(),
            occurred_at: "2026-07-31T15:00:00Z".to_owned(),
            payload: serde_json::json!({
                "conversationId": conversation_id,
                "actorId": recipient,
                "highestSeenSequence": 4,
                "version": 2,
            }),
        };
        OutboxPublisher::publish(&RealtimeHub::new(1), &envelope)
            .expect("publish correctly targeted read state");
        let mismatched = OutboxEnvelope {
            recipient_actor_id: Some(other_project_member),
            ..envelope.clone()
        };
        assert_eq!(
            OutboxPublisher::publish(&RealtimeHub::new(1), &mismatched),
            Err(RealtimePublishError::InvalidMetadata)
        );
        let route = super::envelope_route(&envelope);
        let delivery = RealtimeDelivery::Durable { envelope, route };

        assert!(delivery.is_visible_to_actor(recipient));
        assert!(!delivery.is_visible_to_actor(other_project_member));
        let wire: serde_json::Value = serde_json::from_str(
            &delivery
                .to_wire_json()
                .expect("bounded targeted wire event"),
        )
        .expect("decode targeted wire event");
        assert_eq!(wire["conversationId"], conversation_id.to_string());
        assert!(wire.get("recipient_actor_id").is_none());
    }

    #[test]
    fn member_removed_reaches_only_the_removed_actor() {
        let removed = Uuid::now_v7();
        let other = Uuid::now_v7();
        let group = Uuid::now_v7();
        let envelope = OutboxEnvelope {
            event_id: Uuid::now_v7(),
            event_cursor: 1,
            version: OUTBOX_SCHEMA_VERSION,
            project_id: Uuid::now_v7(),
            conversation_id: None,
            recipient_actor_id: Some(removed),
            event_type: "conversation.member.removed".to_owned(),
            occurred_at: "2026-10-02T15:00:00Z".to_owned(),
            payload: serde_json::json!({
                "conversationId": group,
                "actorId": removed,
                "version": 3,
            }),
        };
        OutboxPublisher::publish(&RealtimeHub::new(1), &envelope)
            .expect("publish an unscoped member removal to its actor");
        let refused = [
            // Conversation-scoped: delivery would depend on visibility the
            // actor just lost.
            OutboxEnvelope {
                conversation_id: Some(group),
                ..envelope.clone()
            },
            // Aimed at someone other than the actor it names.
            OutboxEnvelope {
                recipient_actor_id: Some(other),
                ..envelope.clone()
            },
            // Aimed at nobody: it would reach everyone in the project.
            OutboxEnvelope {
                recipient_actor_id: None,
                ..envelope.clone()
            },
            // No group named.
            OutboxEnvelope {
                payload: serde_json::json!({"actorId": removed, "version": 3}),
                ..envelope.clone()
            },
        ];
        for envelope in &refused {
            assert_eq!(
                OutboxPublisher::publish(&RealtimeHub::new(1), envelope),
                Err(RealtimePublishError::InvalidMetadata)
            );
        }
        // Other conversation events keep their scope checks: none may go out
        // without a conversation the way member.removed does.
        for event_type in [
            "conversation.updated",
            "conversation.message.updated",
            "conversation.message.reactions.updated",
            "conversation.read_state.updated",
        ] {
            let unscoped = OutboxEnvelope {
                event_type: event_type.to_owned(),
                recipient_actor_id: if event_type == "conversation.read_state.updated" {
                    Some(removed)
                } else {
                    None
                },
                ..envelope.clone()
            };
            assert_eq!(
                OutboxPublisher::publish(&RealtimeHub::new(1), &unscoped),
                Err(RealtimePublishError::InvalidMetadata),
                "{event_type} without a conversation scope"
            );
        }
        // A read-state event must name its actor in the payload too.
        let read_without_actor = OutboxEnvelope {
            conversation_id: Some(group),
            event_type: "conversation.read_state.updated".to_owned(),
            payload: serde_json::json!({
                "conversationId": group,
                "highestSeenSequence": 1,
                "version": 2,
            }),
            ..envelope.clone()
        };
        assert_eq!(
            OutboxPublisher::publish(&RealtimeHub::new(1), &read_without_actor),
            Err(RealtimePublishError::InvalidMetadata)
        );
        let route = super::envelope_route(&envelope);
        let delivery = RealtimeDelivery::Durable { envelope, route };
        assert!(delivery.is_visible_to_actor(removed));
        assert!(!delivery.is_visible_to_actor(other));
    }

    #[test]
    fn ephemeral_provider_compatibility_events_are_allowlisted_and_bounded() {
        let hub = RealtimeHub::new(2);
        let mut receiver = hub.subscribe();
        hub.publish(RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "message.delta",
            project_id: Uuid::now_v7(),
            conversation_id: Some(Uuid::now_v7()),
            payload: serde_json::json!({"delta": "hello"}),
        });
        assert!(matches!(
            receiver.try_recv(),
            Ok(RealtimeDelivery::Ephemeral(_))
        ));

        hub.publish(RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "message.delta",
            project_id: Uuid::now_v7(),
            conversation_id: Some(Uuid::now_v7()),
            payload: serde_json::json!({"delta": "x".repeat(66_000)}),
        });
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn session_revocation_is_addressed_by_opaque_session_id() {
        let hub = RealtimeHub::new(4);
        let mut revocations = hub.subscribe_session_revocations();
        let session_id = Uuid::now_v7();

        hub.revoke_session(session_id);

        assert_eq!(
            revocations.recv().await.expect("session revocation"),
            session_id
        );
    }
}

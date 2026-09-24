use std::{process, time::Duration};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket, close_code},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use tokio::{
    sync::{broadcast, watch},
    time::{MissedTickBehavior, interval},
};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, load_authenticated_actor, origin_is_allowed},
    error::AppError,
    outbox::{self, OUTBOX_SCHEMA_VERSION, OutboxEnvelope, OutboxFailureKind, OutboxPublisher},
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
};

const SESSION_REVALIDATION_INTERVAL: Duration = Duration::from_secs(15);
const REALTIME_RESYNC_CLOSE_REASON: &str = "realtime lag; reconnect and refetch";
const MAX_REALTIME_FRAME_BYTES: usize = 70_000;
const MAX_REALTIME_CLIENT_FRAME_BYTES: usize = 4_096;
const MAX_EPHEMERAL_PAYLOAD_BYTES: usize = 65_536;
const OUTBOX_POLL_INTERVAL: Duration = Duration::from_millis(100);
const OUTBOX_BATCH_SIZE: i64 = 32;
const OUTBOX_LEASE_SECONDS: i32 = 30;

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
enum RealtimeDelivery {
    Durable { envelope: OutboxEnvelope },
    Ephemeral(RealtimeEvent),
}

impl RealtimeDelivery {
    fn event_id(&self) -> Uuid {
        match self {
            Self::Durable { envelope, .. } => envelope.event_id,
            Self::Ephemeral(event) => event.event_id,
        }
    }

    fn project_id(&self) -> Uuid {
        match self {
            Self::Durable { envelope, .. } => envelope.project_id,
            Self::Ephemeral(event) => event.project_id,
        }
    }

    fn conversation_id(&self) -> Option<Uuid> {
        match self {
            Self::Durable { envelope } => envelope.conversation_id,
            Self::Ephemeral(event) => event.conversation_id,
        }
    }

    fn recipient_actor_id(&self) -> Option<Uuid> {
        match self {
            Self::Durable { envelope } => envelope.recipient_actor_id,
            Self::Ephemeral(_) => None,
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

#[derive(Clone)]
pub struct RealtimeHub {
    sender: broadcast::Sender<RealtimeDelivery>,
    session_revocations: broadcast::Sender<Uuid>,
}

impl RealtimeHub {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let (session_revocations, _) = broadcast::channel(capacity);
        Self {
            sender,
            session_revocations,
        }
    }

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

    pub fn revoke_session(&self, session_id: Uuid) {
        let _ = self.session_revocations.send(session_id);
    }

    fn subscribe(&self) -> broadcast::Receiver<RealtimeDelivery> {
        self.sender.subscribe()
    }

    fn subscribe_session_revocations(&self) -> broadcast::Receiver<Uuid> {
        self.session_revocations.subscribe()
    }
}

impl OutboxPublisher for RealtimeHub {
    type Error = RealtimePublishError;

    fn publish(&self, envelope: &OutboxEnvelope) -> Result<(), Self::Error> {
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
        let invalid_actor_scope = if envelope.event_type == "conversation.read_state.updated" {
            payload_actor_id != envelope.recipient_actor_id
        } else {
            envelope.recipient_actor_id.is_some()
        };
        let supported_event_type = matches!(
            envelope.event_type.as_str(),
            "conversation.created"
                | "conversation.archived"
                | "conversation.message.created"
                | "conversation.read_state.updated"
                | "conversation.agent_run.updated"
                | "work_item.updated"
        );
        let invalid_conversation_scope = if envelope.event_type == "work_item.updated" {
            envelope.conversation_id.is_some() || envelope.payload.get("conversationId").is_some()
        } else {
            envelope.conversation_id.is_none()
                || payload_conversation_id != envelope.conversation_id
        };
        if envelope.event_id.is_nil()
            || envelope.event_cursor < 1
            || envelope.version != OUTBOX_SCHEMA_VERSION
            || envelope.project_id.is_nil()
            || envelope.conversation_id.is_some_and(|id| id.is_nil())
            || invalid_conversation_scope
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
        let delivery = RealtimeDelivery::Durable {
            envelope: envelope.clone(),
        };
        // Validate and bound the exact frame before making it observable.
        delivery.to_wire_json()?;
        // No connected browser is still a successful broadcast operation:
        // reconnect correctness comes from HTTP refetch, not socket replay.
        let _ = self.sender.send(delivery);
        Ok(())
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
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
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
) {
    let mut revalidation = interval(SESSION_REVALIDATION_INTERVAL);
    revalidation.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Authentication was checked immediately before the upgrade. Consuming
    // the eager first tick prevents a redundant query on socket startup.
    revalidation.tick().await;

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
                        close_socket_for_resync(&mut socket).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let current_principal =
                    match current_principal(&state, &headers, &original_principal).await {
                        Ok(Some(principal)) => principal,
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
                    };
                match event_is_authorized(&state, &current_principal, &event).await {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => {
                        tracing::error!(
                            error_category = error.log_category(),
                            event_id = %event.event_id(),
                            project_id = %event.project_id(),
                            "realtime authorization failed"
                        );
                        close_socket(&mut socket).await;
                        break;
                    }
                }
                let json = match event.to_wire_json() {
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
            _ = revalidation.tick() => {
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

#[derive(Debug, FromRow)]
struct RealtimeMembership {
    role: String,
    enabled: bool,
}

async fn event_is_authorized(
    state: &AppState,
    principal: &AuthenticatedActor,
    event: &RealtimeDelivery,
) -> Result<bool, AppError> {
    if !event.is_visible_to_actor(principal.actor_id) {
        return Ok(false);
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
    .bind(event.project_id())
    .bind(event.conversation_id())
    .fetch_optional(&state.pool)
    .await?;
    let Some(membership) = membership else {
        return Ok(false);
    };
    realtime_policy_allows(
        principal.actor_id,
        &principal.actor_kind,
        &membership.role,
        membership.enabled,
        event.project_id(),
        event.conversation_id(),
    )
}

fn realtime_policy_allows(
    actor_id: Uuid,
    actor_kind: &str,
    role: &str,
    membership_enabled: bool,
    project_id: Uuid,
    conversation_id: Option<Uuid>,
) -> Result<bool, AppError> {
    let subject = Subject::new(
        actor_id,
        parse_actor_kind(actor_kind)?,
        true,
        Some(Membership::project(
            project_id,
            parse_membership_role(role)?,
            membership_enabled,
        )),
        None,
    );
    let scope = conversation_id.map_or_else(
        || ProjectScope::project(project_id),
        |conversation_id| ProjectScope::thread(project_id, conversation_id),
    );
    Ok(matches!(
        authorize(
            &subject,
            Action::RealtimeSubscribe,
            Resource::Project(scope),
        ),
        Decision::Allow
    ))
}

fn parse_actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("unknown Actor kind")),
    }
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

pub fn outbox_worker_id() -> String {
    format!("outbox:{}:{}", process::id(), Uuid::now_v7().simple())
}

/// Publishes committed outbox rows until shutdown is requested.
///
/// Claim transactions only hold a short lease. Broadcasting happens after
/// the claim commits, and `published_at` is recorded in a later transaction.
/// A crash between those operations can therefore redeliver the stable event
/// ID, which is the intended at-least-once contract.
pub async fn run_outbox_publisher(
    pool: PgPool,
    realtime: RealtimeHub,
    worker_id: String,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), AppError> {
    if !valid_worker_id(&worker_id) {
        return Err(AppError::Internal("invalid outbox worker identity"));
    }

    let mut poll = interval(OUTBOX_POLL_INTERVAL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = poll.tick() => {
                if let Err(error) = publish_ready_outbox_batch(&pool, &realtime, &worker_id).await {
                    tracing::error!(
                        error_category = error.log_category(),
                        "outbox publisher could not claim a batch"
                    );
                }
            }
        }
    }
    tracing::info!("outbox publisher stopped");
    Ok(())
}

fn valid_worker_id(worker_id: &str) -> bool {
    !worker_id.is_empty()
        && worker_id.chars().count() <= outbox::MAX_OUTBOX_WORKER_ID_CHARACTERS
        && worker_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
}

async fn publish_ready_outbox_batch(
    pool: &PgPool,
    realtime: &RealtimeHub,
    worker_id: &str,
) -> Result<usize, AppError> {
    let mut transaction = pool.begin().await?;
    let claims = outbox::claim_ready(
        &mut transaction,
        worker_id,
        OUTBOX_BATCH_SIZE,
        OUTBOX_LEASE_SECONDS,
    )
    .await?;
    transaction.commit().await?;
    let claim_count = claims.len();

    for claim in claims {
        let envelope = match claim.envelope() {
            Ok(envelope) => envelope,
            Err(_) => {
                record_outbox_failure(pool, &claim, worker_id, OutboxFailureKind::MalformedEvent)
                    .await;
                continue;
            }
        };

        if let Err(error) = OutboxPublisher::publish(realtime, &envelope) {
            tracing::warn!(
                event_id = %claim.event_id(),
                event_cursor = claim.event_cursor(),
                error_category = %error,
                "outbox event could not be published"
            );
            record_outbox_failure(pool, &claim, worker_id, OutboxFailureKind::Serialization).await;
            continue;
        }

        let mut transaction = match pool.begin().await {
            Ok(transaction) => transaction,
            Err(error) => {
                tracing::error!(
                    event_id = %claim.event_id(),
                    event_cursor = claim.event_cursor(),
                    error_category = AppError::from(error).log_category(),
                    "outbox publication acknowledgement could not start"
                );
                continue;
            }
        };
        match outbox::mark_published(&mut transaction, claim.event_id(), worker_id).await {
            Ok(()) => {
                if let Err(error) = transaction.commit().await {
                    tracing::error!(
                        event_id = %claim.event_id(),
                        event_cursor = claim.event_cursor(),
                        error_category = AppError::from(error).log_category(),
                        "outbox publication acknowledgement could not commit"
                    );
                    record_outbox_failure(
                        pool,
                        &claim,
                        worker_id,
                        OutboxFailureKind::PublicationUnavailable,
                    )
                    .await;
                }
            }
            Err(error) => {
                tracing::error!(
                    event_id = %claim.event_id(),
                    event_cursor = claim.event_cursor(),
                    error_category = error.log_category(),
                    "outbox publication acknowledgement failed"
                );
                let _ = transaction.rollback().await;
                record_outbox_failure(
                    pool,
                    &claim,
                    worker_id,
                    OutboxFailureKind::PublicationUnavailable,
                )
                .await;
            }
        }
    }

    Ok(claim_count)
}

async fn record_outbox_failure(
    pool: &PgPool,
    claim: &outbox::ClaimedOutboxEvent,
    worker_id: &str,
    failure: OutboxFailureKind,
) {
    let mut transaction = match pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::error!(
                event_id = %claim.event_id(),
                event_cursor = claim.event_cursor(),
                error_category = AppError::from(error).log_category(),
                "outbox failure record could not start"
            );
            return;
        }
    };
    match outbox::record_failure(&mut transaction, claim, worker_id, failure).await {
        Ok(()) => {
            if let Err(error) = transaction.commit().await {
                tracing::error!(
                    event_id = %claim.event_id(),
                    event_cursor = claim.event_cursor(),
                    error_category = AppError::from(error).log_category(),
                    "outbox failure record could not commit"
                );
            }
        }
        Err(error) => {
            tracing::error!(
                event_id = %claim.event_id(),
                event_cursor = claim.event_cursor(),
                error_category = error.log_category(),
                "outbox failure record failed"
            );
            let _ = transaction.rollback().await;
        }
    }
}

async fn close_socket(socket: &mut WebSocket) {
    let _ = socket.send(Message::Close(None)).await;
}

async fn close_socket_for_resync(socket: &mut WebSocket) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: close_code::RESTART,
            reason: REALTIME_RESYNC_CLOSE_REASON.into(),
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
            realtime_policy_allows(
                actor_id,
                "human",
                "viewer",
                true,
                event.project_id,
                event.conversation_id,
            )
            .expect("viewer policy")
        );
        assert!(
            realtime_policy_allows(
                actor_id,
                "external_agent",
                "member",
                true,
                event.project_id,
                event.conversation_id,
            )
            .expect("external member policy")
        );
        assert!(
            !realtime_policy_allows(
                actor_id,
                "human",
                "owner",
                false,
                event.project_id,
                event.conversation_id,
            )
            .expect("disabled membership policy")
        );
        assert!(
            !realtime_policy_allows(
                actor_id,
                "execution_worker",
                "worker",
                true,
                event.project_id,
                event.conversation_id,
            )
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
        let delivery = RealtimeDelivery::Durable { envelope };

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

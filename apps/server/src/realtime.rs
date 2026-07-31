use std::time::Duration;

use axum::{
    extract::{
        State, WebSocketUpgrade,
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

use crate::{
    AppState,
    auth::{AuthenticatedActor, load_authenticated_actor, origin_is_allowed},
    error::AppError,
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
};

const SESSION_REVALIDATION_INTERVAL: Duration = Duration::from_secs(15);
const REALTIME_RESYNC_CLOSE_REASON: &str = "realtime lag; reconnect and refetch";

#[derive(Clone, Debug, Serialize)]
pub struct RealtimeEvent {
    pub event_id: Uuid,
    pub kind: &'static str,
    pub project_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

#[derive(Clone)]
pub struct RealtimeHub {
    sender: broadcast::Sender<RealtimeEvent>,
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
        let _ = self.sender.send(event);
    }

    pub fn revoke_session(&self, session_id: Uuid) {
        let _ = self.session_revocations.send(session_id);
    }

    fn subscribe(&self) -> broadcast::Receiver<RealtimeEvent> {
        self.sender.subscribe()
    }

    fn subscribe_session_revocations(&self) -> broadcast::Receiver<Uuid> {
        self.session_revocations.subscribe()
    }
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
    mut receiver: broadcast::Receiver<RealtimeEvent>,
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
                            event_id = %event.event_id,
                            project_id = %event.project_id,
                            "realtime authorization failed"
                        );
                        close_socket(&mut socket).await;
                        break;
                    }
                }
                let Ok(json) = serde_json::to_string(&event) else {
                    continue;
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
    event: &RealtimeEvent,
) -> Result<bool, AppError> {
    let membership = sqlx::query_as::<_, RealtimeMembership>(
        r#"
        SELECT pm.role, pm.enabled
        FROM project_memberships pm
        WHERE pm.actor_id = $1
          AND pm.project_id = $2
          AND (
              $3::uuid IS NULL
              OR EXISTS (
                  SELECT 1
                  FROM conversations c
                  WHERE c.id = $3
                    AND c.project_id = pm.project_id
              )
          )
        "#,
    )
    .bind(principal.actor_id)
    .bind(event.project_id)
    .bind(event.conversation_id)
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
        event,
    )
}

fn realtime_policy_allows(
    actor_id: Uuid,
    actor_kind: &str,
    role: &str,
    membership_enabled: bool,
    event: &RealtimeEvent,
) -> Result<bool, AppError> {
    let subject = Subject::new(
        actor_id,
        parse_actor_kind(actor_kind)?,
        true,
        Some(Membership::project(
            event.project_id,
            parse_membership_role(role)?,
            membership_enabled,
        )),
        None,
    );
    let scope = event.conversation_id.map_or_else(
        || ProjectScope::project(event.project_id),
        |conversation_id| ProjectScope::thread(event.project_id, conversation_id),
    );
    Ok(matches!(
        authorize(&subject, Action::ProjectView, Resource::Project(scope)),
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
    use uuid::Uuid;

    use super::{RealtimeEvent, RealtimeHub, realtime_policy_allows};
    use crate::{auth::origin_is_allowed, config::AppConfig};

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
    fn realtime_delivery_uses_project_view_policy_for_each_event() {
        let event = RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "timeline.updated",
            project_id: Uuid::now_v7(),
            conversation_id: Some(Uuid::now_v7()),
            payload: serde_json::json!({}),
        };
        let actor_id = Uuid::now_v7();

        assert!(
            realtime_policy_allows(actor_id, "human", "viewer", true, &event)
                .expect("viewer policy")
        );
        assert!(
            realtime_policy_allows(actor_id, "external_agent", "member", true, &event)
                .expect("external member policy")
        );
        assert!(
            !realtime_policy_allows(actor_id, "human", "owner", false, &event)
                .expect("disabled membership policy")
        );
        assert!(
            !realtime_policy_allows(actor_id, "execution_worker", "worker", true, &event)
                .expect("undelegated Worker policy")
        );
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

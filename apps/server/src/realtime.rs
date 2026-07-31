use axum::{
    extract::{State, WebSocketUpgrade, ws::Message},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::AppState;

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
}

impl RealtimeHub {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn publish(&self, event: RealtimeEvent) {
        let _ = self.sender.send(event);
    }

    fn subscribe(&self) -> broadcast::Receiver<RealtimeEvent> {
        self.sender.subscribe()
    }
}

pub async fn websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !origin_is_allowed(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut receiver = state.realtime.subscribe();
    upgrade
        .on_upgrade(move |mut socket| async move {
            while let Ok(event) = receiver.recv().await {
                let Ok(json) = serde_json::to_string(&event) else {
                    continue;
                };
                if socket.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        })
        .into_response()
}

fn origin_is_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    if state.config.environment.is_development() {
        return matches!(
            origin,
            "http://127.0.0.1:3100"
                | "http://localhost:3100"
                | "http://127.0.0.1:5173"
                | "http://localhost:5173"
        );
    }
    origin == format!("http://{}", state.config.server_addr)
        || origin == format!("https://{}", state.config.server_addr)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use axum::http::{HeaderMap, HeaderValue};
    use sqlx::postgres::PgPoolOptions;

    use super::{RealtimeHub, origin_is_allowed};
    use crate::{AppState, config::AppConfig};

    #[tokio::test]
    async fn development_websocket_origin_is_loopback_only() {
        let config = AppConfig::from_values(&BTreeMap::from([
            (
                "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
                "postgres://example.invalid/db".to_owned(),
            ),
            ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        ]))
        .expect("config");
        let state = AppState {
            config: Arc::new(config),
            pool: PgPoolOptions::new()
                .connect_lazy("postgres://example.invalid/db")
                .expect("lazy pool"),
            realtime: RealtimeHub::new(8),
        };
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:5173"));
        assert!(origin_is_allowed(&state, &headers));
        headers.insert("origin", HeaderValue::from_static("https://example.com"));
        assert!(!origin_is_allowed(&state, &headers));
    }
}

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
};
use rmcp::{
    ServerHandler,
    handler::server::wrapper::{Json, Parameters},
    schemars, tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub const MCP_PATH: &str = "/api/v1/mcp";

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FakeWorkItem {
    pub id: String,
    pub status: String,
    pub version: u64,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct GetFakeWorkItem {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateFakeWorkItem {
    pub id: String,
    pub expected_version: u64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum GetResult {
    Found { item: FakeWorkItem },
    NotFound { id: String },
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum UpdateResult {
    Updated {
        item: FakeWorkItem,
        changed_fields: Vec<String>,
        idempotent_retry: bool,
    },
    Conflict {
        current: FakeWorkItem,
        changed_fields: Vec<String>,
    },
    NotFound {
        id: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct FakeStore {
    records: Arc<RwLock<HashMap<String, FakeWorkItem>>>,
}

impl FakeStore {
    pub async fn seeded() -> Self {
        let store = Self::default();
        store.records.write().await.insert(
            "demo-1".to_owned(),
            FakeWorkItem {
                id: "demo-1".to_owned(),
                status: "ready".to_owned(),
                version: 1,
            },
        );
        store
    }

    pub async fn get(&self, id: &str) -> GetResult {
        match self.records.read().await.get(id).cloned() {
            Some(item) => GetResult::Found { item },
            None => GetResult::NotFound { id: id.to_owned() },
        }
    }

    pub async fn update(&self, id: &str, expected_version: u64, status: &str) -> UpdateResult {
        let mut records = self.records.write().await;
        let Some(current) = records.get_mut(id) else {
            return UpdateResult::NotFound { id: id.to_owned() };
        };

        if expected_version == current.version {
            let changed_fields = if current.status == status {
                Vec::new()
            } else {
                current.status = status.to_owned();
                current.version += 1;
                vec!["status".to_owned(), "version".to_owned()]
            };
            return UpdateResult::Updated {
                item: current.clone(),
                changed_fields,
                idempotent_retry: false,
            };
        }

        if expected_version.checked_add(1) == Some(current.version) && current.status == status {
            return UpdateResult::Updated {
                item: current.clone(),
                changed_fields: Vec::new(),
                idempotent_retry: true,
            };
        }

        let mut changed_fields = vec!["version".to_owned()];
        if current.status != status {
            changed_fields.push("status".to_owned());
        }
        UpdateResult::Conflict {
            current: current.clone(),
            changed_fields,
        }
    }
}

#[derive(Clone, Debug)]
pub struct McpSpike {
    store: FakeStore,
}

impl McpSpike {
    pub fn new(store: FakeStore) -> Self {
        Self { store }
    }
}

#[tool_router]
impl McpSpike {
    #[tool(
        description = "Read a development-only fake work item by id",
        annotations(
            title = "Get fake work item",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_fake_work_item(
        &self,
        Parameters(GetFakeWorkItem { id }): Parameters<GetFakeWorkItem>,
    ) -> Json<GetResult> {
        Json(self.store.get(&id).await)
    }

    #[tool(
        description = "Update a development-only fake work item with optimistic concurrency",
        annotations(
            title = "Update fake work item",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn update_fake_work_item(
        &self,
        Parameters(UpdateFakeWorkItem {
            id,
            expected_version,
            status,
        }): Parameters<UpdateFakeWorkItem>,
    ) -> Json<UpdateResult> {
        Json(self.store.update(&id, expected_version, &status).await)
    }
}

#[tool_handler(
    name = "agent-workspace-m0-spike",
    version = "0.0.1",
    instructions = "Development-only fake work item contract spike"
)]
impl ServerHandler for McpSpike {}

#[derive(Clone)]
struct BearerAuth {
    expected_digest: [u8; 32],
}

impl BearerAuth {
    fn new(token: &str) -> Result<Self, McpSpikeError> {
        if token.len() < 32 {
            return Err(McpSpikeError::WeakToken);
        }
        Ok(Self {
            expected_digest: Sha256::digest(token.as_bytes()).into(),
        })
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(candidate) = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return false;
        };
        let candidate_digest: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        bool::from(candidate_digest.ct_eq(&self.expected_digest))
    }
}

#[derive(Debug, Error)]
pub enum McpSpikeError {
    #[error("MCP bearer token must contain at least 32 characters")]
    WeakToken,
}

pub fn router(
    store: FakeStore,
    bearer_token: &str,
    cancellation_token: CancellationToken,
) -> Result<Router, McpSpikeError> {
    let auth = BearerAuth::new(bearer_token)?;
    let handler = McpSpike::new(store);
    let service: StreamableHttpService<McpSpike, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Default::default(),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(["localhost", "127.0.0.1"])
            .with_legacy_session_mode(true)
            .with_json_response(true)
            .with_max_request_body_bytes(1024 * 1024)
            .with_cancellation_token(cancellation_token),
    );

    Ok(Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(auth, require_bearer)))
}

pub fn loopback_address(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

async fn require_bearer(
    State(auth): State<BearerAuth>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if !auth.authorized(request.headers()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::{FakeStore, GetResult, McpSpikeError, UpdateResult};

    #[tokio::test]
    async fn reads_and_updates_with_optimistic_concurrency() {
        let store = FakeStore::seeded().await;
        assert!(matches!(
            store.get("demo-1").await,
            GetResult::Found { item } if item.version == 1
        ));

        let updated = store.update("demo-1", 1, "done").await;
        assert!(matches!(
            updated,
            UpdateResult::Updated {
                item,
                idempotent_retry: false,
                ..
            } if item.version == 2 && item.status == "done"
        ));

        let retry = store.update("demo-1", 1, "done").await;
        assert!(matches!(
            retry,
            UpdateResult::Updated {
                idempotent_retry: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn conflict_returns_current_record_and_changed_fields() {
        let store = FakeStore::seeded().await;
        let conflict = store.update("demo-1", 9, "blocked").await;
        assert!(matches!(
            conflict,
            UpdateResult::Conflict {
                current,
                changed_fields
            } if current.version == 1
                && changed_fields == vec!["version".to_owned(), "status".to_owned()]
        ));
    }

    #[tokio::test]
    async fn bearer_auth_is_required_for_initialize() {
        let token = "m0-test-token-with-at-least-thirty-two-characters";
        let cancellation = tokio_util::sync::CancellationToken::new();
        let app = super::router(FakeStore::seeded().await, token, cancellation.clone())
            .expect("test token is high entropy");
        let listener = tokio::net::TcpListener::bind(super::loopback_address(0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("read listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(cancellation.cancelled_owned())
                .await
        });
        let client = reqwest::Client::new();
        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.0.1"}
            }
        });
        let url = format!("http://{address}{}", super::MCP_PATH);

        let unauthorized = client
            .post(&url)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(&initialize)
            .send()
            .await
            .expect("unauthorized request completes");
        assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);

        let authorized = client
            .post(&url)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .bearer_auth(token)
            .json(&initialize)
            .send()
            .await
            .expect("authorized request completes");
        assert!(authorized.status().is_success());
        let authorized_status = authorized.status();
        let session_id = authorized
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("legacy handshake returns a session id")
            .to_owned();
        let initialize_body = authorized
            .text()
            .await
            .expect("initialize response has a body");
        assert!(
            !initialize_body.is_empty(),
            "initialize response unexpectedly had an empty body at status {authorized_status}"
        );
        let initialize_result = parse_protocol_body(&initialize_body);
        assert_eq!(
            initialize_result.pointer("/result/protocolVersion"),
            Some(&serde_json::json!("2025-06-18"))
        );

        let initialized = client
            .post(&url)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header("mcp-session-id", &session_id)
            .header("mcp-protocol-version", "2025-06-18")
            .bearer_auth(token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .send()
            .await
            .expect("initialized notification completes");
        assert_eq!(initialized.status(), reqwest::StatusCode::ACCEPTED);

        let tools = client
            .post(&url)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header("mcp-session-id", &session_id)
            .header("mcp-protocol-version", "2025-06-18")
            .bearer_auth(token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }))
            .send()
            .await
            .expect("tools/list request completes");
        assert!(tools.status().is_success());
        let tools_status = tools.status();
        let tools_body = tools.text().await.expect("tool list response has a body");
        assert!(
            !tools_body.is_empty(),
            "tools/list unexpectedly had an empty body at status {tools_status}"
        );
        let tools = parse_protocol_body(&tools_body);
        let listed = tools
            .pointer("/result/tools")
            .and_then(serde_json::Value::as_array)
            .expect("tool list is present");
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|tool| {
            tool.get("name") == Some(&serde_json::json!("get_fake_work_item"))
                && tool.pointer("/annotations/readOnlyHint") == Some(&serde_json::json!(true))
        }));

        server.abort();
    }

    fn parse_protocol_body(body: &str) -> serde_json::Value {
        let json = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| !line.trim().is_empty())
            .unwrap_or(body);
        serde_json::from_str(json).expect("protocol response contains JSON")
    }

    #[test]
    fn weak_token_is_rejected() {
        assert!(matches!(
            super::router(
                FakeStore::default(),
                "short",
                tokio_util::sync::CancellationToken::new()
            ),
            Err(McpSpikeError::WeakToken)
        ));
    }
}

//! K5.1: two or more full server instances on one test database, each with
//! its own state, pool, listener and loops, as production runs them. Stop
//! one gracefully (`stop`) or as a crash would (`abort`: nothing released).
//! Requests are the same `Browser::request`s the in-process tests build,
//! sent over each instance's real listener.
#![allow(dead_code)]

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    server::{RunningServer, Server},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use uuid::Uuid;

use crate::{
    agent_interface::{Browser, PASSWORD},
    support::db::{Db, db},
};

/// A browser's realtime socket.
pub(crate) type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One running instance.
pub(crate) struct Instance {
    pub(crate) name: &'static str,
    running: Option<RunningServer>,
    base: String,
    client: reqwest::Client,
}

impl Instance {
    pub(crate) fn state(&self) -> &AppState {
        self.running.as_ref().expect("running").state()
    }

    /// Every event this instance's tail hands on, from now.
    pub(crate) fn tail(
        &self,
    ) -> tokio::sync::broadcast::Receiver<agent_workspace_server::realtime::tail::Tailed> {
        self.running.as_ref().expect("running").subscribe_tail()
    }

    /// This instance's leases and heartbeat.
    pub(crate) fn cluster(&self) -> &agent_workspace_server::cluster::Cluster {
        self.running.as_ref().expect("running").cluster()
    }

    /// Send a request built for the in-process app to this instance's
    /// listener.
    pub(crate) async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let (status, _, body) = self.send_raw(request).await;
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()))
        };
        (status, value)
    }

    /// The status, headers and body of one request.
    pub(crate) async fn send_raw(
        &self,
        request: Request<Body>,
    ) -> (StatusCode, reqwest::header::HeaderMap, Vec<u8>) {
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, 10_000_000).await.expect("request body");
        let path = parts
            .uri
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_owned();
        let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()).expect("method");
        let mut outgoing = self.client.request(method, format!("{}{path}", self.base));
        for (name, value) in &parts.headers {
            outgoing = outgoing.header(name.as_str(), value.as_bytes());
        }
        let response = outgoing.body(body.to_vec()).send().await.expect("response");
        let status = StatusCode::from_u16(response.status().as_u16()).expect("status");
        let headers = response.headers().clone();
        let bytes = response.bytes().await.expect("response body").to_vec();
        (status, headers, bytes)
    }

    /// Sign in as `login_name` on this instance.
    pub(crate) async fn login(&self, login_name: &str) -> Browser {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("origin", "http://127.0.0.1:3100")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "loginName": login_name,
                    "password": PASSWORD,
                }))
                .expect("login body"),
            ))
            .expect("login request");
        let (status, headers, body) = self.send_raw(request).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let cookies = headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
            .collect::<Vec<_>>();
        let csrf = cookies
            .iter()
            .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
            .expect("CSRF cookie")
            .to_owned();
        Browser {
            cookie: cookies.join("; "),
            csrf,
        }
    }

    /// A browser's realtime socket on this instance.
    pub(crate) async fn socket(&self, browser: &Browser) -> Socket {
        use tokio_tungstenite::tungstenite::{
            client::IntoClientRequest,
            http::{
                HeaderValue,
                header::{COOKIE, ORIGIN},
            },
        };
        let url = format!(
            "{}/api/v1/realtime",
            self.base.replacen("http://", "ws://", 1)
        );
        let mut request = url.into_client_request().expect("socket request");
        request
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:3100"));
        request.headers_mut().insert(
            COOKIE,
            HeaderValue::from_str(&browser.cookie).expect("cookie"),
        );
        tokio_tungstenite::connect_async(request)
            .await
            .expect("socket")
            .0
    }

    /// A browser's protocol 2 socket on this instance (K5.9), having said
    /// `hello` with where it would resume from.
    pub(crate) async fn socket_v2(&self, browser: &Browser, resume_from: Option<i64>) -> Socket {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::{
            Message,
            client::IntoClientRequest,
            http::{
                HeaderValue,
                header::{COOKIE, ORIGIN},
            },
        };
        let url = format!(
            "{}/api/v1/realtime?v=2",
            self.base.replacen("http://", "ws://", 1)
        );
        let mut request = url.into_client_request().expect("socket request");
        request
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:3100"));
        request.headers_mut().insert(
            COOKIE,
            HeaderValue::from_str(&browser.cookie).expect("cookie"),
        );
        let mut socket = tokio_tungstenite::connect_async(request)
            .await
            .expect("socket")
            .0;
        let hello = serde_json::json!({
            "type": "hello",
            "protocol": 2,
            "resumeFrom": resume_from.map(|position| position.to_string()),
            "client": "test",
        });
        socket
            .send(Message::Text(hello.to_string().into()))
            .await
            .expect("hello");
        socket
    }

    /// What drains this instance before it stops (K5.7).
    pub(crate) fn drainer(&self) -> agent_workspace_server::server::Drainer {
        self.running.as_ref().expect("running").drainer()
    }

    /// `/health/ready`: its status and body.
    pub(crate) async fn ready(&self) -> (StatusCode, Value) {
        self.send(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .expect("request"),
        )
        .await
    }

    /// Stop gracefully.
    pub(crate) async fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop().await.expect("stop gracefully");
        }
    }

    /// Stop as a crash would: every task aborted, nothing released.
    pub(crate) fn abort(&mut self) {
        if let Some(running) = self.running.take() {
            running.abort();
        }
    }
}

/// The instances, the database they share and its owner.
pub(crate) struct Cluster {
    pub(crate) instances: Vec<Instance>,
    pub(crate) pool: PgPool,
    pub(crate) url: String,
    pub(crate) organization_id: Uuid,
    db: Option<Db>,
}

const NAMES: [&str; 4] = ["A", "B", "C", "D"];

/// What a transaction-mode pooler in front of PostgreSQL needs (K5.17).
pub(crate) const POOLER: [(&str, &str); 2] = [
    ("AGENT_WORKSPACE_DATABASE_STATEMENT_CACHE", "0"),
    ("AGENT_WORKSPACE_DATABASE_LISTEN_URL", "off"),
];

/// Whether this run is the pooler run: frames that only the listener
/// carries between instances (sign-outs, streamed text, module switches)
/// stay on their own instance there until the live role (K5.19).
pub(crate) fn pooler_run() -> bool {
    std::env::var("AGENT_WORKSPACE_TEST_POOLER").is_ok_and(|value| value == "1")
}

/// `count` instances (at most four) on a fresh test database with an owner
/// who signs in as `owner`. None when no database is configured.
pub(crate) async fn cluster(name: &str, count: usize) -> Option<Cluster> {
    cluster_with(name, count, &[]).await
}

/// The same, with extra configuration for every instance.
pub(crate) async fn cluster_with(
    name: &str,
    count: usize,
    extra: &[(&str, &str)],
) -> Option<Cluster> {
    assert!(count <= NAMES.len(), "keep clusters small");
    let db = db(name).await?;
    let owner = bootstrap_admin(
        &db.pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Cluster".to_owned()),
            organization_slug: Some(format!("cluster-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let mut cluster = Cluster {
        instances: Vec::new(),
        pool: db.pool.clone(),
        url: db.url.clone(),
        organization_id: owner.organization_id,
        db: Some(db),
    };
    for _ in 0..count {
        cluster.start(extra).await;
    }
    Some(cluster)
}

impl Cluster {
    /// Start one more instance; it gets the next name.
    pub(crate) async fn start(&mut self, extra: &[(&str, &str)]) -> &mut Instance {
        let name = NAMES[self.instances.len()];
        let mut values = BTreeMap::from([
            ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), self.url.clone()),
            // Stopping drains at once unless a test asks for a drain time.
            ("AGENT_WORKSPACE_DRAIN_SECONDS".to_owned(), "0".to_owned()),
        ]);
        // `AGENT_WORKSPACE_TEST_POOLER=1` runs the suite the way a
        // transaction-mode pooler needs (K5.17): no prepared statements kept
        // and no listener, so everything runs on polls.
        if pooler_run() {
            values.extend(
                POOLER
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
            );
        }
        for (key, value) in extra {
            values.insert((*key).to_owned(), (*value).to_owned());
        }
        let config = Arc::new(AppConfig::from_values(&values).expect("test configuration"));
        // Each instance has its own small pool, as separate processes do.
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .connect_with(
                agent_workspace_server::db::connect_options(&config).expect("connect options"),
            )
            .await
            .expect("instance pool");
        let server = Server::build(config, pool).await.expect("build instance");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let webhooks = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("webhook listener");
        let running = server.spawn(listener, Some(webhooks));
        let base = format!("http://{}", running.address().expect("address"));
        self.instances.push(Instance {
            name,
            running: Some(running),
            base,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("client"),
        });
        self.instances.last_mut().expect("instance")
    }

    pub(crate) fn instance(&self, index: usize) -> &Instance {
        &self.instances[index]
    }

    pub(crate) fn instance_mut(&mut self, index: usize) -> &mut Instance {
        &mut self.instances[index]
    }

    /// Stop every instance and drop the database.
    pub(crate) async fn finish(mut self) {
        for instance in &mut self.instances {
            instance.stop().await;
        }
        if let Some(db) = self.db.take() {
            db.finish().await;
        }
    }
}

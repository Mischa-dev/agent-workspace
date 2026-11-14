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
        let mut values =
            BTreeMap::from([("AGENT_WORKSPACE_DATABASE_URL".to_owned(), self.url.clone())]);
        for (key, value) in extra {
            values.insert((*key).to_owned(), (*value).to_owned());
        }
        let config = Arc::new(AppConfig::from_values(&values).expect("test configuration"));
        // Each instance has its own small pool, as separate processes do.
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .connect(&self.url)
            .await
            .expect("instance pool");
        let server = Server::build(config, pool).await.expect("build instance");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let webhooks = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("webhook listener");
        let running = server.spawn(listener, webhooks);
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

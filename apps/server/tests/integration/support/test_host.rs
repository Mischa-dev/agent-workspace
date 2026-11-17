//! A1.12: a host as a test drives it, speaking the protocol over the app's
//! routes: enrol with a code, describe itself, read its commands, fetch and
//! acknowledge them, and send heartbeats. It remembers the starts it ran
//! by start key, as a real host must, so a start offered again is answered
//! with the run it already started. A1.23 grows this into the conformance
//! suite.

use std::collections::HashMap;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::app::{Browser, send};

pub(crate) fn host_request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("aw-host-protocol", "2026-11");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("host request")
}

pub(crate) struct TestHost {
    pub(crate) app: axum::Router,
    pub(crate) id: Uuid,
    pub(crate) token: String,
    /// The last command sequence read.
    pub(crate) cursor: i64,
    /// Runs this host started, by start key: its own run reference.
    pub(crate) started: HashMap<String, String>,
    /// The enrolment's answer, whole.
    pub(crate) enrolled: Value,
}

impl TestHost {
    /// A host the workspace owns, linked with a fresh code and described
    /// with `capabilities` and `agents` (each `{"hostAgentRef": …}`).
    pub(crate) async fn enrol(
        app: &axum::Router,
        browser: &Browser,
        workspace: Uuid,
        name: &str,
        connection: Value,
    ) -> Self {
        let key = Uuid::now_v7().to_string();
        let (status, made) = send(
            app,
            browser.request(
                "POST",
                &format!("/api/v2/w/{workspace}/ops/host_enrolment_create"),
                Some(&key),
                json!({"owner": "workspace", "label": name}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{made}");
        let code = made["value"]["code"].as_str().expect("code").to_owned();
        let mut body = json!({"code": code, "host": {"name": name, "software": "test-host", "version": "1.0"},
                              "protocolVersions": ["2026-11"]});
        if let (Value::Object(body), Value::Object(connection)) = (&mut body, connection) {
            body.extend(connection);
        }
        let (status, enrolled) = send(
            app,
            host_request("POST", "/api/v1/hosts/enrol", None, Some(body)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{enrolled}");
        Self {
            app: app.clone(),
            id: enrolled["hostId"]
                .as_str()
                .expect("host")
                .parse()
                .expect("uuid"),
            token: enrolled["credential"]["token"]
                .as_str()
                .expect("token")
                .to_owned(),
            cursor: 0,
            started: HashMap::new(),
            enrolled,
        }
    }

    pub(crate) async fn call(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        send(
            &self.app,
            host_request(method, uri, Some(&self.token), body),
        )
        .await
    }

    pub(crate) async fn describe(&self, capabilities: Value, agents: Value) {
        let (status, described) = self
            .call(
                "POST",
                "/api/v1/host/describe",
                Some(json!({"software": {"name": "test-host", "version": "1.0"},
                            "capabilities": capabilities, "reportIntervalSeconds": 60,
                            "agents": agents})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{described}");
    }

    /// The commands due now, moving the cursor on.
    pub(crate) async fn poll(&mut self) -> Vec<Value> {
        let (status, list) = self
            .call(
                "GET",
                &format!("/api/v1/host/commands?after={}&wait=0", self.cursor),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        self.cursor = list["next"].as_i64().expect("next");
        list["commands"].as_array().cloned().unwrap_or_default()
    }

    pub(crate) async fn fetch(&self, command_id: &Value) -> (StatusCode, Value) {
        self.call(
            "GET",
            &format!("/api/v1/host/commands/{}", command_id.as_str().expect("id")),
            None,
        )
        .await
    }

    pub(crate) async fn ack(&self, command_id: &Value, ack: Value) -> (StatusCode, Value) {
        self.call(
            "POST",
            &format!(
                "/api/v1/host/commands/{}/ack",
                command_id.as_str().expect("id")
            ),
            Some(ack),
        )
        .await
    }

    /// Run a start as a host must: a start key seen before answers with
    /// the run already started. Returns the host's run reference and
    /// whether this call started it.
    pub(crate) fn run_start(&mut self, command: &Value) -> (String, bool) {
        let key = command["startKey"].as_str().expect("startKey").to_owned();
        if let Some(reference) = self.started.get(&key) {
            return (reference.clone(), false);
        }
        let reference = format!("hr-{}", self.started.len() + 1);
        self.started.insert(key, reference.clone());
        (reference, true)
    }

    pub(crate) async fn heartbeat(&self, runs: Value) -> Value {
        let (status, beat) = self
            .call(
                "POST",
                "/api/v1/host/heartbeat",
                Some(json!({"runs": runs})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{beat}");
        beat
    }
}

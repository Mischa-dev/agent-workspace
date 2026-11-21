//! The host's client: the command channel, heartbeats, enrolment, describe,
//! notices and run reports. Every call is a plain HTTP request; how it is
//! carried is a [`Transport`], over the network ([`ReqwestTransport`]) or,
//! for a host running inside the server's own process, handed to the
//! server's router without a socket. Either way the bytes are the public
//! protocol's.

use std::{fmt, future::Future, pin::Pin, sync::Arc, time::Duration};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    Ack, AckReply, Command, CommandList, Description, EnrolRequest, Enrolled, Heartbeat,
    MAX_WAIT_SECONDS, Notice, PROTOCOL_HEADER, PROTOCOL_VERSION, WindowsReport,
    WorkspaceCredential,
};

/// Why a call failed.
#[derive(Debug)]
pub enum ClientError {
    /// It never got an answer (connection, timeout, a body that isn't
    /// what the protocol says).
    Http(String),
    /// The server answered with this status and body.
    Refused(u16, String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(formatter, "{error}"),
            Self::Refused(status, body) => write!(formatter, "{status} {body}"),
        }
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.to_string())
    }
}

/// One request as the client sends it: a path on the server's origin.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: &'static str,
    /// The path and query, such as `/api/v1/host/commands?after=0&wait=0`.
    pub path: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Vec<u8>>,
}

/// The server's answer.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// The answer's headers, names in lowercase.
    pub headers: Vec<(String, String)>,
}

impl HttpResponse {
    /// A header's value, by its lowercase name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

pub type TransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HttpResponse, ClientError>> + Send + 'a>>;

/// How a request reaches the server.
pub trait Transport: Send + Sync + fmt::Debug {
    fn send(&self, request: HttpRequest) -> TransportFuture<'_>;
}

/// Over the network, to `base` (the server's origin).
#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    base: String,
    http: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new(base: impl Into<String>) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(MAX_WAIT_SECONDS + 15))
            .build()?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_owned(),
            http,
        })
    }
}

impl Transport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|error| ClientError::Http(error.to_string()))?;
            let mut builder = self
                .http
                .request(method, format!("{}{}", self.base, request.path));
            for (name, value) in request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder
                    .header("content-type", "application/json")
                    .body(body);
            }
            let response = builder.send().await?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
                })
                .collect();
            let body = response.bytes().await?.to_vec();
            Ok(HttpResponse {
                status,
                body,
                headers,
            })
        })
    }
}

fn read<T: DeserializeOwned>(response: HttpResponse) -> Result<T, ClientError> {
    if !(200..300).contains(&response.status) {
        return Err(ClientError::Refused(
            response.status,
            String::from_utf8_lossy(&response.body).into_owned(),
        ));
    }
    serde_json::from_slice(&response.body).map_err(|error| ClientError::Http(error.to_string()))
}

fn json_body(value: &impl Serialize) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(value).map_err(|error| ClientError::Http(error.to_string()))
}

/// A host's client for the command channel and heartbeats.
#[derive(Clone, Debug)]
pub struct HostClient {
    base: String,
    token: String,
    transport: Arc<dyn Transport>,
}

impl HostClient {
    /// `base` is the server's origin, such as `https://work.example`.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Result<Self, ClientError> {
        let base = base.into().trim_end_matches('/').to_owned();
        let transport = Arc::new(ReqwestTransport::new(base.clone())?);
        Ok(Self {
            base,
            token: token.into(),
            transport,
        })
    }

    /// A client whose calls go through `transport` (such as the server's
    /// router, for a host inside the server's process). `base` is still
    /// the origin the host names in what it reports.
    pub fn with_transport(
        base: impl Into<String>,
        token: impl Into<String>,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            token: token.into(),
            transport,
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        path: String,
        body: Option<Vec<u8>>,
    ) -> Result<T, ClientError> {
        let response = self
            .transport
            .send(HttpRequest {
                method,
                path,
                headers: vec![
                    ("authorization", format!("Bearer {}", self.token)),
                    (PROTOCOL_HEADER, PROTOCOL_VERSION.to_owned()),
                ],
                body,
            })
            .await?;
        read(response)
    }

    /// Commands after `after`, waiting up to `wait` seconds for one.
    pub async fn commands(&self, after: i64, wait: u64) -> Result<CommandList, ClientError> {
        let wait = wait.min(MAX_WAIT_SECONDS);
        self.call(
            "GET",
            format!("/api/v1/host/commands?after={after}&wait={wait}"),
            None,
        )
        .await
    }

    /// The whole command, with any credential minted for it.
    pub async fn command(&self, command_id: Uuid) -> Result<Command, ClientError> {
        self.call("GET", format!("/api/v1/host/commands/{command_id}"), None)
            .await
    }

    pub async fn ack(&self, command_id: Uuid, ack: &Ack) -> Result<AckReply, ClientError> {
        self.call(
            "POST",
            format!("/api/v1/host/commands/{command_id}/ack"),
            Some(json_body(ack)?),
        )
        .await
    }

    pub async fn heartbeat(&self, heartbeat: &Heartbeat) -> Result<Value, ClientError> {
        self.call(
            "POST",
            "/api/v1/host/heartbeat".to_owned(),
            Some(json_body(heartbeat)?),
        )
        .await
    }

    /// Link a host with a one-time code; the answer's credential is the
    /// token for every call after.
    pub async fn enrol(base: &str, request: &EnrolRequest) -> Result<Enrolled, ClientError> {
        let transport = ReqwestTransport::new(base)?;
        Self::enrol_through(&transport, request).await
    }

    /// [`Self::enrol`] through a transport of the caller's.
    pub async fn enrol_through(
        transport: &dyn Transport,
        request: &EnrolRequest,
    ) -> Result<Enrolled, ClientError> {
        let response = transport
            .send(HttpRequest {
                method: "POST",
                path: "/api/v1/hosts/enrol".to_owned(),
                headers: vec![(PROTOCOL_HEADER, PROTOCOL_VERSION.to_owned())],
                body: Some(json_body(request)?),
            })
            .await?;
        read(response)
    }

    /// The server's origin this client calls.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn describe(&self, description: &Description) -> Result<Value, ClientError> {
        self.call(
            "POST",
            "/api/v1/host/describe".to_owned(),
            Some(json_body(description)?),
        )
        .await
    }

    /// Report the plan windows of accounts this host described.
    pub async fn report_windows(&self, report: &WindowsReport) -> Result<Value, ClientError> {
        self.call(
            "POST",
            "/api/v1/host/windows".to_owned(),
            Some(json_body(report)?),
        )
        .await
    }

    /// The built-in host's harnesses, for the server to offer and check
    /// launches against.
    pub async fn builtin_harnesses(
        &self,
        harnesses: &crate::BuiltinHarnesses,
    ) -> Result<Value, ClientError> {
        self.call(
            "PUT",
            "/api/v1/host/builtin/harnesses".to_owned(),
            Some(json_body(harnesses)?),
        )
        .await
    }

    /// Adopt a run this host started itself (A2.26); fetch the returned
    /// command for the run's credential.
    pub async fn adopt_run(&self, run: &crate::AdoptRun) -> Result<crate::AdoptedRun, ClientError> {
        self.call(
            "POST",
            "/api/v1/host/runs".to_owned(),
            Some(json_body(run)?),
        )
        .await
    }

    pub async fn notice(&self, notice: &Notice) -> Result<Value, ClientError> {
        self.call(
            "POST",
            "/api/v1/host/notices".to_owned(),
            Some(json_body(notice)?),
        )
        .await
    }

    /// A reporter for one run, with the credential its start (or another
    /// command about it) carried.
    pub fn reporter(&self, run_id: Uuid, credential: &WorkspaceCredential) -> RunReporter {
        RunReporter {
            token: credential.token.clone(),
            run_id,
            transport: self.transport.clone(),
        }
    }
}

/// How a host reports on a run: the run operations (`run_activity_report`,
/// `run_usage_report`, `run_result_report`, `run_update`) on the public
/// API, as the run's agent. Each report carries a key, so sending one again
/// changes nothing.
#[derive(Clone, Debug)]
pub struct RunReporter {
    token: String,
    run_id: Uuid,
    transport: Arc<dyn Transport>,
}

impl RunReporter {
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }

    /// Any operation as the run's agent; `input` gets the run's id.
    pub async fn operation(
        &self,
        name: &str,
        mut input: Value,
        key: &str,
    ) -> Result<Value, ClientError> {
        if let Value::Object(fields) = &mut input {
            fields.insert("runId".to_owned(), Value::String(self.run_id.to_string()));
        }
        self.call_as_agent(name, &input, key).await
    }

    /// Any operation as the run's agent, with `input` as it is (for an
    /// operation that takes no run id, such as sending a message).
    pub async fn call_as_agent(
        &self,
        name: &str,
        input: &Value,
        key: &str,
    ) -> Result<Value, ClientError> {
        let response = self
            .transport
            .send(HttpRequest {
                method: "POST",
                path: format!("/api/v2/ops/{name}"),
                headers: vec![
                    ("authorization", format!("Bearer {}", self.token)),
                    ("idempotency-key", key.to_owned()),
                ],
                body: Some(json_body(input)?),
            })
            .await?;
        read(response)
    }

    /// Activities, each `{"id", "kind", "payload"?}`: `run_started`,
    /// `text_message`, `interrupt` (a request), and so on.
    pub async fn activities(
        &self,
        activities: Vec<Value>,
        key: &str,
    ) -> Result<Value, ClientError> {
        self.operation(
            "run_activity_report",
            serde_json::json!({ "activities": activities }),
            key,
        )
        .await
    }

    /// A request of the host's own as the run's agent: `headers` and the
    /// run's credential. For calls the reporter has no method for, such as
    /// the agent's MCP tools.
    pub async fn send_as_agent(
        &self,
        method: &'static str,
        path: &str,
        mut headers: Vec<(&'static str, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<HttpResponse, ClientError> {
        headers.push(("authorization", format!("Bearer {}", self.token)));
        self.transport
            .send(HttpRequest {
                method,
                path: path.to_owned(),
                headers,
                body,
            })
            .await
    }

    /// A1.25: one model turn through the server's model gateway, with the
    /// workspace's model and key; the server records its tokens on the run.
    pub async fn model_turn(
        &self,
        turn: &crate::ModelTurn,
    ) -> Result<crate::ModelReply, ClientError> {
        let response = self
            .send_as_agent(
                "POST",
                &format!("/api/v1/host/runs/{}/model", self.run_id),
                Vec::new(),
                Some(json_body(turn)?),
            )
            .await?;
        read(response)
    }

    /// Words of the agent's reply as they arrive (see [`crate::StreamDelta`]).
    pub async fn stream(&self, delta: &crate::StreamDelta) -> Result<(), ClientError> {
        let response = self
            .send_as_agent(
                "POST",
                &format!("/api/v1/host/runs/{}/stream", self.run_id),
                Vec::new(),
                Some(json_body(delta)?),
            )
            .await?;
        read::<Value>(response).map(drop)
    }

    /// Tokens used, under a key of the host's own.
    pub async fn usage(
        &self,
        report_key: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<Value, ClientError> {
        self.operation(
            "run_usage_report",
            serde_json::json!({ "reportKey": report_key, "model": model,
                                "inputTokens": input_tokens, "outputTokens": output_tokens }),
            &format!("usage-{report_key}"),
        )
        .await
    }

    /// The run's state (`working`, `done`, `failed`, `cancelled`, ...) and
    /// anything else `run_update` takes.
    pub async fn update(&self, mut fields: Value, key: &str) -> Result<Value, ClientError> {
        if !fields.is_object() {
            fields = serde_json::json!({});
        }
        self.operation("run_update", fields, key).await
    }
}

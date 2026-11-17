//! The host protocol (A1 section 3.9) as a host and the server both speak
//! it: the wire types of the command channel and the heartbeat, the signing
//! of pushed commands (Standard Webhooks), and a small client a host uses
//! to read its commands, fetch each, acknowledge it and send heartbeats.
//!
//! A host learns of a command by long-poll (`GET /api/v1/host/commands`),
//! by a stream on the same address, or from a signed push; either way it
//! fetches the whole command (`GET /api/v1/host/commands/{id}`), where any
//! short-lived credential is minted, and acknowledges it once
//! (`POST /api/v1/host/commands/{id}/ack`). A `start` it has seen before
//! (the same `startKey`) is acknowledged again with the run it started,
//! never started twice.

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// The header every host call carries.
pub const PROTOCOL_HEADER: &str = "aw-host-protocol";
/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: &str = "2026-11";
/// The longest a long-poll waits, in seconds.
pub const MAX_WAIT_SECONDS: u64 = 30;

/// What a command asks a host to do.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Start,
    Cancel,
    Steer,
    Answer,
    Inspect,
    Action,
}

impl CommandKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Cancel => "cancel",
            Self::Steer => "steer",
            Self::Answer => "answer",
            Self::Inspect => "inspect",
            Self::Action => "action",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Start,
            Self::Cancel,
            Self::Steer,
            Self::Answer,
            Self::Inspect,
            Self::Action,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == value)
    }
}

/// A command as the list, the stream and a push name it: enough to fetch
/// it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSummary {
    pub command_id: Uuid,
    pub seq: i64,
    pub kind: CommandKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
    /// How many times it was offered before, counting this one from 1: a
    /// re-offer of a start whose acknowledgement was lost.
    pub offer: i32,
}

/// The answer to a long-poll.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandList {
    pub commands: Vec<CommandSummary>,
    /// Pass it as `after` next time.
    pub next: i64,
}

/// A short-lived credential for the run's agent, minted when the command
/// is fetched and never stored by the server.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCredential {
    pub token: String,
    pub expires_at: String,
    pub scope: String,
}

/// A whole command, as fetched. What it carries beyond the fields below
/// depends on its kind (`agent`, `input`, `cause`, `requestedBy`,
/// `settings`, `repository` for a start; `reason` for a cancel; ...).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub command_id: Uuid,
    pub seq: i64,
    pub kind: CommandKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Uuid>,
    /// A start's key: the same on every offer of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_key: Option<String>,
    pub issued_at: String,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_credential: Option<WorkspaceCredential>,
    #[serde(flatten)]
    pub detail: Map<String, Value>,
}

/// How a host answers a command.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AckOutcome {
    /// It took the command: a start began (or had begun), a cancel, steer
    /// or answer was passed on.
    Accepted,
    /// It won't: `reason` and `message` say why.
    Refused,
    /// It doesn't know whether it started the run.
    Uncertain,
    /// An inspect or action finished (its `result`), or a cancel was
    /// carried out.
    Done,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    pub outcome: Option<AckOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_run_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// An inspect's or action's answer; for inspect, `{"known": bool,
    /// "hostRunRef"?: …, "state"?: …}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

impl Ack {
    pub fn new(outcome: AckOutcome) -> Self {
        Self {
            outcome: Some(outcome),
            ..Self::default()
        }
    }
}

/// What the server made of an acknowledgement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckReply {
    pub command_id: Uuid,
    /// The command's state now: acked, refused or uncertain.
    pub state: String,
    /// The run's state now, for a command about a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_state: Option<String>,
}

/// One run a heartbeat lists: the host still knows it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatRun {
    pub run_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_run_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<Value>,
    /// Every run the host is still doing: one it stops listing for three
    /// heartbeats is interrupted.
    #[serde(default)]
    pub runs: Vec<HeartbeatRun>,
}

/// The thin body of a pushed command: the host answers 2xx at once and
/// fetches it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub command_id: Uuid,
    pub kind: CommandKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
}

// ------------------------------------------------------- push signing

/// The key a `whsec_…` signing secret stands for.
fn signing_key(secret: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(secret.strip_prefix("whsec_").unwrap_or(secret))
        .ok()
        .filter(|key| !key.is_empty())
}

/// The `webhook-signature` value for a push: `v1,` and the base64
/// HMAC-SHA256 of `{id}.{timestamp}.{body}` (Standard Webhooks).
pub fn sign_push(secret: &str, id: &str, timestamp: i64, body: &str) -> Option<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(&signing_key(secret)?).ok()?;
    mac.update(format!("{id}.{timestamp}.{body}").as_bytes());
    Some(format!(
        "v1,{}",
        STANDARD.encode(mac.finalize().into_bytes())
    ))
}

/// Whether a push is the server's: one of the space-separated signatures
/// in `header` matches, and `timestamp` is within five minutes of `now`.
pub fn verify_push(
    secret: &str,
    id: &str,
    timestamp: i64,
    body: &str,
    header: &str,
    now: i64,
) -> bool {
    if (now - timestamp).abs() > 300 {
        return false;
    }
    let Some(expected) = sign_push(secret, id, timestamp, body) else {
        return false;
    };
    header
        .split_ascii_whitespace()
        .any(|candidate| bool::from(candidate.as_bytes().ct_eq(expected.as_bytes())))
}

/// A new push signing secret: `whsec_` and 32 random bytes in base64.
pub fn new_push_secret(random: [u8; 32]) -> String {
    format!("whsec_{}", STANDARD.encode(random))
}

// ------------------------------------------------------------- client

/// Why a call failed.
#[derive(Debug)]
pub enum ClientError {
    Http(reqwest::Error),
    /// The server answered with this status and body.
    Refused(u16, String),
}

impl From<reqwest::Error> for ClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

/// A host's client for the command channel and heartbeats.
#[derive(Clone, Debug)]
pub struct HostClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl HostClient {
    /// `base` is the server's origin, such as `https://work.example`.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(MAX_WAIT_SECONDS + 15))
            .build()?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_owned(),
            token: token.into(),
            http,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
    }

    async fn read<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T, ClientError> {
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::Refused(
                status.as_u16(),
                response.text().await.unwrap_or_default(),
            ));
        }
        Ok(response.json().await?)
    }

    /// Commands after `after`, waiting up to `wait` seconds for one.
    pub async fn commands(&self, after: i64, wait: u64) -> Result<CommandList, ClientError> {
        let wait = wait.min(MAX_WAIT_SECONDS);
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/host/commands?after={after}&wait={wait}"),
            )
            .send()
            .await?;
        Self::read(response).await
    }

    /// The whole command, with any credential minted for it.
    pub async fn command(&self, command_id: Uuid) -> Result<Command, ClientError> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/host/commands/{command_id}"),
            )
            .send()
            .await?;
        Self::read(response).await
    }

    pub async fn ack(&self, command_id: Uuid, ack: &Ack) -> Result<AckReply, ClientError> {
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/api/v1/host/commands/{command_id}/ack"),
            )
            .json(ack)
            .send()
            .await?;
        Self::read(response).await
    }

    pub async fn heartbeat(&self, heartbeat: &Heartbeat) -> Result<Value, ClientError> {
        let response = self
            .request(reqwest::Method::POST, "/api/v1/host/heartbeat")
            .json(heartbeat)
            .send()
            .await?;
        Self::read(response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_verifies_only_with_its_secret_and_in_time() {
        let secret = new_push_secret([7; 32]);
        let signature = sign_push(&secret, "msg_1", 1_700_000_000, "{}").expect("signs");
        assert!(signature.starts_with("v1,"));
        assert!(verify_push(
            &secret,
            "msg_1",
            1_700_000_000,
            "{}",
            &format!("v1,other {signature}"),
            1_700_000_100
        ));
        assert!(!verify_push(
            &new_push_secret([8; 32]),
            "msg_1",
            1_700_000_000,
            "{}",
            &signature,
            1_700_000_100
        ));
        assert!(!verify_push(
            &secret,
            "msg_1",
            1_700_000_000,
            "{\"x\":1}",
            &signature,
            1_700_000_100
        ));
        assert!(!verify_push(
            &secret,
            "msg_1",
            1_700_000_000,
            "{}",
            &signature,
            1_700_001_000
        ));
    }

    #[test]
    fn commands_read_what_the_server_writes() {
        let command: Command = serde_json::from_value(serde_json::json!({
            "commandId": "0199c000-0000-7000-8000-000000000001", "seq": 4, "kind": "start",
            "runId": "0199c000-0000-7000-8000-000000000002", "startKey": "run-1",
            "issuedAt": "2026-11-16T10:00:00Z", "expiresAt": "2026-11-17T10:00:00Z",
            "input": {"prompt": "Do it"}
        }))
        .expect("command");
        assert_eq!(command.kind, CommandKind::Start);
        assert_eq!(command.detail["input"]["prompt"], "Do it");
        assert_eq!(CommandKind::parse("inspect"), Some(CommandKind::Inspect));
    }
}

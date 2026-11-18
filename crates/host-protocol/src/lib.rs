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
//!
//! It also enrols a host, describes it, sends its notices, and reports on
//! a run (`RunReporter`) with the credential a command carried.

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

// ------------------------------------------- enrolment, describe, notices

/// Who the host is, as it enrols.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSoftware {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `POST /api/v1/hosts/enrol`: a one-time code from the workspace (or a
/// person, for a host of their own) links the host.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrolRequest {
    pub code: String,
    pub host: HostSoftware,
    pub protocol_versions: Vec<String>,
    /// `dial_out` (the host reads its commands; the default) or `push`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<Value>,
}

/// The credential a host sends from then on.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCredential {
    pub kind: String,
    pub token: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enrolled {
    pub host_id: Uuid,
    pub protocol: String,
    pub credential: HostCredential,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_signing_secret: Option<String>,
}

/// What an action needs before it runs: a yes, or a panel the host draws.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionNeeds {
    Confirm,
    Panel,
}

/// Something a person can ask the host (or one of its agents) to do; it
/// arrives as an `action` command.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAction {
    pub id: String,
    pub label: String,
    pub needs: ActionNeeds,
}

/// Something the host offers to set up, such as a new agent.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One agent the host runs.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribedAgent {
    pub host_agent_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<HostAction>,
}

/// `POST /api/v1/host/describe`: what the host is and can do. Sent after
/// enrolling, whenever it changes, and when a heartbeat's answer asks.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Description {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software: Option<Value>,
    #[serde(default)]
    pub capabilities: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_interval_seconds: Option<i32>,
    #[serde(default)]
    pub offers: Vec<Offer>,
    #[serde(default)]
    pub actions: Vec<HostAction>,
    #[serde(default)]
    pub agents: Vec<DescribedAgent>,
    /// The provider accounts the host runs on; it reports their plan
    /// windows on `POST /api/v1/host/windows`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<DescribedAccount>,
}

/// A provider account a host runs on, by the host's own reference.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DescribedAccount {
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
}

/// `POST /api/v1/host/windows`: the plan windows of described accounts.
/// A full report: windows an account no longer lists are gone.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsReport {
    pub accounts: Vec<AccountWindows>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountWindows {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default)]
    pub windows: Vec<PlanWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<Value>,
}

/// One plan window: `key` is stable for the account (lowercase letters,
/// digits and `._-`), `usedPercent` 0 to 100.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanWindow {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub used_percent: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_minutes: Option<i64>,
    #[serde(default)]
    pub limit_reached: bool,
}

/// The three things a host tells the server about one of its agents.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// Its profile changed (`profile`, with at least a `name`).
    AgentProfileChanged,
    /// It is known by another reference now (`newHostAgentRef`, `ref`).
    AgentRefChanged,
    /// It is gone: its runs are interrupted.
    AgentRetired,
}

/// `POST /api/v1/host/notices`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub kind: NoticeKind,
    pub host_agent_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_host_agent_ref: Option<String>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Notice {
    pub fn new(kind: NoticeKind, host_agent_ref: impl Into<String>) -> Self {
        Self {
            kind,
            host_agent_ref: host_agent_ref.into(),
            profile: None,
            new_host_agent_ref: None,
            reference: None,
            reason: None,
        }
    }
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

mod client;
mod model;
pub use model::{
    MAX_MESSAGES as MAX_MODEL_MESSAGES, MAX_TOOLS as MAX_MODEL_TOOLS, ModelMessage, ModelPart,
    ModelReply, ModelRole, ModelStop, ModelTool, ModelTurn,
};
pub use client::{
    ClientError, HostClient, HttpRequest, HttpResponse, ReqwestTransport, RunReporter, Transport,
    TransportFuture,
};

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

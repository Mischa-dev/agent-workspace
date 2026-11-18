//! A1.20: the A2A binding (A1 section 3.10). An agent that speaks A2A is
//! linked by its Agent Card's address and becomes a host whose connection
//! is `a2a`, with the card's agent as its one described agent. Workspace is
//! its A2A client and runs nothing of it:
//!
//! - a command for the host is sent by a job ([`COMMAND_KIND`]): a start is
//!   `message/send` with the start key as its `messageId`, Stop is
//!   `tasks/cancel`, an answer is a message to the same task, an inspect is
//!   `tasks/get`, and the result is acknowledged through the same path a
//!   protocol host's acknowledgement takes;
//! - the agent's task is looked at (`tasks/get`) by a scheduler over due
//!   rows ([`POLL_KIND`]) and each snapshot becomes run reports, made as
//!   the run's agent: status messages are its messages, `input-required`
//!   and `auth-required` open requests, artifacts become messages or
//!   results, and `completed`, `failed`, `rejected` and `canceled` end it;
//! - an answer reaching no task, or a send with no answer, is never sent
//!   twice: an unconfirmed start is `start_uncertain` and a person decides.
//!
//! A2A's JSON stays in this file. Every call goes through `outbound.rs`,
//! with no transaction held across it.

use std::time::Duration;

use agent_workspace_host_protocol::{Ack, AckOutcome};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use url::Url;
use uuid::Uuid;

use super::{HOST_COLUMNS, HostRow, HostView, Owner, PROTOCOL, protocol::HostError};
use crate::{
    AppState,
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    runs::{
        RunState,
        activities::NewActivity,
        ops::{self, Reporter, RunOpError, RunUpdate},
        store::RunRow,
    },
};

pub const COMMAND_KIND: &str = "hosts.a2a_command";
pub const POLL_KIND: &str = "hosts.a2a_poll";
/// The host's one agent: the card's.
pub const AGENT_REF: &str = "agent";
/// The well-known place of a card under an agent's address.
const CARD_PATH: &str = "/.well-known/agent-card.json";
const MAX_CARD_BYTES: usize = 64 * 1024;
const MAX_ANSWER_BYTES: usize = 1024 * 1024;
const MAX_TEXT_CHARS: usize = 20_000;
/// Tasks looked at in one pass.
const POLL_BATCH: i64 = 50;
/// How long a task row is held by the pass looking at it.
const POLL_LEASE_SECONDS: f64 = 60.0;

/// Seconds until a task is looked at again, after `polls` looks: soon at
/// first, then once a minute.
pub fn poll_after(polls: i32) -> f64 {
    match polls {
        ..=2 => 5.0,
        3..=5 => 15.0,
        6..=10 => 30.0,
        _ => 60.0,
    }
}

// ------------------------------------------------------------------ card

/// What a card says that Workspace keeps.
#[derive(Clone, Debug)]
pub struct Card {
    pub url: String,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub endpoint: String,
    pub skills: Vec<Value>,
}

fn line(value: Option<&Value>, maximum: usize) -> Option<String> {
    let text = crate::runs::ops::one_line(value?.as_str()?);
    let text = crate::run_log::redact(&text);
    (!text.is_empty()).then(|| text.chars().take(maximum).collect())
}

/// The card's address: the one given, or the well-known place under an
/// agent's bare address.
pub fn card_address(value: &str) -> Option<Url> {
    let mut url = Url::parse(value.trim()).ok()?;
    if matches!(url.path(), "" | "/") {
        url.set_path(CARD_PATH);
    }
    Some(url)
}

/// Read a card: its name and the JSON-RPC address it names (relative to
/// the card), with the same rules as any address a workspace sets.
pub fn read_card(card: &Value, card_url: &Url, trusted: &[String]) -> Result<Card, &'static str> {
    let name = line(card.get("name"), 80).ok_or("The card has no name.")?;
    let transport = card
        .get("preferredTransport")
        .and_then(Value::as_str)
        .unwrap_or("JSONRPC");
    if !transport.eq_ignore_ascii_case("JSONRPC") {
        return Err("The agent doesn't offer A2A over JSON-RPC.");
    }
    let endpoint = card
        .get("url")
        .and_then(Value::as_str)
        .and_then(|value| card_url.join(value).ok())
        .ok_or("The card names no address to call.")?;
    let endpoint = crate::outbound::validate_webhook_url(endpoint.as_str(), trusted)
        .map_err(|_| "The card's address isn't one Workspace may call.")?;
    let skills = card
        .get("skills")
        .and_then(Value::as_array)
        .map(|skills| {
            skills
                .iter()
                .take(50)
                .filter_map(|skill| {
                    Some(json!({
                        "id": line(skill.get("id"), 64)?,
                        "name": line(skill.get("name"), 80),
                        "description": line(skill.get("description"), 500),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Card {
        url: card_url.to_string(),
        name,
        description: line(card.get("description"), 500),
        version: line(card.get("version"), 40),
        endpoint: endpoint.to_string(),
        skills,
    })
}

/// Fetch and read the card at `value`. A network call: never inside a
/// transaction.
pub async fn fetch_card(state: &AppState, value: &str) -> Result<Card, AppError> {
    let refused = |message: &'static str| AppError::Refused {
        field: "cardUrl",
        message,
    };
    let url = card_address(value).ok_or_else(|| refused("Give the agent card's address."))?;
    let trusted = &state.config.webhook_trusted_origins;
    let (url, client) = crate::outbound::webhook_client(url.as_str(), trusted)
        .await
        .map_err(|_| refused("Give an https address Workspace may reach."))?;
    let response = client
        .get(url.clone())
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|_| refused("The agent card didn't answer."))?;
    if !response.status().is_success() {
        return Err(refused("The agent card didn't answer."));
    }
    let bytes = read_limited(response, MAX_CARD_BYTES)
        .await
        .ok_or_else(|| refused("The agent card is too big or cut short."))?;
    let card = serde_json::from_slice::<Value>(&bytes)
        .map_err(|_| refused("That address doesn't serve an agent card."))?;
    read_card(&card, &url, trusted).map_err(refused)
}

async fn read_limited(mut response: reqwest::Response, maximum: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > maximum {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

// ------------------------------------------------------------------ link

/// Link the card's agent as a host owned by `owner`, which `caller` may do
/// (as for an enrolment code). A token is sealed in `secrets`, the
/// workspace's (or, for a person's host, the person's in `workspace`).
#[allow(clippy::too_many_arguments)]
pub async fn link(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    caller: Uuid,
    caller_kind: crate::policy::ActorKind,
    workspace: Uuid,
    owner: Owner,
    card: &Card,
    token: Option<&str>,
) -> Result<HostRow, AppError> {
    super::may_link(transaction, caller, caller_kind, owner).await?;
    let account_id = Uuid::now_v7();
    let host_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'integration', 'active')")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    let token_id = match token.map(str::trim).filter(|token| !token.is_empty()) {
        None => None,
        Some(token) => {
            if token.len() > 4096 || token.chars().any(char::is_control) {
                return Err(AppError::Refused {
                    field: "token",
                    message: "Give the agent's token on one line.",
                });
            }
            let keys = crate::secrets::keyring(&state.config)?;
            let secret_owner = match owner {
                Owner::Workspace(organization_id) => {
                    crate::secrets::SecretOwner::Workspace { organization_id }
                }
                Owner::Account(actor_id) => crate::secrets::SecretOwner::Actor {
                    organization_id: workspace,
                    actor_id,
                },
            };
            Some(
                crate::secrets::put(
                    transaction,
                    keys.as_ref(),
                    crate::secrets::NewSecret {
                        owner: secret_owner,
                        name: &format!("host_a2a_token:{host_id}"),
                        description: Some("The token Workspace sends this A2A agent."),
                        value: &crate::secrets::SecretValue::new(token.to_owned()),
                        actor_id: Some(caller),
                        system_managed: true,
                    },
                )
                .await?
                .id,
            )
        }
    };
    let (owner_account, owner_workspace) = match owner {
        Owner::Account(account) => (Some(account), None),
        Owner::Workspace(workspace) => (None, Some(workspace)),
    };
    let described = json!([{
        "hostAgentRef": AGENT_REF,
        "name": card.name,
        "description": card.description,
        "skills": card.skills,
        "ref": Value::Null,
        "backends": [],
        "actions": [],
    }]);
    let capabilities = json!({
        "start": true, "cancel": true, "answer": true, "inspect": true,
        "idempotentStart": false, "steer": false, "a2a": true,
    });
    Ok(sqlx::query_as::<_, HostRow>(&format!(
        r#"
        INSERT INTO hosts AS host (
            id, account_id, owner_account_id, owner_workspace_id, name, software,
            software_version, protocol_version, connection, capabilities, described_agents,
            a2a_card_url, a2a_endpoint, a2a_token_secret_id, enrolled_by_account_id, last_seen_at
        )
        VALUES ($1, $2, $3, $4, $5, 'A2A', $6, $7, 'a2a', $8, $9, $10, $11, $12, $13, now())
        RETURNING {HOST_COLUMNS}
        "#
    ))
    .bind(host_id)
    .bind(account_id)
    .bind(owner_account)
    .bind(owner_workspace)
    .bind(&card.name)
    .bind(card.version.as_deref())
    .bind(PROTOCOL)
    .bind(&capabilities)
    .bind(&described)
    .bind(&card.url)
    .bind(&card.endpoint)
    .bind(token_id)
    .bind(caller)
    .fetch_one(&mut **transaction)
    .await?)
}

/// The host as its caller sees it after linking.
pub async fn view(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Uuid,
    host: &HostRow,
) -> Result<HostView, AppError> {
    super::view(transaction, caller, host).await
}

// ------------------------------------------------------------- commands

/// Queue the job that sends an A2A host one command.
pub async fn send_later(
    transaction: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    workspace_id: Uuid,
) -> Result<(), AppError> {
    crate::jobs::enqueue(
        &mut **transaction,
        NewJob::new(COMMAND_KIND)
            .payload(json!({"commandId": command_id}))
            .workspace(workspace_id)
            .unique(format!("host-a2a:{command_id}"))
            .max_attempts(5),
    )
    .await?;
    Ok(())
}

/// Where an A2A host is called, and with what token.
struct Endpoint {
    host: HostRow,
    url: String,
    token: Option<String>,
}

async fn endpoint(state: &AppState, host_id: Uuid) -> Result<Option<Endpoint>, JobError> {
    let query = sqlx::query_as::<_, (Option<String>, Option<Uuid>)>(
        "SELECT a2a_endpoint, a2a_token_secret_id FROM hosts WHERE id = $1 AND status <> 'revoked'",
    )
    .bind(host_id);
    let found = crate::db::scoped_one(&state.pool, async |connection| {
        query.fetch_optional(connection).await
    })
    .await
    .map_err(|_: sqlx::Error| JobError::retry("database"))?;
    let Some((Some(url), token_id)) = found else {
        return Ok(None);
    };
    let sql = format!("SELECT {HOST_COLUMNS} FROM hosts host WHERE host.id = $1");
    let host = sqlx::query_as::<_, HostRow>(&sql).bind(host_id);
    let host = crate::db::scoped_one(&state.pool, async |connection| {
        host.fetch_one(connection).await
    })
    .await
    .map_err(|_: sqlx::Error| JobError::retry("database"))?;
    let token = match token_id {
        None => None,
        Some(id) => {
            let keys = crate::secrets::keyring(&state.config)
                .map_err(|_| JobError::permanent("keyring"))?;
            let secret = crate::secrets::reveal(&state.pool, keys.as_ref(), id, "host.a2a.auth")
                .await
                .map_err(|_| JobError::retry("a2a token"))?;
            Some(secret.expose().to_owned())
        }
    };
    Ok(Some(Endpoint { host, url, token }))
}

/// Why a call got no result.
#[derive(Debug)]
enum CallError {
    /// No answer, or one that can't be read: it may or may not have
    /// happened.
    Unknown,
    /// The agent answered with an error: it didn't happen.
    Refused(String),
}

/// One JSON-RPC call. A network call: never inside a transaction.
async fn call(
    state: &AppState,
    endpoint: &Endpoint,
    method: &str,
    params: Value,
) -> Result<Value, CallError> {
    let (url, client) =
        crate::outbound::webhook_client(&endpoint.url, &state.config.webhook_trusted_origins)
            .await
            .map_err(|_| CallError::Unknown)?;
    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&json!({"jsonrpc": "2.0", "id": Uuid::now_v7().to_string(),
                      "method": method, "params": params}));
    if let Some(token) = &endpoint.token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|_| CallError::Unknown)?;
    let status = response.status();
    let bytes = read_limited(response, MAX_ANSWER_BYTES)
        .await
        .ok_or(CallError::Unknown)?;
    let body = serde_json::from_slice::<Value>(&bytes).map_err(|_| CallError::Unknown)?;
    if let Some(error) = body.get("error").filter(|error| error.is_object()) {
        let message = line(error.get("message"), 200)
            .unwrap_or_else(|| "The agent refused the call.".to_owned());
        return Err(CallError::Refused(message));
    }
    if !status.is_success() {
        return Err(if status.is_client_error() {
            CallError::Refused(format!("The agent answered {}.", status.as_u16()))
        } else {
            CallError::Unknown
        });
    }
    let result = body.get("result").cloned().ok_or(CallError::Unknown)?;
    // Heard from: it is there.
    let seen =
        sqlx::query("UPDATE hosts SET last_seen_at = now() WHERE id = $1").bind(endpoint.host.id);
    let _ = crate::db::scoped_one(&state.pool, async |connection| {
        seen.execute(connection).await
    })
    .await;
    Ok(result)
}

fn acked(outcome: AckOutcome) -> Ack {
    Ack::new(outcome)
}

async fn answer(state: &AppState, host: &HostRow, command_id: Uuid, ack: Ack) {
    if let Err(error) = super::commands::ack(&state.pool, host, command_id, ack).await {
        let category = match error {
            HostError::App(error) => error.log_category(),
            _ => "refused",
        };
        tracing::warn!(%command_id, category, "an A2A command's answer was not recorded");
    }
}

fn text_of(parts: Option<&Value>) -> Option<String> {
    let text = parts?
        .as_array()?
        .iter()
        .filter(|part| part.get("kind").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(MAX_TEXT_CHARS).collect())
}

/// Send one command. A start is sent at most once: once it may have gone,
/// a failure is the run's `start_uncertain`, never a retry.
pub async fn send(state: &AppState, command_id: Uuid) -> Result<(), JobError> {
    let query = sqlx::query_as::<_, (Uuid, Option<Uuid>, String, String)>(
        "SELECT host_id, run_id, kind, state FROM host_commands WHERE id = $1",
    )
    .bind(command_id);
    let found = crate::db::scoped_one(&state.pool, async |connection| {
        query.fetch_optional(connection).await
    })
    .await
    .map_err(|_: sqlx::Error| JobError::retry("database"))?;
    let Some((host_id, run_id, kind, command_state)) = found else {
        return Ok(());
    };
    if command_state != "pending" {
        return Ok(());
    }
    let Some(endpoint) = endpoint(state, host_id).await? else {
        return Ok(());
    };
    // Read as a host reads it (the answer to a request is filled in now).
    let command = match super::commands::fetch(state, &endpoint.host, command_id).await {
        Ok(command) => command,
        Err(HostError::CommandClosed | HostError::CommandUnknown) => return Ok(()),
        Err(_) => return Err(JobError::retry("fetch")),
    };
    let task = match run_id {
        Some(run_id) => task_of(state, run_id).await?,
        None => None,
    };
    let host = endpoint.host.clone();
    match kind.as_str() {
        "start" => {
            let Some(run_id) = run_id else {
                return Ok(());
            };
            let prompt = command["input"]["prompt"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let start_key = command["startKey"].as_str().unwrap_or_default().to_owned();
            let params = json!({
                "message": {
                    "kind": "message", "role": "user", "messageId": start_key,
                    "parts": [{"kind": "text", "text": prompt}],
                    "metadata": {"workspace": {"runId": run_id, "title": command["title"],
                                               "cause": command["cause"]}},
                },
                "configuration": {"blocking": false,
                                  "acceptedOutputModes": ["text/plain", "application/json"]},
            });
            match call(state, &endpoint, "message/send", params).await {
                Ok(result) => started(state, &host, command_id, run_id, &result).await,
                Err(CallError::Refused(message)) => {
                    let mut ack = acked(AckOutcome::Refused);
                    ack.reason = Some("a2a_error".to_owned());
                    ack.message = Some(message);
                    answer(state, &host, command_id, ack).await;
                }
                Err(CallError::Unknown) => {
                    let mut ack = acked(AckOutcome::Uncertain);
                    ack.message = Some("The agent didn't answer the start.".to_owned());
                    answer(state, &host, command_id, ack).await;
                }
            }
        }
        "cancel" => {
            let Some((task_id, _)) = task else {
                // Nothing was started there: nothing to stop.
                answer(state, &host, command_id, acked(AckOutcome::Done)).await;
                return Ok(());
            };
            match call(state, &endpoint, "tasks/cancel", json!({"id": task_id})).await {
                Ok(_) => answer(state, &host, command_id, acked(AckOutcome::Done)).await,
                Err(CallError::Refused(message)) => {
                    // Usually finished already: its state says how it ended.
                    let mut ack = acked(AckOutcome::Refused);
                    ack.message = Some(message);
                    answer(state, &host, command_id, ack).await;
                    poll_soon(state, run_id).await;
                }
                Err(CallError::Unknown) => {
                    answer(state, &host, command_id, acked(AckOutcome::Accepted)).await;
                    poll_soon(state, run_id).await;
                }
            }
        }
        "answer" => {
            let Some((task_id, context_id)) = task else {
                let mut ack = acked(AckOutcome::Refused);
                ack.message = Some("The agent has no task to answer.".to_owned());
                answer(state, &host, command_id, ack).await;
                return Ok(());
            };
            let given = &command["answer"];
            let text = if given["status"] == "answered" {
                given["answer"]["text"]
                    .as_str()
                    .or(given["answer"]["label"].as_str())
                    .or(given["answer"]["optionId"].as_str())
                    .unwrap_or("Answered.")
                    .to_owned()
            } else {
                "No answer came in time.".to_owned()
            };
            let request_id = command["requestId"].as_str().unwrap_or_default();
            let params = json!({
                "message": {
                    "kind": "message", "role": "user", "messageId": format!("answer-{request_id}"),
                    "taskId": task_id, "contextId": context_id,
                    "parts": [{"kind": "text", "text": text}],
                },
                "configuration": {"blocking": false},
            });
            match call(state, &endpoint, "message/send", params).await {
                Ok(result) => {
                    answer(state, &host, command_id, acked(AckOutcome::Done)).await;
                    if let Some(run_id) = run_id
                        && result.get("kind").and_then(Value::as_str) == Some("task")
                    {
                        apply_logged(state, run_id, &result).await;
                    }
                }
                Err(error) => {
                    let mut ack = acked(AckOutcome::Refused);
                    ack.message = Some(match error {
                        CallError::Refused(message) => message,
                        CallError::Unknown => "The agent didn't take the answer.".to_owned(),
                    });
                    answer(state, &host, command_id, ack).await;
                }
            }
            poll_soon(state, run_id).await;
        }
        "inspect" => {
            let mut ack = acked(AckOutcome::Done);
            ack.result = Some(match task {
                Some((task_id, _)) => {
                    match call(state, &endpoint, "tasks/get", json!({"id": task_id})).await {
                        Ok(_) => json!({"known": true, "hostRunRef": task_id}),
                        Err(_) => json!({"known": false}),
                    }
                }
                None => json!({"known": false}),
            });
            answer(state, &host, command_id, ack).await;
        }
        _ => {
            let mut ack = acked(AckOutcome::Refused);
            ack.reason = Some("unsupported".to_owned());
            ack.message = Some("An A2A agent takes starts, stops and answers.".to_owned());
            answer(state, &host, command_id, ack).await;
        }
    }
    Ok(())
}

/// The agent took the start: a task to follow, or a message that is the
/// whole answer.
async fn started(state: &AppState, host: &HostRow, command_id: Uuid, run_id: Uuid, result: &Value) {
    let kind = result.get("kind").and_then(Value::as_str);
    let reference = match kind {
        Some("task") => result.get("id"),
        _ => result.get("messageId"),
    }
    .and_then(Value::as_str)
    .map(|value| value.chars().take(200).collect::<String>());
    let Some(reference) = reference.filter(|value| !value.trim().is_empty()) else {
        let mut ack = acked(AckOutcome::Uncertain);
        ack.message = Some("The agent's answer to the start can't be read.".to_owned());
        answer(state, host, command_id, ack).await;
        return;
    };
    if kind == Some("task") {
        let context = result
            .get("contextId")
            .and_then(Value::as_str)
            .map(|value| value.chars().take(200).collect::<String>());
        let follow = sqlx::query(
            r#"
            INSERT INTO host_a2a_tasks (run_id, workspace_id, host_id, task_id, context_id)
            SELECT command.run_id, command.workspace_id, $2, $3, $4 FROM host_commands command
            WHERE command.id = $5 AND command.run_id = $1
            ON CONFLICT (run_id) DO NOTHING
            "#,
        )
        .bind(run_id)
        .bind(host.id)
        .bind(&reference)
        .bind(context)
        .bind(command_id);
        if let Err(error) = crate::db::scoped_one(&state.pool, async |connection| {
            follow.execute(connection).await
        })
        .await
        {
            tracing::warn!(%run_id, category = AppError::from(error).log_category(), "an A2A task is not followed");
        }
    }
    let mut ack = acked(AckOutcome::Accepted);
    ack.host_run_ref = Some(reference);
    answer(state, host, command_id, ack).await;
    if kind == Some("task") {
        apply_logged(state, run_id, result).await;
    } else {
        reply_message(state, run_id, result).await;
    }
}

/// The task a run's start became, if one did: its id and context.
async fn task_of(
    state: &AppState,
    run_id: Uuid,
) -> Result<Option<(String, Option<String>)>, JobError> {
    let query = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT task_id, context_id FROM host_a2a_tasks WHERE run_id = $1",
    )
    .bind(run_id);
    crate::db::scoped_one(&state.pool, async |connection| {
        query.fetch_optional(connection).await
    })
    .await
    .map_err(|_: sqlx::Error| JobError::retry("database"))
}

async fn poll_soon(state: &AppState, run_id: Option<Uuid>) {
    let Some(run_id) = run_id else {
        return;
    };
    let query =
        sqlx::query("UPDATE host_a2a_tasks SET next_poll_at = now(), polls = 0 WHERE run_id = $1")
            .bind(run_id);
    let _ = crate::db::scoped_one(&state.pool, async |connection| {
        query.execute(connection).await
    })
    .await;
}

// ---------------------------------------------------------------- reports

fn activity(client_id: String, kind: &str, payload: Value) -> NewActivity {
    NewActivity {
        client_id: client_id.chars().take(200).collect(),
        kind: kind.to_owned(),
        occurred_at: Utc::now(),
        step_ref: None,
        parent_ref: None,
        summary: None,
        payload,
    }
}

/// What one task snapshot reports, and how it ends the run if it does.
#[derive(Debug, Default, PartialEq)]
pub struct Snapshot {
    pub activities: Vec<(String, String, Value)>,
    /// done, cancelled, or failed with the reason.
    pub end: Option<(&'static str, Option<String>)>,
    pub working: bool,
    pub task_state: String,
}

/// Read one task: its status message, its artifacts, its state.
pub fn snapshot(task: &Value) -> Snapshot {
    let task_id = task.get("id").and_then(Value::as_str).unwrap_or("task");
    let status = task.get("status").cloned().unwrap_or(Value::Null);
    let task_state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .chars()
        .take(40)
        .collect::<String>();
    let message = status.get("message");
    let message_text = message.and_then(|message| text_of(message.get("parts")));
    let message_id = message
        .and_then(|message| message.get("messageId"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("{task_id}:{task_state}"), str::to_owned);
    let mut snapshot = Snapshot {
        task_state: task_state.clone(),
        ..Snapshot::default()
    };
    match task_state.as_str() {
        "input-required" | "auth-required" => {
            let approval = task_state == "auth-required";
            let title = message_text.clone().unwrap_or_else(|| {
                if approval {
                    "The agent needs you to sign in or allow something.".to_owned()
                } else {
                    "The agent needs more from you.".to_owned()
                }
            });
            let title: String = title.chars().take(1_900).collect();
            let request = if approval {
                json!({"kind": "approval", "title": title, "allowText": true,
                       "clientRequestId": format!("a2a:{message_id}"),
                       "options": [{"id": "done", "label": "Done", "style": "approve"},
                                   {"id": "no", "label": "Not now", "style": "deny"}]})
            } else {
                json!({"kind": "question", "title": title, "allowText": true,
                       "clientRequestId": format!("a2a:{message_id}")})
            };
            snapshot.activities.push((
                format!("a2a-ask:{message_id}"),
                "interrupt".to_owned(),
                json!({"request": request}),
            ));
        }
        _ => {
            if let Some(text) = &message_text {
                snapshot.activities.push((
                    format!("a2a:{message_id}"),
                    "text_message".to_owned(),
                    json!({"text": text}),
                ));
            }
        }
    }
    for (index, artifact) in task
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(50)
        .enumerate()
    {
        let artifact_id = artifact
            .get("artifactId")
            .and_then(Value::as_str)
            .map_or_else(|| format!("{task_id}:{index}"), str::to_owned);
        let name = line(artifact.get("name"), 200);
        if let Some(text) = text_of(artifact.get("parts")) {
            snapshot.activities.push((
                format!("a2a-artifact:{artifact_id}"),
                "text_message".to_owned(),
                json!({"text": text, "artifact": name}),
            ));
        }
        for (part_index, part) in artifact
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(20)
            .enumerate()
        {
            let payload = match part.get("kind").and_then(Value::as_str) {
                Some("data") => json!({"kind": "data", "name": name, "data": part.get("data")}),
                Some("file") => {
                    let file = part.get("file").cloned().unwrap_or(Value::Null);
                    let uri = file
                        .get("uri")
                        .and_then(Value::as_str)
                        .filter(|uri| uri.starts_with("https://") && uri.len() <= 2_000);
                    json!({"kind": "file", "name": line(file.get("name"), 200).or(name.clone()),
                           "mimeType": line(file.get("mimeType"), 100), "uri": uri})
                }
                _ => continue,
            };
            snapshot.activities.push((
                format!("a2a-artifact:{artifact_id}:{part_index}"),
                "result".to_owned(),
                payload,
            ));
        }
    }
    snapshot.end = match task_state.as_str() {
        "completed" => Some(("done", None)),
        "canceled" => Some(("cancelled", None)),
        "failed" => Some((
            "failed",
            Some(message_text.unwrap_or_else(|| "The agent's task failed.".to_owned())),
        )),
        "rejected" => Some((
            "failed",
            Some(message_text.unwrap_or_else(|| "The agent declined the task.".to_owned())),
        )),
        _ => None,
    };
    snapshot.working = task_state == "working";
    snapshot
}

fn ended(task_state: &str) -> bool {
    matches!(task_state, "completed" | "canceled" | "failed" | "rejected")
}

async fn apply_logged(state: &AppState, run_id: Uuid, task: &Value) {
    if let Err(error) = apply(state, run_id, task).await {
        tracing::warn!(%run_id, category = error.log_category(), "an A2A task update was not recorded");
    }
}

fn run_error(error: RunOpError) -> Option<AppError> {
    match error {
        RunOpError::App(error) => Some(error),
        // A report the run's state no longer takes (it ended, or moved on):
        // nothing to record.
        _ => None,
    }
}

/// Record one task snapshot on its run, as the run's agent. Repeats are
/// duplicates (each report's id comes from the agent's own ids).
pub async fn apply(state: &AppState, run_id: Uuid, task: &Value) -> Result<(), AppError> {
    let snapshot = snapshot(task);
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(run) = ops::locked(&mut transaction, run_id).await? else {
        return Ok(());
    };
    let reporter = Reporter::Account {
        account_id: run.account_id,
    };
    let done = finished(&run);
    if !done {
        if snapshot.working && run.state == RunState::Accepted.as_str() {
            let change = RunUpdate {
                state: Some(RunState::Working),
                ..RunUpdate::default()
            };
            if let Err(error) = ops::update(&mut transaction, reporter, run_id, change).await
                && let Some(error) = run_error(error)
            {
                return Err(error);
            }
        }
        // A long task can say more than one report takes: all of it is
        // recorded, a report at a time, before the run can finish.
        let reports = snapshot
            .activities
            .iter()
            .map(|(id, kind, payload)| activity(id.clone(), kind, payload.clone()))
            .collect::<Vec<_>>();
        for batch in reports.chunks(crate::runs::activities::MAX_BATCH) {
            if let Err(error) =
                ops::append(&mut transaction, reporter, run_id, batch.to_vec()).await
                && let Some(error) = run_error(error)
            {
                return Err(error);
            }
        }
        match &snapshot.end {
            Some(("failed", reason)) => {
                let change = RunUpdate {
                    state: Some(RunState::Failed),
                    failure_reason: reason.clone(),
                    ..RunUpdate::default()
                };
                if let Err(error) = ops::update(&mut transaction, reporter, run_id, change).await
                    && let Some(error) = run_error(error)
                {
                    return Err(error);
                }
            }
            Some((outcome, _)) => {
                let finish = activity(
                    format!("a2a-finished:{run_id}"),
                    "run_finished",
                    json!({"outcome": outcome}),
                );
                if let Err(error) =
                    ops::append(&mut transaction, reporter, run_id, vec![finish]).await
                    && let Some(error) = run_error(error)
                {
                    return Err(error);
                }
            }
            None => {}
        }
    }
    if done || ended(&snapshot.task_state) {
        sqlx::query("DELETE FROM host_a2a_tasks WHERE run_id = $1")
            .bind(run_id)
            .execute(&mut *transaction)
            .await?;
    } else {
        sqlx::query(
            r#"
            UPDATE host_a2a_tasks
               SET task_state = $2, polls = polls + 1, updated_at = now(),
                   next_poll_at = now() + make_interval(secs => $3::double precision)
             WHERE run_id = $1
            "#,
        )
        .bind(run_id)
        .bind(&snapshot.task_state)
        .bind(poll_after(0))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

fn finished(run: &RunRow) -> bool {
    run.run_state().is_ok_and(RunState::is_terminal)
}

/// The agent answered the start with a message: that is all it has to say.
async fn reply_message(state: &AppState, run_id: Uuid, message: &Value) {
    let message_id = message
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or("reply")
        .to_owned();
    let text = text_of(message.get("parts"));
    let result = async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let Some(run) = ops::locked(&mut transaction, run_id).await? else {
            return Ok(());
        };
        let reporter = Reporter::Account {
            account_id: run.account_id,
        };
        let mut batch = Vec::new();
        if let Some(text) = text {
            batch.push(activity(
                format!("a2a:{message_id}"),
                "text_message",
                json!({"text": text}),
            ));
        }
        batch.push(activity(
            format!("a2a-finished:{run_id}"),
            "run_finished",
            json!({"outcome": "done"}),
        ));
        if let Err(error) = ops::append(&mut transaction, reporter, run_id, batch).await
            && let Some(error) = run_error(error)
        {
            return Err(error);
        }
        transaction.commit().await?;
        Ok::<_, AppError>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%run_id, category = error.log_category(), "an A2A reply was not recorded");
    }
}

// ------------------------------------------------------------------ poll

/// Look at the tasks that are due: each is held for a minute while it is
/// looked at, so two passes never look at the same one. Returns how many
/// were looked at.
pub async fn poll_due(state: &AppState) -> Result<usize, AppError> {
    // Server-wide: due remote tasks of every workspace.
    crate::tenancy::system("runs.host", poll_due_here(state)).await
}

async fn poll_due_here(state: &AppState) -> Result<usize, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let due = sqlx::query_as::<_, (Uuid, Uuid, String, i32)>(
        r#"
        UPDATE host_a2a_tasks task
           SET next_poll_at = now() + make_interval(secs => $2::double precision)
         WHERE task.run_id IN (
               SELECT run_id FROM host_a2a_tasks
               WHERE next_poll_at <= now()
               ORDER BY next_poll_at
               LIMIT $1
               FOR UPDATE SKIP LOCKED)
        RETURNING task.run_id, task.host_id, task.task_id, task.polls
        "#,
    )
    .bind(POLL_BATCH)
    .bind(POLL_LEASE_SECONDS)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    for (run_id, host_id, task_id, polls) in &due {
        let endpoint = match endpoint(state, *host_id).await {
            Ok(Some(endpoint)) => endpoint,
            Ok(None) => {
                // Unlinked: its runs were interrupted; nothing to follow.
                sqlx::query("DELETE FROM host_a2a_tasks WHERE run_id = $1")
                    .bind(run_id)
                    .execute(crate::db::scoped(&state.pool))
                    .await?;
                continue;
            }
            Err(_) => continue,
        };
        match call(
            state,
            &endpoint,
            "tasks/get",
            json!({"id": task_id, "historyLength": 0}),
        )
        .await
        {
            Ok(task) => {
                apply_logged(state, *run_id, &task).await;
                sqlx::query(
                    r#"
                    UPDATE host_a2a_tasks
                       SET next_poll_at = now() + make_interval(secs => $2::double precision)
                     WHERE run_id = $1
                    "#,
                )
                .bind(run_id)
                .bind(poll_after(polls + 1))
                .execute(crate::db::scoped(&state.pool))
                .await?;
            }
            Err(_) => {
                // Looked at again when the hold runs out.
            }
        }
    }
    Ok(due.len())
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        COMMAND_KIND,
        Duration::from_secs(60),
        |context: JobContext| async move {
            let command_id = context
                .job
                .payload
                .get("commandId")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok())
                .ok_or_else(|| JobError::permanent("payload"))?;
            send(&context.state, command_id).await
        },
    );
    registry.register_with_timeout(
        POLL_KIND,
        Duration::from_secs(120),
        |context: JobContext| async move {
            poll_due(&context.state)
                .await
                .map_err(|error| JobError::retry(error.log_category()))?;
            Ok(())
        },
    );
    registry.every(POLL_KIND, Duration::from_secs(10));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{card_address, poll_after, snapshot};

    #[test]
    fn a_bare_address_means_its_well_known_card() {
        assert_eq!(
            card_address("https://agent.example").map(|url| url.to_string()),
            Some("https://agent.example/.well-known/agent-card.json".to_owned())
        );
        assert_eq!(
            card_address("https://agent.example/cards/one.json").map(|url| url.to_string()),
            Some("https://agent.example/cards/one.json".to_owned())
        );
    }

    #[test]
    fn tasks_are_looked_at_less_often_as_they_run() {
        let schedule = [0, 3, 6, 11].map(poll_after);
        assert_eq!(schedule, [5.0, 15.0, 30.0, 60.0]);
    }

    #[test]
    fn a_task_reads_as_reports() {
        let task = json!({
            "kind": "task", "id": "t1", "contextId": "c1",
            "status": {"state": "input-required", "message": {"kind": "message", "role": "agent",
                "messageId": "m2", "parts": [{"kind": "text", "text": "Which branch?"}]}},
        });
        let read = snapshot(&task);
        assert_eq!(read.activities.len(), 1);
        assert_eq!(read.activities[0].1, "interrupt");
        assert_eq!(read.activities[0].2["request"]["title"], "Which branch?");
        assert_eq!(read.end, None);

        let task = json!({
            "kind": "task", "id": "t1",
            "status": {"state": "completed"},
            "artifacts": [{"artifactId": "a1", "name": "Report", "parts": [
                {"kind": "text", "text": "All done."},
                {"kind": "data", "data": {"passed": 3}}]}],
        });
        let read = snapshot(&task);
        assert_eq!(read.end, Some(("done", None)));
        let kinds = read
            .activities
            .iter()
            .map(|(_, kind, _)| kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(kinds, ["text_message", "result"]);
        assert_eq!(read.activities[1].2["data"]["passed"], 3);

        let read = snapshot(&json!({"id": "t1", "status": {"state": "rejected"}}));
        assert_eq!(read.end.map(|(outcome, _)| outcome), Some("failed"));
    }
}

//! A1.16: what the built-in host tells the server about a run goes only
//! through the host protocol's run reports (`run_activity_report`,
//! `run_update`, `run_usage_report`), with the credential the run's start
//! carried, as any host reports. The built-in host's own step names
//! (`command_started`, `run_failed`, ...) travel in `payload.step`.

use agent_workspace_host_protocol::{ClientError, HostClient, RunReporter};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::requests::Outcome;
use crate::RuntimeError as AppError;

/// The longest step payload sent whole; a bigger one keeps only its name.
const MAX_PAYLOAD_BYTES: usize = 60_000;

/// A run's reports, for as long as the host works on it. Its credential is
/// fetched again from the run's start when it runs out.
pub struct RunReports {
    client: HostClient,
    run_id: Uuid,
    start_command_id: Uuid,
    reporter: Mutex<RunReporter>,
}

impl std::fmt::Debug for RunReports {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunReports")
            .field("run_id", &self.run_id)
            .finish_non_exhaustive()
    }
}

fn unavailable(error: ClientError) -> AppError {
    let status = match error {
        ClientError::Refused(status, _) => status,
        _ => 0,
    };
    tracing::warn!(status, "the built-in host's report was not taken");
    AppError::Internal("report to the run")
}

impl RunReports {
    /// The reports of the run `start_command_id` started.
    pub async fn for_start(
        client: HostClient,
        run_id: Uuid,
        start_command_id: Uuid,
    ) -> Result<Self, AppError> {
        let reporter = Self::fresh(&client, run_id, start_command_id).await?;
        Ok(Self {
            client,
            run_id,
            start_command_id,
            reporter: Mutex::new(reporter),
        })
    }

    async fn fresh(
        client: &HostClient,
        run_id: Uuid,
        start_command_id: Uuid,
    ) -> Result<RunReporter, AppError> {
        let start = client
            .command(start_command_id)
            .await
            .map_err(unavailable)?;
        let credential = start.workspace_credential.ok_or(AppError::Conflict(
            "agent_run",
            "the run has ended; nothing more is reported on it",
        ))?;
        Ok(client.reporter(run_id, &credential))
    }

    pub fn run_id(&self) -> Uuid {
        self.run_id
    }

    /// One call on the run (its id added), with a fresh credential once if
    /// the one held ran out. Answers the operation's value.
    async fn call(&self, name: &str, input: Value, key: &str) -> Result<Value, AppError> {
        self.call_with(name, input, key, true).await
    }

    async fn call_with(
        &self,
        name: &str,
        input: Value,
        key: &str,
        on_run: bool,
    ) -> Result<Value, AppError> {
        let send = async |reporter: RunReporter, input: Value| {
            if on_run {
                reporter.operation(name, input, key).await
            } else {
                reporter.call_as_agent(name, &input, key).await
            }
        };
        let reporter = self.reporter.lock().await.clone();
        let answer = match send(reporter, input.clone()).await {
            Err(ClientError::Refused(401, _)) => {
                let fresh = Self::fresh(&self.client, self.run_id, self.start_command_id).await?;
                *self.reporter.lock().await = fresh.clone();
                send(fresh, input).await.map_err(unavailable)?
            }
            other => other.map_err(unavailable)?,
        };
        Ok(match answer {
            Value::Object(mut fields) => fields.remove("value").unwrap_or(Value::Null),
            _ => Value::Null,
        })
    }

    /// One step of the run, once per `key`; `in_thread` also shows it in
    /// the run's thread.
    pub async fn step(
        &self,
        key: &str,
        step: &str,
        summary: &str,
        payload: Value,
        in_thread: bool,
    ) -> Result<(), AppError> {
        self.activities(vec![activity(key, step, summary, payload, in_thread)])
            .await
    }

    /// A step that only says something (shown in the thread when
    /// `in_thread`): whatever its name, it moves the run nowhere, as a
    /// failure's note before `moved("failed")` must not.
    pub async fn note(
        &self,
        key: &str,
        step: &str,
        summary: &str,
        in_thread: bool,
    ) -> Result<(), AppError> {
        let mut note = activity(key, step, summary, json!({}), in_thread);
        note["kind"] = json!("custom");
        if let Some(payload) = note["payload"].as_object_mut() {
            payload.remove("outcome");
            payload.remove("toolCallRef");
        }
        self.activities(vec![note]).await
    }

    /// Several steps at once (at most 100).
    pub async fn activities(&self, activities: Vec<Value>) -> Result<(), AppError> {
        let key = report_key(self.run_id, &activities);
        self.call(
            "run_activity_report",
            json!({ "activities": activities }),
            &key,
        )
        .await?;
        Ok(())
    }

    /// Report what the harness asked (an `interrupt` from
    /// [`super::requests::interrupt`]): the request it opened, or opened
    /// the first time it was asked.
    pub async fn raise(&self, interrupt: Value) -> Result<Uuid, AppError> {
        let batch = vec![interrupt];
        let key = report_key(self.run_id, &batch);
        let output = self
            .call("run_activity_report", json!({ "activities": batch }), &key)
            .await?;
        output["requests"][0]["requestId"]
            .as_str()
            .and_then(|id| id.parse().ok())
            .ok_or(AppError::Internal("the run's request didn't open"))
    }

    /// Where a request the run raised stands, read as the run's agent.
    pub async fn outcome(&self, request_id: Uuid) -> Result<Outcome, AppError> {
        let read = self
            .call_with(
                "request_get",
                json!({ "requestId": request_id }),
                &format!("read-{}", request_id.simple()),
                false,
            )
            .await?;
        Ok(super::requests::outcome_of(&read["request"]))
    }

    /// The run's state (`working`, `failed`, `cancelled`, ...), with a
    /// failure's reason.
    pub async fn moved(&self, state: &str, failure_reason: Option<&str>) -> Result<(), AppError> {
        let mut fields = json!({ "state": state });
        if let Some(reason) = failure_reason {
            fields["failureReason"] = json!(reason.chars().take(2_000).collect::<String>());
        }
        let key = format!("builtin-{}-{state}", self.run_id.simple());
        self.call("run_update", fields, &key).await?;
        Ok(())
    }

    /// Anything else `run_update` takes (a status line, an address).
    pub async fn update(&self, fields: Value, key: &str) -> Result<(), AppError> {
        self.call("run_update", fields, key).await?;
        Ok(())
    }

    /// Tokens a turn used.
    pub async fn usage(
        &self,
        report_key: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<(), AppError> {
        self.call(
            "run_usage_report",
            json!({"reportKey": report_key, "model": model,
                   "inputTokens": input_tokens, "outputTokens": output_tokens}),
            &format!(
                "usage-{}",
                short(&format!("{}-{report_key}", self.run_id.simple()))
            ),
        )
        .await?;
        Ok(())
    }
}

/// The idempotency key of a batch of a run's activities: it names them, so
/// sending the same batch again is a duplicate.
fn report_key(run_id: Uuid, activities: &[Value]) -> String {
    let mut named = run_id.to_string();
    for id in activities
        .iter()
        .filter_map(|activity| activity.get("id").and_then(Value::as_str))
    {
        named.push('\n');
        named.push_str(id);
    }
    format!("steps-{}", hex::encode(Sha256::digest(named.as_bytes())))
}

/// A key the idempotency rules take (letters, digits, `-_.`, short).
fn short(key: &str) -> String {
    if key.len() <= 64
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return key.to_owned();
    }
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// One step as a run activity.
pub fn activity(key: &str, step: &str, summary: &str, payload: Value, in_thread: bool) -> Value {
    let kind = activity_kind(step, &payload);
    let mut payload = activity_payload(step, payload);
    if kind == "tool_call" {
        payload["toolCallRef"] = json!(client_id(key));
    }
    if in_thread {
        payload["inThread"] = Value::Bool(true);
    }
    json!({
        "id": client_id(key),
        "kind": kind,
        "summary": summary.chars().take(2_000).collect::<String>(),
        "payload": payload,
    })
}

/// The activity kind a built-in step is reported as.
fn activity_kind(step: &str, payload: &Value) -> &'static str {
    match step {
        "run_state_changed" => match payload.get("to").and_then(Value::as_str) {
            Some("starting" | "working") => "run_started",
            Some("completed" | "failed" | "canceled" | "interrupted") => "run_finished",
            _ => "custom",
        },
        "command_started" | "command_completed" | "file_change" | "tool_used" | "test_result" => {
            "tool_call"
        }
        "run_completed" | "run_failed" | "run_interrupted" => "run_finished",
        "agent_note" | "agent_reply" => "text_message",
        "plan" => "plan",
        _ => "custom",
    }
}

/// What the activity's payload holds: the step's own, its name, and the
/// outcome a finishing step reports.
fn activity_payload(step: &str, payload: Value) -> Value {
    let to = payload.get("to").and_then(Value::as_str).map(str::to_owned);
    let mut fields = match payload {
        Value::Object(fields) => fields,
        _ => Map::new(),
    };
    fields.insert("step".to_owned(), Value::String(step.to_owned()));
    let outcome = match (step, to.as_deref()) {
        ("run_completed", _) | ("run_state_changed", Some("completed")) => Some("done"),
        ("run_failed", _) | ("run_state_changed", Some("failed")) => Some("failed"),
        ("run_interrupted", _) | ("run_state_changed", Some("canceled" | "interrupted")) => {
            Some("cancelled")
        }
        _ => None,
    };
    if let Some(outcome) = outcome {
        fields.insert("outcome".to_owned(), Value::String(outcome.to_owned()));
    }
    if step == "agent_note" && !fields.contains_key("text") {
        fields.insert("role".to_owned(), Value::String("note".to_owned()));
    }
    let payload = Value::Object(fields);
    if payload.to_string().len() > MAX_PAYLOAD_BYTES {
        return json!({"step": step, "truncated": true});
    }
    payload
}

/// A step's key as the log's dedupe id (at most 200 characters).
fn client_id(key: &str) -> String {
    if key.chars().count() <= 200 {
        return key.to_owned();
    }
    format!("sha256:{}", hex::encode(Sha256::digest(key.as_bytes())))
}

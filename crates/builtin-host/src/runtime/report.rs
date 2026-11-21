//! A1.16: what the built-in host tells the server about a run goes only
//! through the host protocol's run reports (`run_activity_report`,
//! `run_update`, `run_usage_report`, `run_changes_report`), with the
//! credential the run's start carried, as any host reports. The built-in
//! host's own step names (`command_started`, `run_failed`, ...) travel in
//! `payload.step`.

use agent_workspace_host_protocol::{ClientError, HostClient, RunReporter, StreamDelta};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::Mutex,
    time::{Duration, Instant, sleep},
};
use uuid::Uuid;

use super::requests::Outcome;
use crate::RuntimeError as AppError;

/// The longest step payload sent whole; a bigger one keeps only its name.
const MAX_PAYLOAD_BYTES: usize = 60_000;

/// How long a report waits out a server it can't reach, such as one
/// restarting while the host (its own program) carries on with the run.
/// Every report carries its key, so one sent twice is taken once.
const UNREACHABLE_FOR: Duration = Duration::from_secs(120);
const FIRST_WAIT: Duration = Duration::from_millis(200);
const LONGEST_WAIT: Duration = Duration::from_secs(5);
/// How long a changes report waits out an unreachable server. What a run
/// changed is shown, not needed: a turn or a stop never waits long on it,
/// and the next report (or the last, at the run's end) says it again.
const CHANGES_UNREACHABLE_FOR: Duration = Duration::from_secs(3);

/// Whether a call may go through if tried again shortly: no answer at all,
/// or a server that is starting or stopping.
fn passing(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Http(_) | ClientError::Refused(502..=504, _)
    )
}

/// Calls `call` until it is answered, or a passing failure has lasted
/// [`UNREACHABLE_FOR`].
async fn patiently<T, F, Fut>(call: F) -> Result<T, ClientError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ClientError>>,
{
    patiently_for(UNREACHABLE_FOR, call).await
}

/// [`patiently`], giving up once a passing failure has lasted `for_up_to`.
async fn patiently_for<T, F, Fut>(for_up_to: Duration, mut call: F) -> Result<T, ClientError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ClientError>>,
{
    let deadline = Instant::now() + for_up_to;
    let mut wait = FIRST_WAIT;
    loop {
        match call().await {
            Err(error) if passing(&error) && Instant::now() + wait < deadline => {
                sleep(wait).await;
                wait = (wait * 2).min(LONGEST_WAIT);
            }
            answer => return answer,
        }
    }
}

/// A run's reports, for as long as the host works on it. Its credential is
/// fetched again from the run's start when it runs out.
pub struct RunReports {
    client: HostClient,
    run_id: Uuid,
    start_command_id: Uuid,
    reporter: Mutex<RunReporter>,
    /// Changes reports sent by these reports (named by `opened`), for each
    /// one's key.
    changes_sent: std::sync::atomic::AtomicU64,
    opened: Uuid,
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
    /// The client the reports go through, for the host's other calls made
    /// while the run goes (its plan windows).
    pub fn client(&self) -> &HostClient {
        &self.client
    }

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
            changes_sent: std::sync::atomic::AtomicU64::new(0),
            opened: Uuid::now_v7(),
        })
    }

    async fn fresh(
        client: &HostClient,
        run_id: Uuid,
        start_command_id: Uuid,
    ) -> Result<RunReporter, AppError> {
        let start = patiently(|| client.command(start_command_id))
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
        self.call_with(name, input, key, true, UNREACHABLE_FOR)
            .await
    }

    async fn call_with(
        &self,
        name: &str,
        input: Value,
        key: &str,
        on_run: bool,
        patience: Duration,
    ) -> Result<Value, AppError> {
        let send = async |reporter: RunReporter, input: Value| {
            patiently_for(patience, || {
                let reporter = reporter.clone();
                let input = input.clone();
                async move {
                    if on_run {
                        reporter.operation(name, input, key).await
                    } else {
                        reporter.call_as_agent(name, &input, key).await
                    }
                }
            })
            .await
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
                UNREACHABLE_FOR,
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

    /// Tokens a turn used, and what the harness says they cost (US dollars)
    /// when it does; the server prices the rest from the workspace's
    /// catalog. `report_key` is the same on a retry.
    pub async fn usage(&self, report_key: &str, usage: &Usage<'_>) -> Result<(), AppError> {
        let mut fields = json!({
            "reportKey": report_key,
            "model": usage.model,
            "inputTokens": usage.input_tokens,
            "outputTokens": usage.output_tokens,
            "cachedInputTokens": usage.cached_input_tokens,
        });
        if let Some(cost) = usage
            .cost_usd
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
        {
            fields["cost"] = json!(cost);
            fields["currency"] = json!("USD");
        }
        self.call(
            "run_usage_report",
            fields,
            &format!(
                "usage-{}",
                short(&format!("{}-{report_key}", self.run_id.simple()))
            ),
        )
        .await?;
        Ok(())
    }

    /// What the run changed in its repository against where it started
    /// (A1.17): the server keeps the latest and shows it on the run's page,
    /// reading nothing on this machine's disk. The same changes sent again
    /// are taken once.
    pub async fn changes(&self, diff: &crate::git::RunDiff) -> Result<(), AppError> {
        let fields = serde_json::to_value(diff)
            .map_err(|_| AppError::Internal("serialize the run's changes"))?;
        // Each report has its own key, the same on its retries: a report
        // sent again after a newer one is answered from the first, never
        // written over the newer one, and the same changes sent again later
        // (an edit undone) still count.
        let report = self
            .changes_sent
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let key = format!(
            "changes-{}",
            hex::encode(Sha256::digest(
                format!("{}-{}-{report}", self.run_id.simple(), self.opened.simple()).as_bytes()
            ))
        );
        self.call_with(
            "run_changes_report",
            fields,
            &key,
            true,
            CHANGES_UNREACHABLE_FOR,
        )
        .await?;
        Ok(())
    }
}

impl RunReports {
    /// Words of the reply as they arrive, shown live and never stored. A
    /// piece that isn't taken is gone; the finished reply still arrives.
    pub async fn stream(&self, stream_id: Uuid, delta: String) -> Result<(), AppError> {
        let delta = StreamDelta { stream_id, delta };
        let reporter = self.reporter.lock().await.clone();
        match reporter.stream(&delta).await {
            Err(ClientError::Refused(401, _)) => {
                let fresh = Self::fresh(&self.client, self.run_id, self.start_command_id).await?;
                *self.reporter.lock().await = fresh.clone();
                fresh.stream(&delta).await.map_err(unavailable)
            }
            other => other.map_err(unavailable),
        }
    }
}

/// One turn's tokens on one model.
#[derive(Clone, Copy, Debug)]
pub struct Usage<'a> {
    pub model: &'a str,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cost_usd: Option<f64>,
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
    let mut payload = activity_payload(step, payload, in_thread);
    // A tool call's start and its completion are two activities of one
    // call: their ids differ by step, their toolCallRef is the item's key.
    let id = if kind == "tool_call" {
        payload["toolCallRef"] = json!(client_id(key));
        client_id(&format!("{key}:{step}"))
    } else {
        client_id(key)
    };
    if in_thread {
        payload["inThread"] = Value::Bool(true);
    }
    json!({
        "id": id,
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
/// outcome a finishing step reports. What the agent says in its thread is
/// never cut here: the server keeps up to a whole message of it and posts
/// it, so dropping it would lose the reply.
fn activity_payload(step: &str, payload: Value, in_thread: bool) -> Value {
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
    if !in_thread && payload.to_string().len() > MAX_PAYLOAD_BYTES {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_report_waits_out_a_restarting_server_but_not_a_refusal() {
        let mut tries = 0;
        let answer = patiently(|| {
            tries += 1;
            let tried = tries;
            async move {
                match tried {
                    1 => Err(ClientError::Http("connection refused".to_owned())),
                    2 => Err(ClientError::Refused(503, String::new())),
                    _ => Ok(tried),
                }
            }
        })
        .await;
        assert!(matches!(answer, Ok(3)));

        let mut tries = 0;
        let refused = patiently(|| {
            tries += 1;
            async { Err::<(), _>(ClientError::Refused(409, String::new())) }
        })
        .await;
        assert!(matches!(refused, Err(ClientError::Refused(409, _))));
        assert_eq!(tries, 1, "a refusal is final");
    }

    #[test]
    fn a_long_reply_in_the_thread_keeps_its_text() {
        let long = "word ".repeat(20_000);
        let said = activity("k", "agent_reply", "Said", json!({"text": long}), true);
        assert_eq!(said["payload"]["text"], long.as_str());
        assert_eq!(said["payload"]["inThread"], true);
        let noted = activity("k", "agent_note", "Note", json!({"text": long}), false);
        assert_eq!(noted["payload"]["truncated"], true);
    }

    #[test]
    fn a_tool_calls_start_and_completion_are_two_activities_of_one_call() {
        let started = activity("t1", "command_started", "Ran", json!({}), true);
        let completed = activity("t1", "command_completed", "Ran", json!({}), true);
        assert_ne!(started["id"], completed["id"]);
        assert_eq!(started["payload"]["toolCallRef"], "t1");
        assert_eq!(completed["payload"]["toolCallRef"], "t1");
    }
}

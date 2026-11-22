//! A1.17: the built-in host's turn engine. It listens to a harness for one
//! turn and tells the server what happens the way any host does, through
//! the host protocol ([`RunReports`]): steps, notes, the plan, tokens, plan
//! windows, questions and the reply's words as they arrive. What the host
//! keeps about its own runs (their detail rows, worktrees and sessions) is
//! behind [`TurnHost`], so the engine doesn't know where that is stored.

use std::{future::Future, time::Duration};

use agent_workspace_redaction::{cap, redact};
use serde_json::json;
use tokio::time::{Instant, sleep, sleep_until};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    reply::{ReplyBuffer, ReplyError},
    report::{self, RunReports, Usage},
    requests::{self, Ask, Outcome},
    windows,
};
use crate::{
    RuntimeError,
    harness::{
        adapter::{
            HarnessAnswer, HarnessEvent, HarnessItem, HarnessRequest, HarnessSession, ItemKind,
            RequestKind,
        },
        registry::SupervisorRegistry,
    },
    steps,
};

/// Where a run stands, as far as a turn needs to know.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunPhase {
    /// Still going.
    Going,
    /// Someone stopped it.
    Interrupted,
    /// It has finished another way.
    Ended,
    /// No such run any more.
    Gone,
}

/// What the engine needs from the host it runs in: its own record of the
/// run, and the lease and shutdown signals of the harnesses it drives.
pub trait TurnHost: Send + Sync {
    type Error: From<RuntimeError> + Send;

    /// The harnesses: lease and shutdown signals.
    fn harnesses(&self) -> &SupervisorRegistry;

    /// Where the run stands.
    fn phase(&self, run_id: Uuid) -> impl Future<Output = Result<RunPhase, Self::Error>> + Send;

    /// The run waits on a person's answer.
    fn blocked(&self, run_id: Uuid) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// A request was answered: the run goes on.
    fn go_on(&self, run_id: Uuid) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// An approval was denied, or a request went unanswered: the run stops.
    fn stop_unanswered(
        &self,
        run_id: Uuid,
        approval: bool,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The run failed, for `reason` (shown in its thread).
    fn fail(
        &self,
        run_id: Uuid,
        reason: &'static str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The agent changed files, or ran a command that may have: what the run
    /// changed so far is reported, so its page shows it while it goes, and
    /// after a stop. Best effort: a failure is the host's to log.
    fn files_may_have_changed(&self, run_id: Uuid) -> impl Future<Output = ()> + Send;

    /// The turn completed with the agent's `reply` (perhaps empty).
    fn complete(
        &self,
        run_id: Uuid,
        reply: &str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// One turn of a run: the harness session and where it is.
pub struct Turn {
    pub run_id: Uuid,
    pub provider_thread_id: String,
    pub turn_id: String,
    /// The model the run is on, for tokens a harness reports without one.
    pub model: Option<String>,
}

/// How long a reply's words gather before they go out live.
pub const STREAM_GATHER: Duration = Duration::from_millis(150);

const SIGNED_OUT: &str = "The account this run used was signed out. Its worktree was kept.";

/// Drives one turn to its end. Returns when the turn completes, the run is
/// stopped, or this host stops driving (a shutdown, or the lease lost: the
/// run is then left for whoever recovers it).
pub async fn drive<H: TurnHost>(
    host: &H,
    reports: &RunReports,
    mut provider: Box<dyn HarnessSession>,
    turn: Turn,
) -> Result<(), H::Error> {
    let harnesses = host.harnesses();
    let run_id = turn.run_id;
    let _active = harnesses.turn_started();
    let lease_lost = harnesses.host_lease_lost();
    let stopping = || harnesses.is_shutting_down() || lease_lost.is_cancelled();
    let mut reply = ReplyBuffer::default();
    let stream_id = Uuid::now_v7();
    // The reply's words go out live through the host protocol, gathered for
    // a moment so a fast harness costs a few calls a second, not one a word.
    let mut streamed = String::new();
    let mut stream_due: Option<Instant> = None;
    loop {
        tokio::select! {
            event = provider.next_event() => {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if !stopping() {
                            sleep(Duration::from_millis(100)).await;
                        }
                        if stopping() {
                            provider.shutdown().await;
                            return Ok(());
                        }
                        return Err(harness_error(run_id, "read event", error).into());
                    }
                };
                if !matches!(event, HarnessEvent::MessageDelta(_)) {
                    flush_stream(reports, stream_id, &mut streamed, &mut stream_due).await;
                }
                match event {
                    HarnessEvent::Request(request) => {
                        answer_request(host, reports, &mut provider, run_id, request, &lease_lost)
                            .await?;
                    }
                    HarnessEvent::MessageDelta(delta) => {
                        if let Err(error) = reply.push(&delta) {
                            provider.shutdown().await;
                            return Err(reply_error(run_id, error).into());
                        }
                        streamed.push_str(&delta);
                        stream_due.get_or_insert_with(|| Instant::now() + STREAM_GATHER);
                    }
                    HarnessEvent::ItemStarted(event)
                        if event.kind == ItemKind::CommandExecution => {
                        record_step(reports, &event, "command_started", true).await?;
                    }
                    HarnessEvent::ItemCompleted(event)
                        if event.kind == ItemKind::CommandExecution => {
                        record_step(reports, &event, "command_completed", true).await?;
                        host.files_may_have_changed(run_id).await;
                    }
                    HarnessEvent::ItemCompleted(event) if event.kind == ItemKind::FileChange => {
                        record_step(reports, &event, "file_change", true).await?;
                        host.files_may_have_changed(run_id).await;
                    }
                    HarnessEvent::ItemCompleted(event)
                        if event.kind == ItemKind::Other && event.detail.tool.is_some() => {
                        record_step(reports, &event, "tool_used", false).await?;
                    }
                    HarnessEvent::TurnCompleted { completed } => {
                        if !completed {
                            return Err(RuntimeError::Conflict(
                                "provider",
                                "the agent's turn did not complete successfully",
                            ).into());
                        }
                        host.complete(run_id, reply.as_str()).await?;
                        return Ok(());
                    }
                    HarnessEvent::Error => {
                        return Err(RuntimeError::Conflict(
                            "provider",
                            "the harness reported an error",
                        ).into());
                    }
                    HarnessEvent::UsageUpdated(usage) => {
                        // Like any host's windows; a failure is logged
                        // there, never the run's.
                        let _ = windows::report(reports.client(), provider.account_id(), &usage, true)
                            .await;
                    }
                    HarnessEvent::TokensUsed(usage) => {
                        // Like any host's usage, so budgets count it; usage
                        // is a side note to the run, so a failure is logged,
                        // never the run's.
                        let model = usage
                            .model
                            .clone()
                            .filter(|model| (1..=200).contains(&model.len()))
                            .or_else(|| turn.model.clone())
                            .unwrap_or_else(|| "default".to_owned());
                        let reported = reports
                            .usage(
                                &format!("tokens-{}", Uuid::now_v7().simple()),
                                &Usage {
                                    model: &model,
                                    input_tokens: usage.input_tokens,
                                    output_tokens: usage.output_tokens,
                                    cached_input_tokens: usage.cached_input_tokens,
                                    cost_usd: usage.reported_cost_usd,
                                },
                            )
                            .await;
                        if let Err(error) = reported {
                            tracing::warn!(%error, %run_id, "run usage not recorded");
                        }
                    }
                    HarnessEvent::Note(text) => {
                        if !text.trim().is_empty() {
                            let key = format!("note:{}", Uuid::now_v7());
                            reports.step(
                                &key,
                                "agent_note",
                                &cap(&redact(text.trim()), 160),
                                steps::note_payload(&text),
                                false,
                            ).await?;
                        }
                    }
                    HarnessEvent::Plan(items) => {
                        let items = items
                            .iter()
                            .map(|item| json!({
                                "title": cap(&redact(&item.title), 500),
                                "status": item.status,
                            }))
                            .collect::<Vec<_>>();
                        let key = format!("plan:{}", Uuid::now_v7());
                        reports.step(&key, "plan", "The plan.", json!({ "items": items }), false).await?;
                    }
                    HarnessEvent::ItemStarted(_)
                    | HarnessEvent::ItemCompleted(_)
                    | HarnessEvent::Ignored => {}
                }
            }
            () = async {
                if let Some(due) = stream_due {
                    sleep_until(due).await;
                }
            }, if stream_due.is_some() => {
                flush_stream(reports, stream_id, &mut streamed, &mut stream_due).await;
            }
            // Stop arrives as a wake on the control channel from whichever
            // server took it; the check every second is the safety net for a
            // lost wake or no listener.
            () = stop_wake(harnesses) => {
                if stopping() {
                    provider.shutdown().await;
                    return Ok(());
                }
                if provider.is_stopping() {
                    provider.shutdown().await;
                    host.fail(run_id, SIGNED_OUT).await?;
                    return Ok(());
                }
                match host.phase(run_id).await? {
                    RunPhase::Interrupted => {
                        let _ = provider
                            .interrupt_turn(&turn.provider_thread_id, &turn.turn_id)
                            .await;
                        return Ok(());
                    }
                    RunPhase::Gone => {
                        provider.shutdown().await;
                        return Ok(());
                    }
                    RunPhase::Going | RunPhase::Ended => {}
                }
            }
            () = harnesses.shutdown_requested() => {
                provider.shutdown().await;
                return Ok(());
            }
            () = lease_lost.cancelled() => {
                provider.shutdown().await;
                return Ok(());
            }
        }
    }
}

/// Wait until a stopped run should be looked at: a wake on the control
/// channel, or a second at most.
async fn stop_wake(harnesses: &SupervisorRegistry) {
    tokio::select! {
        () = sleep(Duration::from_secs(1)) => {}
        () = harnesses.lease_released() => {}
    }
}

/// Sends the reply's gathered words, if any. Live words are a courtesy: a
/// failure is logged, never the run's.
async fn flush_stream(
    reports: &RunReports,
    stream_id: Uuid,
    streamed: &mut String,
    due: &mut Option<Instant>,
) {
    *due = None;
    if streamed.is_empty() {
        return;
    }
    let mut piece = std::mem::take(streamed);
    // One call carries at most the protocol's limit; a longer gather (rare)
    // goes out in pieces on character boundaries.
    while !piece.is_empty() {
        let mut end = piece
            .len()
            .min(agent_workspace_host_protocol::MAX_STREAM_DELTA_BYTES);
        while !piece.is_char_boundary(end) {
            end -= 1;
        }
        let rest = piece.split_off(end);
        if let Err(error) = reports.stream(stream_id, piece).await {
            tracing::debug!(%error, "streamed words not shown");
            return;
        }
        piece = rest;
    }
}

/// A harness asks for an approval or an answer: the ask is an `interrupt`
/// the run reports, which opens a request whoever may answer is told of.
async fn answer_request<H: TurnHost>(
    host: &H,
    reports: &RunReports,
    provider: &mut Box<dyn HarnessSession>,
    run_id: Uuid,
    request: HarnessRequest,
    lease_lost: &CancellationToken,
) -> Result<(), H::Error> {
    let harnesses = host.harnesses();
    if request.kind == RequestKind::Unsupported {
        provider
            .answer(request, HarnessAnswer::Deny)
            .await
            .map_err(|error| harness_error(run_id, "reject unsupported provider request", error))?;
        return Ok(());
    }
    let approval = request.kind == RequestKind::Approval;
    let options = request
        .options
        .iter()
        .map(|option| (option.id.clone(), option.label.clone()))
        .collect::<Vec<_>>();
    let asked = requests::interrupt(
        &request.durable_id,
        if approval {
            Ask::Approval
        } else {
            Ask::Question
        },
        &request.summary,
        &options,
    );
    // Raised first: a run shown blocked always has what it waits on.
    let request_id = reports.raise(asked).await?;
    host.blocked(run_id).await?;

    // The answer comes to the host as an `answer` command; the engine reads
    // it with the run's credential. The wake on the control channel is the
    // prompt; the one-second check is the safety net.
    loop {
        tokio::select! {
            () = sleep(Duration::from_secs(1)) => {}
            () = harnesses.lease_released() => {}
            () = lease_lost.cancelled() => {}
        }
        if harnesses.is_shutting_down() || lease_lost.is_cancelled() {
            let _ = provider.answer(request, HarnessAnswer::Deny).await;
            provider.shutdown().await;
            return Ok(());
        }
        if provider.is_stopping() {
            let _ = provider.answer(request, HarnessAnswer::Deny).await;
            provider.shutdown().await;
            host.fail(run_id, SIGNED_OUT).await?;
            return Ok(());
        }
        let outcome = match host.phase(run_id).await? {
            RunPhase::Interrupted | RunPhase::Gone => Outcome::Interrupted,
            RunPhase::Going | RunPhase::Ended => reports.outcome(request_id).await?,
        };
        let answer = match outcome {
            Outcome::Waiting => continue,
            Outcome::Interrupted => {
                provider
                    .answer(request, HarnessAnswer::Deny)
                    .await
                    .map_err(|error| harness_error(run_id, "decline interrupted request", error))?;
                return Ok(());
            }
            Outcome::Approve => HarnessAnswer::Approve,
            Outcome::Deny => HarnessAnswer::Deny,
            Outcome::Text(text) => HarnessAnswer::Text(text),
        };
        // An approval denied, or a request nobody answered, stops the run.
        if matches!(answer, HarnessAnswer::Deny) {
            host.stop_unanswered(run_id, approval).await?;
        } else {
            host.go_on(run_id).await?;
            if host.phase(run_id).await? != RunPhase::Going {
                let _ = provider.answer(request, HarnessAnswer::Deny).await;
                return Ok(());
            }
        }
        provider
            .answer(request, answer)
            .await
            .map_err(|error| harness_error(run_id, "answer provider request", error))?;
        return Ok(());
    }
}

/// Reports one step of a run with its redacted detail, and the tests its
/// output reported as a step of their own.
async fn record_step(
    reports: &RunReports,
    item: &HarnessItem,
    kind: &str,
    in_thread: bool,
) -> Result<(), RuntimeError> {
    let payload = steps::item_payload(item);
    let summary = steps::step_summary(kind, &item.detail);
    let tests = payload.get("tests").cloned();
    let mut activities = vec![report::activity(
        &item.key, kind, &summary, payload, in_thread,
    )];
    if let (Some(tests), "command_completed") = (tests, kind) {
        let passed = tests["passed"].as_u64().unwrap_or(0);
        let failed = tests["failed"].as_u64().unwrap_or(0);
        let summary = if failed == 0 {
            format!("Tests passed: {passed}")
        } else {
            format!("Tests failed: {failed} of {}", passed + failed)
        };
        activities.push(report::activity(
            &format!("{}:tests", item.key),
            "test_result",
            &summary,
            json!({
                "tests": tests,
                "command": item
                    .detail
                    .command
                    .as_deref()
                    .map(|command| cap(&redact(command), 2_000)),
            }),
            false,
        ));
    }
    reports.activities(activities).await
}

/// A harness call failed: logged with its kind, never its text.
pub fn harness_error(
    run_id: Uuid,
    operation: &'static str,
    error: impl std::fmt::Debug,
) -> RuntimeError {
    tracing::error!(
        %run_id,
        operation,
        error_type = std::any::type_name_of_val(&error),
        "harness operation failed"
    );
    RuntimeError::Conflict("provider", "the agent harness operation failed")
}

fn reply_error(run_id: Uuid, error: ReplyError) -> RuntimeError {
    tracing::warn!(
        %run_id,
        validation_error = ?error,
        "the agent's reply passed a message's bounds"
    );
    RuntimeError::Conflict("provider", "the agent's Message output is invalid")
}

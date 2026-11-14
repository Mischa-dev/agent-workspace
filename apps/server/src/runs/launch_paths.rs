//! A1.4: connected runners get runs. A launch to a runner opens a run for
//! the runner (cause: the hand-off message, state `accepted`) and the reply
//! tools the runner already calls report on it, so a runner that never calls
//! a run tool still has a run that shows what it is doing (A1 3.7):
//!
//! - `reply_claim`: `run_started` (the run is `working`).
//! - `reply_progress`: a `step_started` per distinct line, and the status line.
//! - `reply_send` with `final`: `run_finished` done.
//! - `reply_release`: `run_finished` failed, with the reason.
//! - `reply_pass`: `run_finished` cancelled.
//! - Stop on the request: the run is asked to stop.
//!
//! Each follows the request's hand-off message to the runner's own run; a
//! request with no launch behind it has no run and nothing happens.

use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    activities::NewActivity,
    ops::{self, Cause, Reporter, RunOpError, RunOpen, RunUpdate},
    state::RunState,
    store::{RUN_COLUMNS, RunRow},
};
use crate::error::AppError;

/// What a launch knows when it hands off to a runner.
#[derive(Clone, Debug)]
pub struct HandOff {
    pub space_id: Uuid,
    pub launch_id: Uuid,
    pub runner_account_id: Uuid,
    pub message_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub requested_by: Uuid,
    pub title: String,
}

/// Open the runner's run for a hand-off, or find the one already open for
/// this launch. `None` when the runner can't have one there (it can't see
/// the message, or it has too many open): the hand-off goes ahead without.
pub async fn open_for_hand_off(
    transaction: &mut Transaction<'_, Postgres>,
    hand_off: HandOff,
) -> Result<Option<RunRow>, AppError> {
    let title: String = hand_off.title.chars().take(200).collect();
    let opened = ops::open(
        transaction,
        Reporter::Account {
            account_id: hand_off.runner_account_id,
        },
        RunOpen {
            space_id: hand_off.space_id,
            client_run_id: Some(format!("launch:{}", hand_off.launch_id)),
            title: if title.trim().is_empty() {
                "A launched task".to_owned()
            } else {
                title
            },
            cause: Cause {
                kind: if hand_off.work_item_id.is_some() {
                    "task"
                } else {
                    "message"
                }
                .to_owned(),
                conversation_id: None,
                work_item_id: hand_off.work_item_id,
                message_id: Some(hand_off.message_id),
                reference: None,
            },
            parent_run_id: None,
            report_interval_seconds: None,
            settings: None,
            initial: RunState::Accepted,
            requested_by: Some(hand_off.requested_by),
            via_app_id: None,
            launch_id: Some(hand_off.launch_id),
        },
    )
    .await;
    match opened {
        Ok(opened) => Ok(Some(opened.run)),
        Err(RunOpError::App(error)) => Err(error),
        Err(_) => Ok(None),
    }
}

/// The run a launch opened for this reply request's hand-off message, if the
/// request is the account's own.
pub async fn for_reply_request(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    request_id: Uuid,
) -> Result<Option<RunRow>, AppError> {
    Ok(sqlx::query_as::<_, RunRow>(&format!(
        r#"
        SELECT {RUN_COLUMNS}
        FROM reply_requests request
        JOIN runs run ON run.cause_message_id = request.message_id
                     AND run.account_id = request.agent_actor_id
        WHERE request.id = $1 AND request.agent_actor_id = $2
          AND run.launch_id IS NOT NULL
        ORDER BY run.created_at DESC
        LIMIT 1
        "#
    ))
    .bind(request_id)
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// What the runner did with the request.
#[derive(Clone, Debug)]
pub enum ReplyStep<'a> {
    Claimed { generation: i32 },
    Progress { generation: i32, status: &'a str },
    Answered { generation: i32 },
    Released { reason: &'a str },
    Passed,
}

fn activity(
    client_id: String,
    kind: &str,
    summary: Option<String>,
    payload: serde_json::Value,
) -> NewActivity {
    NewActivity {
        client_id,
        kind: kind.to_owned(),
        occurred_at: Utc::now(),
        step_ref: None,
        parent_ref: None,
        summary,
        payload,
    }
}

fn short_digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Report a reply step on the request's run, if it has one. Returns the run
/// as it is afterwards. A step the run can't take (it finished already, or
/// the runner lost sight of it) changes nothing.
pub async fn follow_reply(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    request_id: Uuid,
    step: ReplyStep<'_>,
) -> Result<Option<RunRow>, AppError> {
    let Some(run) = for_reply_request(transaction, account_id, request_id).await? else {
        return Ok(None);
    };
    // A claim after the run finished is a retried request (Retry after a
    // release, a stop or a lost lease): a new attempt, not the old run.
    let run = match step {
        ReplyStep::Claimed { generation } if run.run_state()?.is_terminal() => {
            match next_attempt(transaction, &run, request_id, generation).await? {
                Some(next) => next,
                None => return Ok(Some(run)),
            }
        }
        _ => run,
    };
    let reporter = Reporter::Account { account_id };
    let state = run.run_state()?;
    let batch = match step {
        ReplyStep::Claimed { generation } => {
            if state != RunState::Accepted {
                return Ok(Some(run));
            }
            vec![activity(
                format!("reply-claim:{request_id}:{generation}"),
                "run_started",
                None,
                json!({}),
            )]
        }
        ReplyStep::Progress { generation, status } => {
            let line = ops::one_line(status);
            let changed = ops::update(
                transaction,
                reporter,
                run.id,
                RunUpdate {
                    status_line: Some(line.chars().take(200).collect()),
                    ..RunUpdate::default()
                },
            )
            .await;
            if let Err(RunOpError::App(error)) = changed {
                return Err(error);
            }
            vec![activity(
                format!(
                    "reply-progress:{request_id}:{generation}:{}",
                    short_digest(&line)
                ),
                "step_started",
                Some(line.chars().take(200).collect()),
                json!({ "title": line.chars().take(200).collect::<String>() }),
            )]
        }
        ReplyStep::Answered { generation } => vec![activity(
            format!("reply-final:{request_id}:{generation}"),
            "run_finished",
            Some("Answered.".to_owned()),
            json!({ "outcome": "done" }),
        )],
        ReplyStep::Released { reason } => {
            let reason = ops::one_line(reason);
            let changed = ops::update(
                transaction,
                reporter,
                run.id,
                RunUpdate {
                    failure_reason: Some(reason.chars().take(2_000).collect()),
                    ..RunUpdate::default()
                },
            )
            .await;
            if let Err(RunOpError::App(error)) = changed {
                return Err(error);
            }
            vec![activity(
                format!("reply-release:{request_id}"),
                "run_finished",
                Some("Released.".to_owned()),
                json!({ "outcome": "failed" }),
            )]
        }
        ReplyStep::Passed => vec![activity(
            format!("reply-pass:{request_id}"),
            "run_finished",
            Some("Passed.".to_owned()),
            json!({ "outcome": "cancelled" }),
        )],
    };
    match ops::append(transaction, reporter, run.id, batch).await {
        Ok(appended) => Ok(Some(appended.run)),
        Err(RunOpError::App(error)) => Err(error),
        Err(_) => Ok(Some(run)),
    }
}

/// Open the next attempt of a launched run whose request was retried: the
/// same cause, a new run, and the launch shows it. `None` when the runner
/// can't have one now.
async fn next_attempt(
    transaction: &mut Transaction<'_, Postgres>,
    previous: &RunRow,
    request_id: Uuid,
    generation: i32,
) -> Result<Option<RunRow>, AppError> {
    let (Some(space_id), Some(launch_id)) = (previous.space_id, previous.launch_id) else {
        return Ok(None);
    };
    let opened = ops::open(
        transaction,
        Reporter::Account {
            account_id: previous.account_id,
        },
        RunOpen {
            space_id,
            client_run_id: Some(format!("launch:{launch_id}:{request_id}:{generation}")),
            title: previous.title.clone(),
            cause: Cause {
                kind: previous.cause_kind.clone(),
                conversation_id: None,
                work_item_id: previous.work_item_id,
                message_id: previous.cause_message_id,
                reference: None,
            },
            parent_run_id: None,
            report_interval_seconds: Some(previous.report_interval_seconds),
            settings: None,
            initial: RunState::Accepted,
            requested_by: previous.requested_by_account_id,
            via_app_id: None,
            launch_id: Some(launch_id),
        },
    )
    .await;
    let next = match opened {
        Ok(opened) => opened.run,
        Err(RunOpError::App(error)) => return Err(error),
        Err(_) => return Ok(None),
    };
    let next = sqlx::query_as::<_, RunRow>(&format!(
        "UPDATE runs run SET predecessor_run_id = $2 WHERE run.id = $1 RETURNING {RUN_COLUMNS}"
    ))
    .bind(next.id)
    .bind(previous.id)
    .fetch_one(&mut **transaction)
    .await?;
    sqlx::query("UPDATE launches SET run_id = $2 WHERE id = $1")
        .bind(launch_id)
        .bind(next.id)
        .execute(&mut **transaction)
        .await?;
    Ok(Some(next))
}

/// A person stopped these requests: their runs are asked to stop.
pub async fn stopped(
    transaction: &mut Transaction<'_, Postgres>,
    by: Uuid,
    agent_account_id: Uuid,
    request_ids: &[Uuid],
    reason: Option<String>,
) -> Result<(), AppError> {
    for request_id in request_ids {
        let Some(run) = for_reply_request(transaction, agent_account_id, *request_id).await? else {
            continue;
        };
        match ops::request_cancel(transaction, by, run.id, reason.clone()).await {
            Ok(_) | Err(RunOpError::NotFound | RunOpError::Conflict(_)) => {}
            Err(RunOpError::App(error)) => return Err(error),
            Err(_) => {}
        }
    }
    Ok(())
}

/// The reply request a launched run still answers, if it is open: a stop
/// on the run goes through it so the runner hears it (A1.4).
pub async fn open_request_of(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT request.id
        FROM runs run
        JOIN reply_requests request
          ON request.message_id = run.cause_message_id
         AND request.agent_actor_id = run.account_id
        WHERE run.id = $1 AND run.launch_id IS NOT NULL
          AND request.status IN ('pending', 'claimed')
          AND NOT EXISTS (SELECT 1 FROM agent_runs builtin WHERE builtin.id = run.id)
        ORDER BY request.updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// The runner bound a session in a conversation (`session_bind`): its
/// active launched runs there show where to watch it and what runs it.
pub async fn session_bound(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    conversation_id: Uuid,
    url: Option<&str>,
    backend: Option<&str>,
) -> Result<(), AppError> {
    if url.is_none() && backend.is_none() {
        return Ok(());
    }
    let runs = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT DISTINCT run.id
        FROM runs run
        JOIN reply_requests request
          ON request.message_id = run.cause_message_id
         AND request.agent_actor_id = run.account_id
        WHERE run.account_id = $1 AND run.launch_id IS NOT NULL
          AND COALESCE(request.thread_id, request.conversation_id) = $2
          AND run.state = ANY($3)
        "#,
    )
    .bind(account_id)
    .bind(conversation_id)
    .bind(RunState::active_keys())
    .fetch_all(&mut **transaction)
    .await?;
    for run_id in runs {
        let changed = ops::update(
            transaction,
            Reporter::Account { account_id },
            run_id,
            RunUpdate {
                external_url: url.map(str::to_owned),
                backend: backend.map(str::to_owned),
                ..RunUpdate::default()
            },
        )
        .await;
        if let Err(RunOpError::App(error)) = changed {
            return Err(error);
        }
    }
    Ok(())
}

/// [`follow_reply`] in a savepoint: the reply goes ahead whatever happens to
/// the run, and a failure is logged rather than returned.
pub async fn follow_reply_best_effort(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    request_id: Uuid,
    step: ReplyStep<'_>,
) -> Option<RunRow> {
    let mut savepoint = sqlx::Connection::begin(&mut **transaction).await.ok()?;
    match follow_reply(&mut savepoint, account_id, request_id, step).await {
        Ok(run) => savepoint.commit().await.ok().and(run),
        Err(error) => {
            let _ = savepoint.rollback().await;
            tracing::warn!(
                category = error.log_category(),
                "a reply step wasn't reported on its run"
            );
            None
        }
    }
}

//! A1.14: what the built-in host's agents ask for (an approval to go on, an
//! answer to a question) is a request, raised by the run like any host's
//! (`requests::open_for_run`). Answering it goes through the request
//! operations; [`settled`] then moves the run along and wakes the driver,
//! which reads the answer with [`outcome`] and passes it to the harness
//! once ([`claim_delivery`]). No table of its own and no polling of one.

use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{domain::RunState, error::AppError, outbox, requests::RequestRow};

/// What the harness asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ask {
    /// Approve or deny going on; answered by whoever holds the addressed
    /// role and `runs.approve`.
    Approval,
    /// An answer in words (or one of the offered answers); anyone in the
    /// space who may answer requests.
    Question,
}

/// The role an approval is addressed to: those who hold it and may approve
/// runs answer it. A key in the data, like any request's audience.
const APPROVERS: &str = "owner";

/// The request a harness's ask became. `created` is false when the same
/// ask (by `durable_id`) was raised before, as after a restart.
#[derive(Clone, Copy, Debug)]
pub struct Raised {
    pub request_id: Uuid,
    pub created: bool,
}

/// Raise what the harness asked as a request of the run.
pub async fn raise(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    durable_id: &str,
    ask: Ask,
    summary: &str,
    options: &[(String, String)],
) -> Result<Raised, AppError> {
    // The run's own row first, as every write to it does (`super::lock`).
    super::lock(transaction, run_id)
        .await?
        .ok_or(AppError::NotFound("agent_run"))?;
    let run = crate::runs::ops::locked(transaction, run_id)
        .await?
        .ok_or(AppError::NotFound("agent_run"))?;
    let title = match summary.trim() {
        "" if ask == Ask::Approval => "The agent needs your approval to go on.",
        "" => "The agent has a question.",
        summary => summary,
    };
    let options = options
        .iter()
        .take(8)
        .enumerate()
        .map(|(index, (id, label))| {
            // A harness may name an option by its whole label; a request's
            // option ids are short, so a long one gets a number. The driver
            // answers a question with the label, so nothing is lost.
            let id = if id.trim().is_empty() || id.chars().count() > 40 {
                format!("option-{}", index + 1)
            } else {
                id.clone()
            };
            let label = label.chars().take(200).collect::<String>();
            let mut option = json!({"id": id, "label": label});
            if ask == Ask::Approval && matches!(id.as_str(), "approve" | "deny") {
                option["style"] = Value::String(id.clone());
            }
            option
        })
        .collect::<Vec<_>>();
    let request = json!({
        "kind": if ask == Ask::Approval { "approval" } else { "question" },
        "title": crate::run_log::cap(title, 2_000),
        "options": options,
        "allowText": ask == Ask::Question,
        "audience": if ask == Ask::Approval { json!({"roles": [APPROVERS]}) } else { json!({}) },
    });
    let raised = crate::requests::open_for_run(transaction, &run, durable_id, &request)
        .await
        .map_err(|error| match error {
            crate::requests::OpenError::App(error) => error,
            _ => AppError::Internal("the built-in host's request was refused"),
        })?;
    Ok(Raised {
        request_id: raised.request_id,
        created: raised.created,
    })
}

/// A request of a built-in run was answered, ran out or was withdrawn: say
/// so in the run's steps, go on (an approval granted, a question answered)
/// or stop (an approval denied or unanswered in time), and wake the driver.
pub async fn settled(
    transaction: &mut Transaction<'_, Postgres>,
    row: &RequestRow,
) -> Result<(), AppError> {
    let Some(run_id) = row.run_id else {
        return Ok(());
    };
    wake_driver(transaction).await?;
    if !matches!(row.status.as_str(), "answered" | "expired") {
        return Ok(());
    }
    #[derive(FromRow)]
    struct Place {
        state: String,
        conversation_id: Uuid,
        project_id: Uuid,
    }
    let Some(place) = sqlx::query_as::<_, Place>(
        r#"
        SELECT run.state, run.conversation_id, conversation.project_id
        -- visibility: the run's own thread, to write its steps
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        WHERE run.id = $1
        FOR UPDATE OF run
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(());
    };
    let state = place.state.parse::<RunState>()?;
    if state.is_terminal() {
        return Ok(());
    }
    let question = row.kind != "approval";
    let goes_on = row.status == "answered" && (question || approved(row));
    let resolution = match (question, goes_on) {
        (true, true) => "answered",
        (true, false) => "unanswered",
        (false, true) => "approved",
        (false, false) => "denied",
    };
    let event = crate::runs::steps::record_new(
        transaction,
        run_id,
        if question {
            "question_resolved"
        } else {
            "approval_resolved"
        },
        match resolution {
            "answered" => "The question was answered.",
            "unanswered" => "The question went unanswered.",
            "approved" => "The approval was granted.",
            _ => "The approval was denied.",
        },
        json!({
            "request_id": row.id,
            "resolution": resolution,
            "responded_by_actor_id": row.answered_by_account_id,
        }),
    )
    .await?;
    crate::runs::steps::show_in_thread(transaction, place.conversation_id, event).await?;
    if goes_on {
        if state == RunState::Blocked {
            crate::domain::resume_answered_run(transaction, run_id).await?;
        }
    } else {
        super::transition(
            transaction,
            run_id,
            RunState::Interrupted,
            Some(if question {
                "The question went unanswered."
            } else {
                "The requested approval was denied."
            }),
        )
        .await?;
        crate::hosts::builtin::http::retain_run_workspace(transaction, run_id).await?;
        crate::provider::idle_provider_session_for_terminal_run(transaction, run_id).await?;
        let interrupted = crate::runs::steps::record_new(
            transaction,
            run_id,
            "run_interrupted",
            if question {
                "The run was interrupted after its question went unanswered."
            } else {
                "The run was interrupted after the approval was denied."
            },
            json!({}),
        )
        .await?;
        crate::runs::steps::show_in_thread(transaction, place.conversation_id, interrupted).await?;
    }
    outbox::enqueue_agent_run_updated(transaction, place.project_id, place.conversation_id, run_id)
        .await?;
    Ok(())
}

/// Whether an approval's answer picked an option styled (or named) approve.
fn approved(row: &RequestRow) -> bool {
    let Some(picked) = row
        .answer
        .as_ref()
        .and_then(|answer| answer.get("optionId"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    crate::requests::options_of(&row.options)
        .iter()
        .find(|option| option.id == picked)
        .is_some_and(|option| match option.style.as_deref() {
            Some(style) => style == "approve",
            None => option.id == "approve",
        })
}

/// Tell the built-in host's holder to look now: it is the one driving the
/// run (K5.3). Delivered at commit; the driver also checks every second.
async fn wake_driver(transaction: &mut Transaction<'_, Postgres>) -> Result<(), AppError> {
    sqlx::query("SELECT pg_notify(realtime_channel('control'), 'host:kick')")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// What the driver passes to the harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Nobody has answered yet.
    Waiting,
    Approve,
    Deny,
    /// A question's answer, in words.
    Text(String),
    /// The run was stopped while it waited: the harness is told no.
    Interrupted,
}

/// Where a request of a built-in run stands, for the driver waiting on it.
pub async fn outcome(pool: &PgPool, request_id: Uuid) -> Result<Outcome, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let row = crate::requests::get(&mut transaction, request_id)
        .await?
        .ok_or(AppError::NotFound("request"))?;
    let interrupted = match row.run_id {
        Some(run_id) => {
            sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
                .bind(run_id)
                .fetch_optional(&mut *transaction)
                .await?
                .is_none_or(|state| state == RunState::Interrupted.to_string())
        }
        None => true,
    };
    transaction.commit().await?;
    if interrupted {
        return Ok(Outcome::Interrupted);
    }
    Ok(match row.status.as_str() {
        "open" => Outcome::Waiting,
        "answered" if row.kind == "approval" => {
            if approved(&row) {
                Outcome::Approve
            } else {
                Outcome::Deny
            }
        }
        "answered" => {
            let answer = row.answer.as_ref();
            let typed = answer
                .and_then(|answer| answer.get("text"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let picked = answer
                .and_then(|answer| answer.get("optionId"))
                .and_then(Value::as_str)
                .and_then(|picked| {
                    crate::requests::options_of(&row.options)
                        .into_iter()
                        .find(|option| option.id == picked)
                        .map(|option| option.label)
                });
            match (picked, typed) {
                (Some(picked), Some(typed)) => Outcome::Text(format!("{picked}\n\n{typed}")),
                (Some(answer), None) | (None, Some(answer)) => Outcome::Text(answer),
                (None, None) => Outcome::Deny,
            }
        }
        _ => Outcome::Deny,
    })
}

/// Mark the answer as passed to the harness; false when it already was
/// (another turn of the same run got there first) or it is still open.
pub async fn claim_delivery(pool: &PgPool, request_id: Uuid) -> Result<bool, AppError> {
    Ok(crate::db::scoped_one(pool, async |connection| {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE requests SET delivered_at = now()
             WHERE id = $1 AND status <> 'open' AND delivered_at IS NULL
            RETURNING id
            "#,
        )
        .bind(request_id)
        .fetch_optional(&mut *connection)
        .await
    })
    .await?
    .is_some())
}

//! A1.14: the built-in host's steps are run activities, written straight
//! into the run's log like any host's report (`run_activities`), with the
//! built-in step's own name kept in `payload.step` (`file_change`,
//! `run_failed`, ...). A step shown in the thread gets a timeline entry.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{domain::allocate_conversation_sequence, error::AppError};

/// The longest step payload kept whole; a bigger one keeps only its name.
const MAX_PAYLOAD_BYTES: usize = 60_000;

/// The activity kind a built-in step is recorded as.
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
        "approval_requested" | "question_requested" => "interrupt",
        "approval_resolved" | "question_resolved" => "interrupt_resolved",
        "run_completed" | "run_failed" | "run_interrupted" => "run_finished",
        "agent_note" => "text_message",
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
        return serde_json::json!({"step": step, "truncated": true});
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

/// Record one step of a built-in run. `key` makes it once-only: a step
/// recorded again under the same key changes nothing and returns `None`.
/// Lock the run (`hosts::builtin::lock`) first when the same transaction
/// also changes it.
pub async fn record(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    key: &str,
    step: &str,
    summary: &str,
    payload: Value,
) -> Result<Option<Uuid>, AppError> {
    let client_id = client_id(key);
    let seen = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM run_activities WHERE run_id = $1 AND client_id = $2)",
    )
    .bind(run_id)
    .bind(&client_id)
    .fetch_one(&mut **transaction)
    .await?;
    if seen {
        return Ok(None);
    }
    let kind = activity_kind(step, &payload);
    let payload = activity_payload(step, payload);
    let summary: String = summary.chars().take(2_000).collect();
    let id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH next AS (
            UPDATE runs SET activity_seq = activity_seq + 1
             WHERE id = $2
            RETURNING activity_seq, workspace_id, account_id
        )
        INSERT INTO run_activities (
            id, run_id, workspace_id, seq, client_id, kind, summary, payload,
            occurred_at, reported_by_account_id
        )
        SELECT $1, $2, next.workspace_id, next.activity_seq, $3, $4, $5, $6, now(),
               next.account_id
          FROM next
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(run_id)
    .bind(&client_id)
    .bind(kind)
    .bind(&summary)
    .bind(payload)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(inserted)
}

/// Record a step that happens once by nature (a state change, a failure):
/// its key is its own id.
pub async fn record_new(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    step: &str,
    summary: &str,
    payload: Value,
) -> Result<Uuid, AppError> {
    let key = format!("{step}:{}", Uuid::now_v7());
    record(transaction, run_id, &key, step, summary, payload)
        .await?
        .ok_or(AppError::Internal("run step was not recorded"))
}

/// Show a recorded step in the run's thread.
pub async fn show_in_thread(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
    activity_id: Uuid,
) -> Result<(), AppError> {
    let sequence = allocate_conversation_sequence(transaction, conversation_id).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, run_activity_id
        )
        VALUES ($1, $2, $3, 'worker_event', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation_id)
    .bind(sequence)
    .bind(activity_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("UPDATE run_activities SET in_thread = true WHERE id = $1")
        .bind(activity_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn steps_become_the_activity_kinds_hosts_report() {
        assert_eq!(
            activity_kind("run_state_changed", &json!({"to": "working"})),
            "run_started"
        );
        assert_eq!(
            activity_kind("run_state_changed", &json!({"to": "blocked"})),
            "custom"
        );
        assert_eq!(activity_kind("file_change", &json!({})), "tool_call");
        assert_eq!(activity_kind("approval_requested", &json!({})), "interrupt");
        assert_eq!(activity_kind("run_failed", &json!({})), "run_finished");
        assert_eq!(activity_kind("recovery", &json!({})), "custom");
    }

    #[test]
    fn a_step_keeps_its_name_and_says_how_a_run_ended() {
        let payload = activity_payload("run_failed", json!({"reason": "x"}));
        assert_eq!(payload["step"], "run_failed");
        assert_eq!(payload["outcome"], "failed");
        assert_eq!(payload["reason"], "x");
        let big = activity_payload("agent_note", json!({"text": "a".repeat(70_000)}));
        assert_eq!(big, json!({"step": "agent_note", "truncated": true}));
        assert!(client_id(&"k".repeat(300)).starts_with("sha256:"));
        assert_eq!(client_id("short"), "short");
    }
}

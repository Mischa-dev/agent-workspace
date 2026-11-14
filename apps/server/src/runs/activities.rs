//! A1: activities, typed entries in a run (A1 3.3), named after AG-UI's
//! events. This module checks what a reporter sends and makes it safe to
//! store: every string is redacted and capped whoever reports it.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::{
    ops::RunOpError,
    state::{ActivityEffect, Outcome},
};
use crate::run_log::{self, MAX_PAYLOAD_BYTES};

/// Every kind a reporter may send.
pub const KINDS: [&str; 17] = [
    "run_started",
    "run_finished",
    "run_error",
    "step_started",
    "step_finished",
    "text_message",
    "reasoning",
    "tool_call",
    "plan",
    "state_snapshot",
    "state_delta",
    "interrupt",
    "interrupt_resolved",
    "usage",
    "result",
    "gap",
    "custom",
];

/// The most activities in one call.
pub const MAX_BATCH: usize = 100;
/// The most activities one run keeps; past it only lifecycle, interrupt and
/// result kinds are accepted, and one `gap` records what was refused.
pub const MAX_PER_RUN: i64 = 20_000;
const MAX_SUMMARY_CHARS: usize = 2_000;
const MAX_REF_CHARS: usize = 200;
const MAX_STRING_CHARS: usize = 8_000;

/// One activity as a reporter sends it.
#[derive(Clone, Debug)]
pub struct NewActivity {
    /// The reporter's own id for it; a repeat is a duplicate, never an error.
    pub client_id: String,
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    pub step_ref: Option<String>,
    pub parent_ref: Option<String>,
    pub summary: Option<String>,
    pub payload: Value,
}

/// An activity checked and made safe to store.
#[derive(Clone, Debug)]
pub struct CleanActivity {
    pub client_id: String,
    pub kind: &'static str,
    pub occurred_at: DateTime<Utc>,
    pub step_ref: Option<String>,
    pub parent_ref: Option<String>,
    pub summary: String,
    pub payload: Value,
    pub effect: ActivityEffect,
    /// Lifecycle, interrupt and result kinds are always kept, even past the
    /// per-run limit.
    pub always_kept: bool,
}

fn refused(field: &'static str, message: impl Into<String>) -> RunOpError {
    RunOpError::Refused {
        field,
        message: message.into(),
    }
}

fn one_line(value: &str, max: usize) -> String {
    let flat = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let trimmed = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    trimmed.chars().take(max).collect()
}

/// Redact and cap every string in a JSON value.
pub fn redact_value(value: Value) -> Value {
    match value {
        Value::String(text) => {
            Value::String(run_log::cap(&run_log::redact(&text), MAX_STRING_CHARS))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    (
                        key.chars().take(MAX_REF_CHARS).collect(),
                        redact_value(value),
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

/// A short summary for an activity that came without one.
fn default_summary(kind: &str, payload: &Value) -> String {
    let text = |key: &str| payload.get(key).and_then(Value::as_str);
    match kind {
        "run_started" => "Started.".to_owned(),
        "run_finished" => match text("outcome") {
            Some("failed") => "Failed.".to_owned(),
            Some("cancelled" | "canceled") => "Cancelled.".to_owned(),
            _ => "Finished.".to_owned(),
        },
        "run_error" => text("message").unwrap_or("An error.").to_owned(),
        "step_started" | "step_finished" => text("title").unwrap_or("A step.").to_owned(),
        "text_message" | "reasoning" => text("text").unwrap_or_default().to_owned(),
        "tool_call" => text("tool").unwrap_or("A tool call.").to_owned(),
        "plan" => "The plan.".to_owned(),
        "interrupt" => payload
            .get("request")
            .and_then(|request| request.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Waiting for an answer.")
            .to_owned(),
        "interrupt_resolved" => text("answerSummary").unwrap_or("Answered.").to_owned(),
        "gap" => text("reason")
            .unwrap_or("Some steps weren't reported.")
            .to_owned(),
        "custom" => text("name").unwrap_or("Custom.").to_owned(),
        other => other.replace('_', " "),
    }
}

/// Check one activity and make it safe to store. `open_after` is how many
/// requests the run has open once this one is applied, for
/// `interrupt_resolved`.
pub fn clean(activity: NewActivity, still_open: i64) -> Result<CleanActivity, RunOpError> {
    let client_id = activity.client_id.trim().to_owned();
    if client_id.is_empty()
        || client_id.chars().count() > MAX_REF_CHARS
        || client_id.chars().any(char::is_control)
    {
        return Err(refused(
            "activities.id",
            "Give each activity an id of 1 to 200 characters, unique within the run.",
        ));
    }
    let Some(kind) = KINDS.into_iter().find(|kind| *kind == activity.kind) else {
        return Err(refused(
            "activities.kind",
            format!("Use one of: {}.", KINDS.join(", ")),
        ));
    };
    if !activity.payload.is_object() {
        return Err(refused(
            "activities.payload",
            "An activity's payload is a JSON object.",
        ));
    }
    let reference =
        |value: Option<String>, field: &'static str| -> Result<Option<String>, RunOpError> {
            match value.map(|value| one_line(&value, usize::MAX)) {
                Some(value) if value.is_empty() => Ok(None),
                Some(value) if value.chars().count() > MAX_REF_CHARS => Err(refused(
                    field,
                    "Keep step and parent references to 200 characters.",
                )),
                other => Ok(other),
            }
        };
    let step_ref = reference(activity.step_ref, "activities.stepRef")?;
    let parent_ref = reference(activity.parent_ref, "activities.parentRef")?;
    let payload = redact_value(activity.payload);
    let effect = match kind {
        "run_finished" => {
            let outcome = payload
                .get("outcome")
                .and_then(Value::as_str)
                .and_then(Outcome::parse)
                .ok_or_else(|| {
                    refused(
                        "activities.payload.outcome",
                        "run_finished needs payload.outcome: done, failed or cancelled.",
                    )
                })?;
            ActivityEffect::Finished(outcome)
        }
        "interrupt" => ActivityEffect::Interrupt,
        "interrupt_resolved" => ActivityEffect::Resolved { still_open },
        _ => ActivityEffect::Report,
    };
    match kind {
        "text_message" | "reasoning" if !payload.get("text").is_some_and(Value::is_string) => {
            return Err(refused(
                "activities.payload.text",
                format!("{kind} needs payload.text."),
            ));
        }
        "tool_call" if !payload.get("toolCallRef").is_some_and(Value::is_string) => {
            return Err(refused(
                "activities.payload.toolCallRef",
                "tool_call needs payload.toolCallRef, the same for every report of one call.",
            ));
        }
        "plan" if !payload.get("items").is_some_and(Value::is_array) => {
            return Err(refused(
                "activities.payload.items",
                "plan needs payload.items: the whole list each time, [{title, status}].",
            ));
        }
        "interrupt"
            if !payload.get("request").is_some_and(Value::is_object)
                && !payload.get("requestId").is_some_and(Value::is_string) =>
        {
            return Err(refused(
                "activities.payload.request",
                "interrupt needs payload.request (what you are asking) or payload.requestId.",
            ));
        }
        "interrupt_resolved" if !payload.get("requestId").is_some_and(Value::is_string) => {
            return Err(refused(
                "activities.payload.requestId",
                "interrupt_resolved needs payload.requestId.",
            ));
        }
        _ => {}
    }
    let summary = one_line(
        &run_log::redact(
            activity
                .summary
                .as_deref()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or(&default_summary(kind, &payload)),
        ),
        MAX_SUMMARY_CHARS,
    );
    let payload = if serde_json::to_vec(&payload).map_or(usize::MAX, |bytes| bytes.len())
        > MAX_PAYLOAD_BYTES
    {
        // Keep what says what it was; drop the bulk.
        let mut kept = Map::new();
        for key in [
            "outcome",
            "toolCallRef",
            "tool",
            "status",
            "requestId",
            "name",
        ] {
            if let Some(value) = payload.get(key) {
                kept.insert(key.to_owned(), value.clone());
            }
        }
        kept.insert("truncated".to_owned(), Value::Bool(true));
        Value::Object(kept)
    } else {
        payload
    };
    Ok(CleanActivity {
        client_id,
        kind,
        occurred_at: activity.occurred_at,
        step_ref,
        parent_ref,
        summary,
        payload,
        effect,
        always_kept: matches!(
            kind,
            "run_started"
                | "run_finished"
                | "run_error"
                | "interrupt"
                | "interrupt_resolved"
                | "result"
        ),
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::{NewActivity, clean};
    use crate::runs::state::{ActivityEffect, Outcome};

    fn activity(kind: &str, payload: serde_json::Value) -> NewActivity {
        NewActivity {
            client_id: "a1".to_owned(),
            kind: kind.to_owned(),
            occurred_at: Utc::now(),
            step_ref: None,
            parent_ref: None,
            summary: None,
            payload,
        }
    }

    #[test]
    fn secrets_are_redacted_whoever_reports() {
        let cleaned = clean(
            activity(
                "tool_call",
                json!({"toolCallRef": "t1", "output": "export API_KEY=abcdef1234567890 done",
                       "nested": [{"token": "ghp_abcdefghijklmnopqrstuvwxyz0123"}]}),
            ),
            0,
        )
        .expect("clean");
        let text = cleaned.payload.to_string();
        assert!(!text.contains("abcdef1234567890"), "{text}");
        assert!(
            !text.contains("ghp_abcdefghijklmnopqrstuvwxyz0123"),
            "{text}"
        );
    }

    #[test]
    fn kinds_and_shapes_are_checked() {
        assert!(clean(activity("nonsense", json!({})), 0).is_err());
        assert!(clean(activity("run_finished", json!({})), 0).is_err());
        assert!(clean(activity("text_message", json!({})), 0).is_err());
        assert_eq!(
            clean(activity("run_finished", json!({"outcome": "done"})), 0)
                .expect("finished")
                .effect,
            ActivityEffect::Finished(Outcome::Done)
        );
        let mut blank = activity("run_started", json!({}));
        blank.client_id = "  ".to_owned();
        assert!(clean(blank, 0).is_err());
    }
}

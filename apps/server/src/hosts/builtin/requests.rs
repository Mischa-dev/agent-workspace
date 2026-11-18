//! A1.14, A1.16: what the built-in host's agents ask for (an approval to go
//! on, an answer to a question) is an `interrupt` the run reports through the
//! host protocol, which opens a request like any host's. Answering it moves
//! the run along as for any host; the driver reads the answer with
//! `request_get` and the run's credential ([`outcome_of`]) and passes it to
//! the harness. No table of its own.

use serde_json::{Value, json};

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

/// What the harness asked, as the `interrupt` activity the run reports:
/// reporting it opens the request (once per `durable_id`, so a restart that
/// asks again finds the same one) and tells whoever may answer it.
pub fn interrupt(durable_id: &str, ask: Ask, summary: &str, options: &[(String, String)]) -> Value {
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
    let title = title.chars().take(2_000).collect::<String>();
    let mut activity = super::report::activity(
        durable_id,
        if ask == Ask::Approval {
            "approval_requested"
        } else {
            "question_requested"
        },
        &title,
        json!({}),
        true,
    );
    activity["kind"] = json!("interrupt");
    activity["payload"]["request"] = json!({
        "kind": if ask == Ask::Approval { "approval" } else { "question" },
        "title": title,
        "options": options,
        "allowText": ask == Ask::Question,
        "audience": if ask == Ask::Approval { json!({"roles": [APPROVERS]}) } else { json!({}) },
    });
    activity
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

/// Where a request stands, from its `request_get` view: the driver
/// reads it with the run's credential while the harness waits.
pub fn outcome_of(request: &Value) -> Outcome {
    let answer = &request["answer"];
    match request["status"].as_str() {
        Some("open") => Outcome::Waiting,
        Some("answered") if request["kind"] == "approval" => {
            if approved(request) {
                Outcome::Approve
            } else {
                Outcome::Deny
            }
        }
        Some("answered") => {
            let typed = answer["text"].as_str().map(str::to_owned);
            let picked = answer["label"].as_str().map(str::to_owned);
            match (picked, typed) {
                (Some(picked), Some(typed)) => Outcome::Text(format!("{picked}\n\n{typed}")),
                (Some(answer), None) | (None, Some(answer)) => Outcome::Text(answer),
                (None, None) => Outcome::Deny,
            }
        }
        _ => Outcome::Deny,
    }
}

/// Whether an approval's answer picked an option styled (or named) approve.
fn approved(request: &Value) -> bool {
    let Some(picked) = request["answer"]["optionId"].as_str() else {
        return false;
    };
    request["options"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|option| option["id"] == picked)
        .and_then(|option| option["style"].as_str())
        .map_or(picked == "approve", |style| style == "approve")
}

//! A team's own guards (A2.27): rules a workspace sets on the changes
//! accounts make, checked by the operation layer before any change runs.
//!
//! A guard matches a change by the operation, the kind of thing it acts
//! on, the account making it and the words in its text (each part left
//! empty matches everything), in the workspace or one space of it. A
//! `deny` guard stops the change. A `review` guard holds it: the change is
//! kept with a request to the guard's reviewers (people or agents, who
//! answer it like any request), and the call answers ok with
//! `{"status": "pending_review", "requestId": ...}`. Once a reviewer
//! allows it, the same call made again goes through, once; declined, it
//! is refused. Guards apply to every kind of account alike.

pub mod ops;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    access::Caller,
    error::AppError,
    ops::{OpError, target::Located},
};

/// What the operation layer does with a change.
#[derive(Debug)]
pub enum Verdict {
    /// Run it. `hold` is an allowed review to use up with it, in the
    /// change's own transaction, so a change that fails can be tried again.
    Allow { hold: Option<Uuid> },
    /// A guard stops it.
    Deny { message: String },
    /// Held for review.
    Held { request_id: Uuid },
}

/// Changes no guard holds: answering and withdrawing requests (so a review
/// can always be answered) and the guards themselves.
fn exempt(operation: &str) -> bool {
    operation.starts_with("guard_") || matches!(operation, "request_respond" | "request_cancel")
}

/// The kind of thing a change acts on, as guards name it.
pub fn target_kind(kind: &str) -> &str {
    match kind {
        "project" => "space",
        "organization" => "workspace",
        other => other,
    }
}

#[derive(sqlx::FromRow)]
struct GuardRow {
    id: Uuid,
    name: String,
    action: String,
    operations: Vec<String>,
    target_kinds: Vec<String>,
    terms: Vec<String>,
    accounts: Vec<Uuid>,
    reviewers: Vec<Uuid>,
}

impl GuardRow {
    fn matches(&self, operation: &str, kind: &str, account: Uuid, text: &str) -> bool {
        (self.operations.is_empty() || self.operations.iter().any(|name| name == operation))
            && (self.target_kinds.is_empty() || self.target_kinds.iter().any(|name| name == kind))
            && (self.accounts.is_empty() || self.accounts.contains(&account))
            && (self.terms.is_empty()
                || self
                    .terms
                    .iter()
                    .any(|term| text.contains(&term.to_lowercase())))
    }
}

/// Every string in a call's input, lower case, one per line: what a
/// guard's terms are looked for in.
fn text_of(input: &Value) -> String {
    fn walk(value: &Value, out: &mut String) {
        match value {
            Value::String(text) => {
                out.push_str(&text.to_lowercase());
                out.push('\n');
            }
            Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            Value::Object(fields) => fields.values().for_each(|item| walk(item, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    walk(input, &mut out);
    out
}

/// A call's input as reviewers see it: anything under a key that names a
/// secret (a token, password, key or credential) is hidden.
fn concealed(input: &Value) -> Value {
    fn secret(key: &str) -> bool {
        let key = key.to_ascii_lowercase();
        [
            "token",
            "secret",
            "password",
            "apikey",
            "api_key",
            "credential",
            "authorization",
        ]
        .iter()
        .any(|word| key.contains(word))
    }
    match input {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let shown = if secret(key) && !value.is_null() {
                        Value::String("(hidden)".to_owned())
                    } else {
                        concealed(value)
                    };
                    (key.clone(), shown)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(concealed).collect()),
        other => other.clone(),
    }
}

fn hash_of(input: &Value) -> String {
    hex::encode(Sha256::digest(input.to_string().as_bytes()))
}

/// A request key as a hold keeps it: its digest, never the key.
fn key_hash(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// Check a change against the workspace's guards.
pub async fn check(
    state: &crate::AppState,
    caller: &Caller,
    located: &Located,
    operation: &'static str,
    (kind, target_id): (&str, Uuid),
    input: &Value,
    request_key: Option<&str>,
) -> Result<Verdict, OpError> {
    if exempt(operation) {
        return Ok(Verdict::Allow { hold: None });
    }
    let logged = |error: AppError| OpError::logged(operation, &error);
    let mut transaction = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(|error| logged(error.into()))?;
    let guards = sqlx::query_as::<_, GuardRow>(
        r#"
        SELECT id, name, action, operations, target_kinds, terms, accounts, reviewers
        FROM guards
        WHERE workspace_id = $1 AND enabled
          AND (space_id IS NULL OR space_id = $2)
        ORDER BY action = 'review', created_at, id
        "#,
    )
    .bind(located.workspace_id)
    .bind(located.space_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| logged(error.into()))?;
    if guards.is_empty() {
        return Ok(Verdict::Allow { hold: None });
    }
    let kind = target_kind(kind);
    let text = text_of(input);
    let Some(guard) = guards
        .iter()
        .find(|guard| guard.matches(operation, kind, caller.account_id, &text))
    else {
        return Ok(Verdict::Allow { hold: None });
    };
    if guard.action == "deny" {
        return Ok(Verdict::Deny {
            message: format!("The workspace's guard \"{}\" stops this.", guard.name),
        });
    }
    let mut reviewers = guard
        .reviewers
        .iter()
        .copied()
        .filter(|reviewer| *reviewer != caller.account_id)
        .collect::<Vec<_>>();
    if reviewers.is_empty() {
        // Its only reviewer is the caller: nobody else to ask.
        return Ok(Verdict::Allow { hold: None });
    }
    // A change in a conversation is shown only to reviewers who can read
    // that conversation, and its request is tied to it, so nobody else in
    // the space sees what was going to be said there.
    let conversation = conversation_of(&mut transaction, kind, target_id, input, caller.account_id)
        .await
        .map_err(logged)?;
    if let Some(conversation) = conversation {
        reviewers = sqlx::query_scalar::<_, Uuid>(
            "SELECT reviewer FROM unnest($1::uuid[]) reviewer \
             WHERE conversation_visible_to($2, reviewer)",
        )
        .bind(&reviewers)
        .bind(conversation)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| logged(error.into()))?;
        if reviewers.is_empty() {
            return Ok(Verdict::Deny {
                message: format!(
                    "The guard \"{}\" holds this for review, but none of its reviewers can read \
                     this conversation.",
                    guard.name
                ),
            });
        }
    }
    let verdict = review(
        &mut transaction,
        caller,
        Held {
            located,
            conversation,
            operation,
            input,
            request_key,
            guard,
            reviewers: &reviewers,
        },
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| logged(error.into()))?;
    Ok(verdict)
}

/// The conversation a change is in, when its target is one or is in one.
async fn conversation_of(
    transaction: &mut Transaction<'_, Postgres>,
    kind: &str,
    target_id: Uuid,
    input: &Value,
    caller: Uuid,
) -> Result<Option<Uuid>, AppError> {
    // A call aimed at a space can still carry a conversation, such as a
    // decision recorded from one: that conversation counts, whatever the
    // target, so its words stay with its readers.
    for key in ["conversationId", "sourceConversationId"] {
        if let Some(named) = input
            .get(key)
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        {
            return Ok(Some(named));
        }
    }
    Ok(match kind {
        "conversation" => Some(target_id),
        "message" | "message_record" => sqlx::query_scalar::<_, Uuid>(
            "SELECT conversation_id FROM messages \
             WHERE id = $1 AND conversation_visible_to(conversation_id, $2)",
        )
        .bind(target_id)
        .bind(caller)
        .fetch_optional(&mut **transaction)
        .await?,
        "scheduled_message" => sqlx::query_scalar::<_, Uuid>(
            "SELECT conversation_id FROM scheduled_messages \
             WHERE id = $1 AND author_actor_id = $2 AND conversation_visible_to(conversation_id, $2)",
        )
        .bind(target_id)
        .bind(caller)
        .fetch_optional(&mut **transaction)
        .await?,
        // A conversation's memory page (A2.16) is that conversation's: a
        // change to it goes only to reviewers who can read it.
        "page" => sqlx::query_scalar::<_, Uuid>(
            "SELECT memory_for_id FROM project_pages \
             WHERE id = $1 AND memory_for_kind = 'conversation'",
        )
        .bind(target_id)
        .fetch_optional(&mut **transaction)
        .await?,
        _ => None,
    })
}

/// A call a review guard holds.
struct Held<'a> {
    located: &'a Located,
    conversation: Option<Uuid>,
    operation: &'static str,
    input: &'a Value,
    /// The call's request key, when it has one.
    request_key: Option<&'a str>,
    guard: &'a GuardRow,
    reviewers: &'a [Uuid],
}

/// The review of one call: asked once, then settled by its request.
async fn review(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    held: Held<'_>,
) -> Result<Verdict, OpError> {
    let Held {
        located,
        conversation,
        operation,
        input,
        request_key,
        guard,
        reviewers,
    } = held;
    let logged = |error: AppError| OpError::logged(operation, &error);
    let hash = hash_of(input);
    let held = sqlx::query_as::<_, (Uuid, Uuid, String, Option<Value>)>(
        r#"
        SELECT hold.id, hold.request_id, request.status, request.answer
        FROM guard_holds hold
        JOIN requests request ON request.id = hold.request_id
        WHERE hold.account_id = $1 AND hold.operation = $2 AND hold.input_hash = $3
          AND hold.workspace_id = $4 AND hold.guard_id = $5
          AND hold.space_id IS NOT DISTINCT FROM $6
          AND hold.state = 'pending'
        FOR UPDATE OF hold
        "#,
    )
    .bind(caller.account_id)
    .bind(operation)
    .bind(&hash)
    .bind(located.workspace_id)
    .bind(guard.id)
    .bind(located.space_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| logged(error.into()))?;
    if let Some((hold, request_id, status, answer)) = held {
        let allowed = answer
            .as_ref()
            .and_then(|answer| answer.get("optionId"))
            .and_then(Value::as_str)
            == Some("allow");
        return match (status.as_str(), allowed) {
            ("open", _) => Ok(Verdict::Held { request_id }),
            ("answered", true) => Ok(Verdict::Allow { hold: Some(hold) }),
            _ => {
                settle(transaction, hold, "declined")
                    .await
                    .map_err(logged)?;
                let reason = answer
                    .as_ref()
                    .and_then(|answer| answer.get("text"))
                    .and_then(Value::as_str)
                    .map(|text| format!(" ({})", text.chars().take(300).collect::<String>()))
                    .unwrap_or_default();
                Ok(Verdict::Deny {
                    message: format!(
                        "A reviewer of the guard \"{}\" didn't allow this{reason}.",
                        guard.name
                    ),
                })
            }
        };
    }
    // The same keyed call that an allowed review already let through goes
    // on to its command, which answers it from its own record of the first
    // run (a lost answer doesn't ask the reviewers again). Only while that
    // record lasts: a receipt lives 24 hours from its reservation, which
    // comes after the review was used, so a retry inside 24 hours of the
    // use always finds it and replays rather than writing again.
    if let Some(key) = request_key {
        let used = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM guard_holds
                WHERE account_id = $1 AND operation = $2 AND key_hash = $3
                  AND input_hash = $4 AND workspace_id = $5 AND guard_id = $6
                  AND space_id IS NOT DISTINCT FROM $7
                  AND state = 'used' AND updated_at > now() - interval '24 hours'
            )
            "#,
        )
        .bind(caller.account_id)
        .bind(operation)
        .bind(key_hash(key))
        .bind(&hash)
        .bind(located.workspace_id)
        .bind(guard.id)
        .bind(located.space_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(|error| logged(error.into()))?;
        if used {
            return Ok(Verdict::Allow { hold: None });
        }
    }
    let space_id = match located.space_id {
        Some(space) => space,
        None => crate::spaces::workspace::ensure(transaction, located.workspace_id)
            .await
            .map_err(logged)?,
    };
    let hold = Uuid::now_v7();
    let shown = crate::run_log::cap(
        &crate::run_log::redact(
            &serde_json::to_string_pretty(&concealed(input)).unwrap_or_default(),
        ),
        6_000,
    );
    let raised = crate::requests::open(
        transaction,
        crate::requests::Origin {
            account_id: caller.account_id,
            workspace_id: located.workspace_id,
            space_id: Some(space_id),
            run_id: None,
            host_request_ref: None,
            conversation_id: conversation,
            work_item_id: None,
            subject: Some(("guard_hold", hold)),
            field: "guard",
        },
        &json!({
            "kind": "approval",
            "title": format!("Allow this {operation} call? (guard \"{}\")", guard.name),
            "detail": format!("The call, as it would run:\n\n{shown}"),
            "options": [
                {"id": "allow", "label": "Allow", "style": "approve"},
                {"id": "deny", "label": "Don't allow", "style": "deny"},
            ],
            "allowText": true,
            "audience": {"accounts": reviewers},
            "expiresInSeconds": 7 * 86_400,
        }),
    )
    .await
    .map_err(OpError::from)?;
    crate::requests::announce_raised(transaction, raised.request_id)
        .await
        .map_err(logged)?;
    sqlx::query(
        r#"
        INSERT INTO guard_holds (
            id, workspace_id, space_id, guard_id, account_id, operation, input_hash,
            request_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(hold)
    .bind(located.workspace_id)
    .bind(located.space_id)
    .bind(guard.id)
    .bind(caller.account_id)
    .bind(operation)
    .bind(&hash)
    .bind(raised.request_id)
    .execute(&mut **transaction)
    .await
    .map_err(|error| logged(error.into()))?;
    Ok(Verdict::Held {
        request_id: raised.request_id,
    })
}

async fn settle(
    transaction: &mut Transaction<'_, Postgres>,
    hold: Uuid,
    state: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE guard_holds SET state = $2, updated_at = now() WHERE id = $1")
        .bind(hold)
        .bind(state)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// Use up an allowed review with the change it allowed, in the change's
/// transaction: a change that then fails leaves the review to use again,
/// and of two calls at once only one uses it.
pub async fn use_hold(
    transaction: &mut Transaction<'_, Postgres>,
    hold: Uuid,
    request_key: Option<&str>,
) -> Result<(), OpError> {
    let used = sqlx::query(
        "UPDATE guard_holds SET state = 'used', key_hash = $2, updated_at = now() \
         WHERE id = $1 AND state = 'pending'",
    )
    .bind(hold)
    .bind(request_key.map(key_hash))
    .execute(&mut **transaction)
    .await
    .map_err(|error| OpError::logged("guard", &error.into()))?
    .rows_affected();
    if used == 0 {
        return Err(OpError::Conflict {
            reason: crate::ops::ConflictReason::State {
                code: "review_used",
            },
            message: "The review that allowed this was used by another call.".into(),
            current: None,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn reviewers_never_see_a_calls_secrets() {
        let shown = super::concealed(&json!({
            "name": "Team host",
            "token": "abc123",
            "auth": {"apiKey": "k", "headers": [{"Authorization": "Bearer x"}]},
            "clientSecret": null,
        }));
        assert_eq!(
            shown,
            json!({
                "name": "Team host",
                "token": "(hidden)",
                "auth": {"apiKey": "(hidden)", "headers": [{"Authorization": "(hidden)"}]},
                "clientSecret": null,
            })
        );
    }
}

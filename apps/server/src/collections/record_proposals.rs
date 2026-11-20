//! Proposals on records (T1.41): someone whose role only suggests on
//! records (`access::write_mode` answers `Propose`) changes one as usual,
//! through `record_update`, and the change is kept as a proposal instead.
//!
//! The change is worked out by running the write inside a savepoint and
//! rolling it back, so it says exactly what the real write would set: each
//! field with how it read before and after, and the stored values it
//! replaced and writes. All of it is one change (one history row, one
//! version) when accepted. Accepting writes it as its author's change, says
//! who accepted it in the history, and is refused when any field it sets
//! changed since, so it never undoes a change the author hadn't seen. Its
//! request card asks the people who edit the record to decide it
//! (`proposals::raise_request`).

use serde_json::{Value, json};
use sqlx::{Connection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    Collection,
    records::{self, Record, RecordInput, RecordRef, WriteProblem, Written},
};
use crate::{
    error::AppError,
    events::{self, NewEvent, catalog},
    memory,
    page_docs::suggestions::{Decided, Decision, Refusal},
    work_item_fields::Writer,
};

/// The most proposals one author keeps waiting on one record.
const MAX_PENDING_PER_AUTHOR: i64 = 10;

/// What proposing came to.
#[derive(Debug)]
pub enum Proposed {
    Made {
        id: Uuid,
        changes: Vec<Value>,
    },
    /// The change wouldn't change anything.
    Unchanged,
    /// The change itself is wrong, as the direct write would have said.
    Problem(WriteProblem),
    Refused(&'static str),
}

/// Keeps `input` (a change to `record`) as a proposal by `author`. The
/// caller has checked the author may suggest on the record, and locked the
/// collection.
#[allow(clippy::too_many_arguments)]
pub async fn propose(
    transaction: &mut Transaction<'_, Postgres>,
    author: Writer<'_>,
    via_grant_id: Option<Uuid>,
    collection: &Collection,
    record: &Record,
    may_build: bool,
    input: RecordInput,
    note: Option<&str>,
) -> Result<Proposed, AppError> {
    let space_id = collection.project_id;
    if input.archived.is_some() {
        return Ok(Proposed::Refused(
            "Your role here suggests changes to records: you can propose a title and field \
             values, but archiving or restoring a record needs edit access.",
        ));
    }
    let definitions = super::field_definitions(transaction, space_id, collection.id, true).await?;
    // Run the write for real, read what it set, and roll it back.
    let changes = {
        let mut trial = (**transaction).begin().await?;
        let written = records::write(
            &mut trial,
            space_id,
            collection,
            author,
            may_build,
            &definitions,
            "fields",
            &[input],
        )
        .await?;
        let changes = match written {
            Err(problem) => {
                trial.rollback().await?;
                return Ok(Proposed::Problem(problem));
            }
            Ok(written) if matches!(written.first(), Some(Written::Changed { .. })) => {
                sqlx::query_scalar::<_, Value>(
                    "SELECT changes FROM collection_record_history \
                     WHERE record_id = $1 ORDER BY version DESC, created_at DESC LIMIT 1",
                )
                .bind(record.id)
                .fetch_one(&mut *trial)
                .await?
            }
            Ok(_) => Value::Array(Vec::new()),
        };
        trial.rollback().await?;
        changes.as_array().cloned().unwrap_or_default()
    };
    if changes.is_empty() {
        return Ok(Proposed::Unchanged);
    }
    if changes.iter().any(|change| change.get("is").is_none()) {
        return Ok(Proposed::Refused(
            "That value is too large to propose. Ask someone who edits the record to set it.",
        ));
    }
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM proposals \
         WHERE thing_kind = 'record' AND thing_id = $1 AND author_actor_id = $2 \
           AND status = 'pending'",
    )
    .bind(record.id)
    .bind(author.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if pending >= MAX_PENDING_PER_AUTHOR {
        return Ok(Proposed::Refused(
            "You have too many proposals waiting on this record. Wait for some to be decided, \
             or withdraw them.",
        ));
    }
    let id = Uuid::now_v7();
    let workspace_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO proposals (
            id, workspace_id, space_id, thing_kind, thing_id, operation, author_actor_id,
            via_oauth_grant_id, note, base_version, change, preview
        )
        SELECT $1, project.organization_id, project.id, 'record', $3, 'record_update', $4, $5,
               $6, $7, jsonb_build_object('collectionId', $8::uuid, 'number', $9::bigint,
                                          'changes', $10::jsonb),
               jsonb_build_object('fields', (SELECT count(*) FROM jsonb_array_elements($10::jsonb)))
        FROM projects project WHERE project.id = $2
        RETURNING workspace_id
        "#,
    )
    .bind(id)
    .bind(space_id)
    .bind(record.id)
    .bind(author.actor_id)
    .bind(via_grant_id)
    .bind(note)
    .bind(record.version)
    .bind(collection.id)
    .bind(record.number)
    .bind(Value::Array(changes.clone()))
    .fetch_one(&mut **transaction)
    .await?;
    announce(
        transaction,
        workspace_id,
        space_id,
        record.id,
        author.actor_id,
    )
    .await?;
    Ok(Proposed::Made { id, changes })
}

/// Accepts, rejects or withdraws a proposal on record `record_id`.
/// `may_edit` is whether `decider` edits the record directly, `may_build`
/// whether they may set builder-only fields; accepting and rejecting need
/// `may_edit`, and only the author withdraws.
pub async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    decider: memory::Writer,
    may_edit: bool,
    may_build: bool,
    record_id: Uuid,
    proposal_id: Uuid,
    decision: Decision,
) -> Result<Result<Decided, Refusal>, AppError> {
    let Some((space_id, workspace_id, author_id, status, change)) =
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Value)>(
            "SELECT space_id, workspace_id, author_actor_id, status, change \
             FROM proposals WHERE id = $1 AND thing_kind = 'record' AND thing_id = $2 \
             FOR UPDATE",
        )
        .bind(proposal_id)
        .bind(record_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    if status != "pending" {
        return Ok(Err(Refusal::AlreadyDecided(status)));
    }
    match decision {
        Decision::Withdraw if decider.actor_id != author_id => {
            return Ok(Err(Refusal::NotYours(
                "Only its author can withdraw a proposal. Reject it instead.",
            )));
        }
        Decision::Accept | Decision::Reject if !may_edit => {
            return Ok(Err(Refusal::NotYours(
                "Deciding a proposal needs edit access to the record. Ask someone who edits it.",
            )));
        }
        _ => {}
    }
    let collection_id = change
        .get("collectionId")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<Uuid>().ok())
        .ok_or(AppError::Internal("record proposal without its collection"))?;
    let Some(collection) = super::lock(transaction, space_id, collection_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    let mut version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM collection_records WHERE id = $1 AND project_id = $2",
    )
    .bind(record_id)
    .bind(space_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("record"))?;
    let status = match decision {
        Decision::Accept => {
            let Some(current) = records::find(
                &mut **transaction,
                space_id,
                collection.id,
                RecordRef::Id(record_id),
                false,
            )
            .await?
            else {
                return Ok(Err(Refusal::NotFound));
            };
            if current.archived {
                return Ok(Err(Refusal::Archived));
            }
            let changes = change
                .get("changes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut title = None;
            let mut values = Vec::new();
            for entry in &changes {
                let field = entry
                    .get("field")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let name = entry.get("name").and_then(Value::as_str).unwrap_or(field);
                let was = entry.get("was").unwrap_or(&Value::Null);
                let is = entry.get("is").cloned().unwrap_or(Value::Null);
                let now = if field == "title" {
                    Value::String(current.title.clone())
                } else {
                    current
                        .field_values
                        .get(field)
                        .cloned()
                        .unwrap_or(Value::Null)
                };
                if &now != was {
                    return Ok(Err(Refusal::Stale(format!(
                        "\"{name}\" has changed since it was proposed."
                    ))));
                }
                if field == "title" {
                    title = is.as_str().map(str::to_owned);
                } else {
                    values.push((field.to_owned(), is));
                }
            }
            let project_key =
                sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                    .bind(space_id)
                    .fetch_one(&mut **transaction)
                    .await?;
            let definitions =
                super::field_definitions(transaction, space_id, collection.id, true).await?;
            // The change is the author's: the history says so, and who
            // accepted it.
            let written = records::write(
                transaction,
                space_id,
                &collection,
                Writer {
                    actor_id: author_id,
                    project_key: &project_key,
                },
                may_build,
                &definitions,
                "fields",
                &[RecordInput {
                    record: Some(RecordRef::Id(record_id)),
                    title,
                    values,
                    archived: None,
                    expected_version: None,
                }],
            )
            .await?;
            match written {
                Ok(written) => {
                    if let Some(Written::Changed { record, .. }) = written.first() {
                        version = record.version;
                        sqlx::query(
                            "UPDATE collection_record_history SET accepted_by_actor_id = $3 \
                             WHERE project_id = $1 AND record_id = $2 AND version = $4",
                        )
                        .bind(space_id)
                        .bind(record_id)
                        .bind(decider.actor_id)
                        .bind(version)
                        .execute(&mut **transaction)
                        .await?;
                        crate::outbox::enqueue_collection_updated(
                            transaction,
                            space_id,
                            collection.id,
                            &[record_id],
                        )
                        .await?;
                    }
                }
                Err(problem) => return Ok(Err(Refusal::Stale(problem.message))),
            }
            "accepted"
        }
        Decision::Reject => "rejected",
        Decision::Withdraw => "withdrawn",
    };
    let applied = status == "accepted";
    sqlx::query(
        "UPDATE proposals SET status = $2, decided_by_actor_id = $3, decided_at = now(), \
                applied_version = $4, updated_at = now() \
         WHERE id = $1",
    )
    .bind(proposal_id)
    .bind(status)
    .bind(decider.actor_id)
    .bind(applied.then_some(version))
    .execute(&mut **transaction)
    .await?;
    announce(
        transaction,
        workspace_id,
        space_id,
        record_id,
        decider.actor_id,
    )
    .await?;
    Ok(Ok(Decided { status, version }))
}

/// Tells the record's readers its proposals changed (ids only).
async fn announce(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    space_id: Uuid,
    record_id: Uuid,
    actor: Uuid,
) -> Result<(), AppError> {
    events::emit(
        transaction,
        NewEvent::thing(
            &catalog::RECORD_PROPOSALS_CHANGED,
            space_id,
            "record",
            record_id,
        )
        .actor(actor)
        .workspace(workspace_id)
        .payload(json!({})),
    )
    .await?;
    Ok(())
}

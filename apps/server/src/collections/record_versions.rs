//! A record's earlier versions (T1.41). A record's history keeps each
//! change, not whole versions; since T1.41 each change also keeps the
//! stored values it replaced and wrote (`was`, `is`). A version is the
//! record as it is now with every later change taken back: for each field
//! changed since, the value its first later change replaced.
//!
//! Showing one and putting one back go through the history seam
//! (`history_show`, `history_restore`); putting back is a write like any
//! other (`records::write`), so the field rules hold and the history says
//! who did it.

use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::records::{self, RecordInput, RecordRef, Written};
use crate::{error::AppError, work_item_fields::Writer};

/// Why a version can't be shown or put back.
pub const NOT_KEPT: &str = "That version is from before a record's history kept its values, \
     or a value then was too large to keep. Choose a later version.";

/// What differs between a version and now.
#[derive(Debug, Default)]
pub struct Earlier {
    /// The title then, when it differs.
    pub title: Option<String>,
    /// Stored values then, by field id, for fields that differ (null: the
    /// field was empty).
    pub values: Vec<(String, Value)>,
    /// How each of them read then: {"field", "name", "then"}.
    pub shown: Vec<Value>,
}

impl Earlier {
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.values.is_empty()
    }

    pub fn to_json(&self, version: i64) -> Value {
        json!({
            "version": version,
            "title": self.title,
            "fields": self.shown,
        })
    }
}

/// The record `id` at `version`: `None` when there's no such version,
/// `Some(Err(NOT_KEPT))` when its values weren't kept.
pub async fn at_version(
    connection: &mut PgConnection,
    id: Uuid,
    version: i64,
) -> Result<Option<Result<Earlier, &'static str>>, AppError> {
    let Some((now, title, values)) = sqlx::query_as::<_, (i64, String, Value)>(
        "SELECT version, title, field_values FROM collection_records WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    if version < 1 || version > now {
        return Ok(None);
    }
    let later = sqlx::query_scalar::<_, Value>(
        "SELECT changes FROM collection_record_history \
         WHERE record_id = $1 AND version > $2 ORDER BY version, created_at",
    )
    .bind(id)
    .bind(version)
    .fetch_all(&mut *connection)
    .await?;
    let mut then = Map::new();
    let mut shown = Map::new();
    for changes in later {
        for change in changes.as_array().into_iter().flatten() {
            let Some(field) = change.get("field").and_then(Value::as_str) else {
                continue;
            };
            if then.contains_key(field) {
                continue;
            }
            let Some(was) = change.get("was") else {
                return Ok(Some(Err(NOT_KEPT)));
            };
            then.insert(field.to_owned(), was.clone());
            shown.insert(
                field.to_owned(),
                json!({
                    "field": field,
                    "name": change.get("name").cloned().unwrap_or(Value::Null),
                    "then": change.get("before").cloned().unwrap_or(Value::Null),
                }),
            );
        }
    }
    let mut earlier = Earlier::default();
    for (field, was) in then {
        if field == "title" {
            let was = was.as_str().unwrap_or_default().to_owned();
            if was != title {
                earlier.title = Some(was);
                earlier.shown.extend(shown.remove("title"));
            }
            continue;
        }
        if values.get(&field).unwrap_or(&Value::Null) != &was {
            earlier.shown.extend(shown.remove(&field));
            earlier.values.push((field, was));
        }
    }
    Ok(Some(Ok(earlier)))
}

/// What putting a version back came to.
pub enum Restored {
    /// The record's version now.
    Done(i64),
    NotFound,
    /// Someone changed it since it was read.
    Moved(i64),
    /// Why it can't be put back (not kept, the same as now, a field's rules).
    Refused(String),
}

/// Puts `version` of record `id` back as a new version written by `writer`.
/// `may_build`: whether they may set builder-only fields (T1.19).
pub async fn restore(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer<'_>,
    may_build: bool,
    id: Uuid,
    version: i64,
    expected_version: i64,
) -> Result<Restored, AppError> {
    let Some((space_id, collection_id)) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT project_id, collection_id FROM collection_records WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Restored::NotFound);
    };
    let Some(collection) = super::lock(transaction, space_id, collection_id).await? else {
        return Ok(Restored::NotFound);
    };
    // A two-way relation's pair locks come before the record's row lock,
    // as in `records::write`: an edit from the other side holds the pair and
    // then waits for this record.
    let definitions = super::field_definitions(transaction, space_id, collection.id, true).await?;
    let ids = definitions
        .iter()
        .map(|field| field.id.to_string())
        .collect::<Vec<_>>();
    super::relations::lock_pairs(
        transaction,
        super::relations::pair_locks(space_id, &definitions, ids.iter().map(String::as_str)),
    )
    .await?;
    let now = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM collection_records WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut **transaction)
    .await?;
    if now != expected_version {
        return Ok(Restored::Moved(now));
    }
    let earlier = match at_version(transaction, id, version).await? {
        None => return Ok(Restored::NotFound),
        Some(Err(why)) => return Ok(Restored::Refused(why.to_owned())),
        Some(Ok(earlier)) => earlier,
    };
    if earlier.is_empty() {
        return Ok(Restored::Refused(
            "That version is how the record is now.".to_owned(),
        ));
    }
    let written = records::write(
        transaction,
        space_id,
        &collection,
        writer,
        may_build,
        &definitions,
        "version",
        &[RecordInput {
            record: Some(RecordRef::Id(id)),
            title: earlier.title,
            values: earlier.values,
            archived: None,
            expected_version: Some(expected_version),
        }],
    )
    .await?;
    let record = match written {
        Ok(written) => match written.into_iter().next() {
            Some(Written::Changed { record, .. } | Written::Unchanged(record)) => record,
            _ => return Ok(Restored::NotFound),
        },
        Err(problem) => {
            return Ok(Restored::Refused(format!(
                "That version can't be put back as the record is now: {}",
                problem.message
            )));
        }
    };
    crate::outbox::enqueue_collection_updated(transaction, space_id, collection.id, &[id]).await?;
    Ok(Restored::Done(record.version))
}

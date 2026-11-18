//! Two-way relations (T1.18): a relation field whose settings name its pair
//! (`FieldConfig::inverse`) on the other collection. When a record's
//! relation gains or loses a record, that record's pair gains or loses this
//! one, in the same transaction, so the two sides always agree.
//!
//! The pair's records are locked in ID order before they change. Their
//! worked-out values are refreshed with them, the records whose rollups
//! read them are queued (`computed::mark_dependents`), and one
//! `collection.updated` event names them with the cause "paired".

use std::collections::HashSet;

use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    error::AppError,
    fields::{self, FieldDefinition, FieldType, MAX_RELATED},
};

/// What one record's write did to its relations' pairs.
pub struct PairChange {
    pub collection_id: Uuid,
    pub records: Vec<Uuid>,
}

/// Keeps the pairs of `record`'s two-way relations in step with its values
/// going from `before` to `after` (its stored values; `before` None for a
/// new record). Refuses, with the message, when a record would name more
/// than `MAX_RELATED` records.
pub async fn write_pairs(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    record: Uuid,
    definitions: &[FieldDefinition],
    before: Option<&Value>,
    after: &Value,
) -> Result<Result<Vec<PairChange>, String>, AppError> {
    let mut changes = Vec::new();
    for field in definitions
        .iter()
        .filter(|field| field.field_type == FieldType::Relation)
    {
        let (Some(pair), Some(collection)) = (field.config.inverse, field.config.collection) else {
            continue;
        };
        let key = field.id.to_string();
        let old = before
            .and_then(|values| values.get(&key))
            .map(|value| fields::referenced_ids(field, value))
            .unwrap_or_default()
            .into_iter()
            .collect::<HashSet<_>>();
        let new = after
            .get(&key)
            .map(|value| fields::referenced_ids(field, value))
            .unwrap_or_default()
            .into_iter()
            .collect::<HashSet<_>>();
        let mut added = new.difference(&old).copied().collect::<Vec<_>>();
        let mut removed = old.difference(&new).copied().collect::<Vec<_>>();
        if added.is_empty() && removed.is_empty() {
            continue;
        }
        added.sort_unstable();
        removed.sort_unstable();
        let mut all = added.iter().chain(&removed).copied().collect::<Vec<_>>();
        all.sort_unstable();
        // In ID order, so two writes pairing the same records can't
        // deadlock.
        sqlx::query(
            "SELECT id FROM collection_records \
             WHERE project_id = $1 AND collection_id = $2 AND id = ANY($3) \
             ORDER BY id FOR UPDATE",
        )
        .bind(project_id)
        .bind(collection)
        .bind(&all)
        .execute(&mut *connection)
        .await?;
        let pair_key = pair.to_string();
        let own = record.to_string();
        sqlx::query(
            r#"
            UPDATE collection_records
            SET field_values = jsonb_set(
                    field_values, ARRAY[$4],
                    COALESCE(field_values -> $4, '[]'::jsonb) || to_jsonb($5::text)),
                version = version + 1, updated_at = now(), updated_by_actor_id = $6
            WHERE project_id = $1 AND collection_id = $2 AND id = ANY($3)
              AND NOT COALESCE(field_values -> $4, '[]'::jsonb) @> to_jsonb(ARRAY[$5::text])
            "#,
        )
        .bind(project_id)
        .bind(collection)
        .bind(&added)
        .bind(&pair_key)
        .bind(&own)
        .bind(actor_id)
        .execute(&mut *connection)
        .await?;
        let over = sqlx::query_as::<_, (i64, String)>(
            "SELECT number, title FROM collection_records \
             WHERE project_id = $1 AND collection_id = $2 AND id = ANY($3) \
               AND jsonb_array_length(COALESCE(field_values -> $4, '[]'::jsonb)) > $5 \
             ORDER BY number LIMIT 1",
        )
        .bind(project_id)
        .bind(collection)
        .bind(&added)
        .bind(&pair_key)
        .bind(MAX_RELATED as i32)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some((number, title)) = over {
            return Ok(Err(format!(
                "#{number} {title} already names {MAX_RELATED} records through \"{}\"'s other side, the most it can.",
                field.name
            )));
        }
        sqlx::query(
            r#"
            UPDATE collection_records
            SET field_values = CASE
                    WHEN (field_values -> $4) - $5 = '[]'::jsonb THEN field_values - $4
                    ELSE jsonb_set(field_values, ARRAY[$4], (field_values -> $4) - $5) END,
                version = version + 1, updated_at = now(), updated_by_actor_id = $6
            WHERE project_id = $1 AND collection_id = $2 AND id = ANY($3)
              AND jsonb_typeof(field_values -> $4) = 'array'
              AND (field_values -> $4) ? $5
            "#,
        )
        .bind(project_id)
        .bind(collection)
        .bind(&removed)
        .bind(&pair_key)
        .bind(&own)
        .bind(actor_id)
        .execute(&mut *connection)
        .await?;
        match changes
            .iter_mut()
            .find(|change: &&mut PairChange| change.collection_id == collection)
        {
            Some(change) => change.records.extend(all),
            None => changes.push(PairChange {
                collection_id: collection,
                records: all,
            }),
        }
    }
    Ok(Ok(changes))
}

/// After `changes`: the paired records' worked-out values, the records
/// whose rollups read them, and the event that names them.
pub async fn after_pairs(
    connection: &mut PgConnection,
    project_id: Uuid,
    changes: Vec<PairChange>,
) -> Result<(), AppError> {
    for PairChange {
        collection_id,
        mut records,
    } in changes
    {
        records.sort_unstable();
        records.dedup();
        super::computed::refresh_records(connection, project_id, collection_id, &records).await?;
        super::computed::mark_dependents(connection, project_id, collection_id, &records, 1)
            .await?;
        crate::outbox::enqueue_records_changed(
            connection,
            project_id,
            collection_id,
            &records,
            "paired",
        )
        .await?;
    }
    Ok(())
}

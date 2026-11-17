//! Formula results kept on records (T1.17).
//!
//! A record's formula results live in `collection_records.computed_values`,
//! keyed by the formula field's ID, so views filter, sort and group by them
//! (`fields::Accessor::Computed`). They're worked out in Rust
//! (`fields::formula::compute`) and written:
//!
//! - with the record, in the same transaction, whenever the record is
//!   written ([`refresh`], from `records::write`);
//! - for every record of the collection when a formula is made or changed,
//!   or when a field a formula reads changes how its values read (an option
//!   renamed), by the [`RECOMPUTE_KIND`] job ([`enqueue`]).
//!
//! The job works in batches of `RECOMPUTE_BATCH`, each in its own
//! transaction that holds the collection row `FOR SHARE` and reads the
//! fields afresh: record writes and field changes lock that row for update,
//! so a batch never races a write. A change while a run is under way queues
//! another run behind it.
//!
//! Recalculation across records (rollups) and nightly recalculation of
//! formulas that read the time are T1.18's.

use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    db,
    error::AppError,
    fields::{
        FieldDefinition,
        formula::{self, RecordData, limits::RECOMPUTE_BATCH},
    },
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    work_item_fields,
};

/// Recalculates every record's formula results in one collection.
pub const RECOMPUTE_KIND: &str = "collections.recompute_formulas";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecomputeJob {
    collection_id: Uuid,
}

#[derive(Debug, FromRow)]
struct Row {
    id: Uuid,
    number: i64,
    title: String,
    field_values: Value,
    computed_values: Value,
    created_by_actor_id: Uuid,
    updated_by_actor_id: Uuid,
    created_by_name: Option<String>,
    updated_by_name: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const ROW_SELECT: &str = "SELECT record.id, record.number, record.title, record.field_values, \
     record.computed_values, record.created_by_actor_id, record.updated_by_actor_id, \
     creator.display_name AS created_by_name, editor.display_name AS updated_by_name, \
     record.created_at, record.updated_at \
     FROM collection_records record \
     LEFT JOIN project_memberships creator \
       ON creator.project_id = record.project_id AND creator.actor_id = record.created_by_actor_id \
     LEFT JOIN project_memberships editor \
       ON editor.project_id = record.project_id AND editor.actor_id = record.updated_by_actor_id";

/// What the people, tasks, pages and records `rows`' values name are
/// called, and who made and last changed each.
async fn labels_for(
    connection: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    rows: &[Row],
) -> Result<HashMap<Uuid, String>, AppError> {
    let mut labels = work_item_fields::references(
        connection,
        project_id,
        definitions,
        rows.iter().map(|row| &row.field_values),
    )
    .await?
    .into_iter()
    .map(|(id, reference)| (id, reference.label))
    .collect::<HashMap<_, _>>();
    for row in rows {
        for (id, name) in [
            (row.created_by_actor_id, &row.created_by_name),
            (row.updated_by_actor_id, &row.updated_by_name),
        ] {
            if let Some(name) = name {
                labels.entry(id).or_insert_with(|| name.clone());
            }
        }
    }
    Ok(labels)
}

fn data<'a>(row: &'a Row, labels: &'a HashMap<Uuid, String>) -> RecordData<'a> {
    RecordData {
        title: &row.title,
        number: row.number,
        values: &row.field_values,
        created_at: row.created_at,
        updated_at: row.updated_at,
        created_by: row.created_by_actor_id,
        updated_by: row.updated_by_actor_id,
        labels,
    }
}

/// Works out `rows`' results and writes the ones that changed.
async fn compute_rows(
    connection: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    rows: &[Row],
) -> Result<HashMap<Uuid, Value>, AppError> {
    let labels = labels_for(connection, project_id, definitions, rows).await?;
    let now = Utc::now();
    let mut results = HashMap::with_capacity(rows.len());
    let mut ids = Vec::new();
    let mut values = Vec::new();
    for row in rows {
        let computed = formula::compute(definitions, &data(row, &labels), now);
        if computed != row.computed_values {
            ids.push(row.id);
            values.push(computed.clone());
        }
        results.insert(row.id, computed);
    }
    if !ids.is_empty() {
        // Results only: the record's version and edited time stay.
        sqlx::query(
            "UPDATE collection_records record SET computed_values = changed.computed_values \
             FROM unnest($2::uuid[], $3::jsonb[]) AS changed(id, computed_values) \
             WHERE record.project_id = $1 AND record.id = changed.id",
        )
        .bind(project_id)
        .bind(&ids)
        .bind(&values)
        .execute(&mut *connection)
        .await?;
    }
    Ok(results)
}

/// Works out one record's formula results after a write, in the write's
/// transaction, and gives them; None when the collection has no formulas.
pub async fn refresh(
    connection: &mut PgConnection,
    project_id: Uuid,
    record_id: Uuid,
    definitions: &[FieldDefinition],
) -> Result<Option<Value>, AppError> {
    if !formula::has_formulas(definitions) {
        return Ok(None);
    }
    let rows = sqlx::query_as::<_, Row>(&format!(
        "{ROW_SELECT} WHERE record.project_id = $1 AND record.id = $2"
    ))
    .bind(project_id)
    .bind(record_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut results = compute_rows(connection, project_id, definitions, &rows).await?;
    Ok(results.remove(&record_id))
}

/// Queues the recalculation of a collection's formulas, as part of the
/// change that needs it (which holds the collection row). One that hasn't
/// started yet covers this change too; one already running may have read
/// the fields before it, so another is queued behind it.
pub async fn enqueue(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<(), AppError> {
    let waiting = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM background_jobs \
         WHERE kind = $1 AND project_id = $2 AND state = 'queued' \
           AND payload->>'collectionId' = $3::text)",
    )
    .bind(RECOMPUTE_KIND)
    .bind(project_id)
    .bind(collection_id)
    .fetch_one(&mut *connection)
    .await?;
    if waiting {
        return Ok(());
    }
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?;
    jobs::enqueue(
        &mut *connection,
        NewJob::new(RECOMPUTE_KIND)
            .project(organization_id, project_id)
            .payload(RecomputeJob { collection_id }),
    )
    .await?;
    Ok(())
}

/// One batch of records after `after`, in ID order: the records it
/// recalculated. None when the collection is gone or has no formulas left.
async fn recompute_batch(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    after: Option<Uuid>,
) -> Result<Option<Vec<Uuid>>, AppError> {
    let found = sqlx::query_scalar::<_, bool>(
        "SELECT acts_like_task FROM collections \
         WHERE project_id = $1 AND id = $2 AND builtin IS NULL FOR SHARE",
    )
    .bind(project_id)
    .bind(collection_id)
    .fetch_optional(&mut *connection)
    .await?;
    if found != Some(false) {
        return Ok(None);
    }
    let definitions = super::field_definitions(connection, project_id, collection_id, true).await?;
    if !formula::has_formulas(&definitions) {
        return Ok(None);
    }
    let rows = sqlx::query_as::<_, Row>(&format!(
        "{ROW_SELECT} WHERE record.project_id = $1 AND record.collection_id = $2 \
           AND ($3::uuid IS NULL OR record.id > $3) \
         ORDER BY record.id LIMIT $4 FOR UPDATE OF record"
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(after)
    .bind(RECOMPUTE_BATCH)
    .fetch_all(&mut *connection)
    .await?;
    compute_rows(connection, project_id, &definitions, &rows).await?;
    Ok(Some(rows.iter().map(|row| row.id).collect()))
}

/// Recalculates every record of one collection, batch by batch.
pub async fn recompute_collection(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    collection_id: Uuid,
) -> Result<(), AppError> {
    let mut after = None;
    loop {
        let mut transaction = db::scoped_tx(pool).await?;
        let Some(ids) = recompute_batch(&mut transaction, project_id, collection_id, after).await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        // Open tables and dashboards fetch the new values (a field's own
        // event can reach them before the batch is done).
        if !ids.is_empty() {
            crate::outbox::enqueue_collection_updated(
                &mut transaction,
                project_id,
                collection_id,
                &ids,
            )
            .await?;
        }
        transaction.commit().await?;
        if (ids.len() as i64) < RECOMPUTE_BATCH {
            return Ok(());
        }
        after = ids.last().copied();
    }
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        RECOMPUTE_KIND,
        Duration::from_secs(30 * 60),
        |context: JobContext| async move {
            let payload: RecomputeJob = context.job.payload_as()?;
            let Some(project_id) = context.job.project_id else {
                return Err(JobError::permanent("the job names no project"));
            };
            recompute_collection(&context.state.pool, project_id, payload.collection_id).await?;
            Ok(())
        },
    );
}

/// One record's value for a formula that isn't saved yet, for the formula
/// editor's preview: the record numbered `number`, or the collection's
/// first active one. None when the collection has no records.
pub async fn preview(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    definitions: &[FieldDefinition],
    formula_field: FieldDefinition,
    number: Option<i64>,
) -> Result<Option<(i64, String, Value)>, AppError> {
    let Some(row) = sqlx::query_as::<_, Row>(&format!(
        "{ROW_SELECT} WHERE record.project_id = $1 AND record.collection_id = $2 \
           AND ($3::bigint IS NULL AND record.archived_at IS NULL OR record.number = $3) \
         ORDER BY record.number LIMIT 1"
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(number)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    let field_id = formula_field.id;
    let mut fields = definitions
        .iter()
        .filter(|field| field.id != field_id)
        .cloned()
        .collect::<Vec<_>>();
    fields.push(formula_field);
    // Worked out as a save would; nothing is written.
    let labels = labels_for(connection, project_id, &fields, std::slice::from_ref(&row)).await?;
    let value = formula::compute(&fields, &data(&row, &labels), Utc::now())
        .get(field_id.to_string())
        .cloned()
        .unwrap_or(Value::Null);
    Ok(Some((row.number, row.title, value)))
}

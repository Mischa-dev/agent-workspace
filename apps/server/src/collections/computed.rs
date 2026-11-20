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
//! Rollups (T1.18) are worked out with formulas, from the records their
//! relations name. When a record changes, the records whose rollups read it
//! are queued in `record_recalculations` ([`mark_dependents`]) and the
//! [`RECALCULATE_KIND`] job works them out, coalescing a burst of changes
//! into one recalculation per record; whatever that changes is queued in
//! turn, up to the relation depth fields may reach. Formulas that read the
//! time are worked out again every night ([`VOLATILE_KIND`]).
//!
//! A collection's whole recalculation is resumable: each batch commits with
//! the job for the next one, so a stopped run carries on where it was.

use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    db,
    error::AppError,
    fields::{
        self, FieldDefinition, FieldType,
        formula::{self, RecordData, limits::RECOMPUTE_BATCH},
    },
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    work_item_fields,
};

/// Recalculates every record's formula results in one collection.
pub const RECOMPUTE_KIND: &str = "collections.recompute_formulas";
/// Works out the records waiting in `record_recalculations` for one space.
pub const RECALCULATE_KIND: &str = "fields.recalculate";
/// Queues every collection with a formula that reads the time.
pub const VOLATILE_KIND: &str = "fields.recompute_volatile";

/// Records one recalculation batch takes from the queue.
const RECALCULATE_BATCH: i64 = 200;
/// How many records away a change may still be worked out (a chain of
/// rollups can't be longer: `fields::dependencies`).
const MAX_DEPTH: i16 = (crate::fields::dependencies::MAX_RELATION_DEPTH + 1) as i16;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecomputeJob {
    collection_id: Uuid,
    /// Where the run had got to: it carries on after this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<Uuid>,
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

fn data<'a>(
    row: &'a Row,
    labels: &'a HashMap<Uuid, String>,
    computed: &'a Value,
) -> RecordData<'a> {
    RecordData {
        title: &row.title,
        number: row.number,
        values: &row.field_values,
        created_at: row.created_at,
        updated_at: row.updated_at,
        created_by: row.created_by_actor_id,
        updated_by: row.updated_by_actor_id,
        labels,
        computed,
    }
}

/// The records a relation names, as rollups read them.
struct Related {
    fields: Vec<FieldDefinition>,
    rows: HashMap<Uuid, Row>,
    labels: HashMap<Uuid, String>,
}

/// Each of `rows`' rollups, worked out from the active records their
/// relations name: per row, its rollup results by field ID.
async fn rollups_for(
    connection: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    rows: &[Row],
) -> Result<Vec<Map<String, Value>>, AppError> {
    let mut out = vec![Map::new(); rows.len()];
    let rollups = definitions
        .iter()
        .filter(|field| field.field_type == FieldType::Rollup && !field.archived)
        .filter_map(|field| Some((field, field.config.rollup.as_ref()?)))
        .collect::<Vec<_>>();
    if rollups.is_empty() {
        return Ok(out);
    }
    // One read per related collection, whichever rollups use it.
    let mut related: HashMap<Uuid, Related> = HashMap::new();
    for (_, rollup) in &rollups {
        let Some(relation) = definitions.iter().find(|field| field.id == rollup.relation) else {
            continue;
        };
        let Some(collection) = relation.config.collection else {
            continue;
        };
        if related.contains_key(&collection) {
            continue;
        }
        let relations = definitions
            .iter()
            .filter(|field| {
                field.field_type == FieldType::Relation
                    && field.config.collection == Some(collection)
            })
            .collect::<Vec<_>>();
        let ids = rows
            .iter()
            .flat_map(|row| {
                relations.iter().flat_map(|field| {
                    row.field_values
                        .get(field.id.to_string())
                        .map(|value| fields::referenced_ids(field, value))
                        .unwrap_or_default()
                })
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let fields = super::field_definitions(connection, project_id, collection, true).await?;
        let found = sqlx::query_as::<_, Row>(&format!(
            "{ROW_SELECT} WHERE record.project_id = $1 AND record.collection_id = $2 \
               AND record.id = ANY($3) AND record.archived_at IS NULL"
        ))
        .bind(project_id)
        .bind(collection)
        .bind(&ids)
        .fetch_all(&mut *connection)
        .await?;
        let labels = labels_for(connection, project_id, &fields, &found).await?;
        related.insert(
            collection,
            Related {
                fields,
                rows: found.into_iter().map(|row| (row.id, row)).collect(),
                labels,
            },
        );
    }
    for (index, row) in rows.iter().enumerate() {
        for (field, rollup) in &rollups {
            let Some(relation) = definitions.iter().find(|field| field.id == rollup.relation)
            else {
                out[index].insert(
                    field.id.to_string(),
                    serde_json::json!({ "error": "The relation it reads is gone." }),
                );
                continue;
            };
            let Some(source) = relation
                .config
                .collection
                .and_then(|collection| related.get(&collection))
            else {
                continue;
            };
            let records = row
                .field_values
                .get(relation.id.to_string())
                .map(|value| fields::referenced_ids(relation, value))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|id| source.rows.get(&id))
                .map(|related| data(related, &source.labels, &related.computed_values))
                .collect::<Vec<_>>();
            if let Some(value) = formula::rollup(rollup, &source.fields, &records) {
                out[index].insert(field.id.to_string(), value);
            }
        }
    }
    Ok(out)
}

/// Works out `rows`' results and writes the ones that changed: every
/// row's results, and the IDs of the rows that changed.
async fn compute_rows(
    connection: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    rows: &[Row],
) -> Result<(HashMap<Uuid, Value>, Vec<Uuid>), AppError> {
    let labels = labels_for(connection, project_id, definitions, rows).await?;
    let rollups = rollups_for(connection, project_id, definitions, rows).await?;
    let now = Utc::now();
    let mut results = HashMap::with_capacity(rows.len());
    let mut ids = Vec::new();
    let mut values = Vec::new();
    for (row, rollups) in rows.iter().zip(rollups) {
        let rollups = Value::Object(rollups);
        let computed = formula::compute(definitions, &data(row, &labels, &rollups), now);
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
    Ok((results, ids))
}

/// Works out one record's formula results after a write, in the write's
/// transaction, and gives them; None when the collection has no formulas.
pub async fn refresh(
    connection: &mut PgConnection,
    project_id: Uuid,
    record_id: Uuid,
    definitions: &[FieldDefinition],
) -> Result<Option<Value>, AppError> {
    if !formula::has_computed(definitions) {
        return Ok(None);
    }
    let rows = sqlx::query_as::<_, Row>(&format!(
        "{ROW_SELECT} WHERE record.project_id = $1 AND record.id = $2"
    ))
    .bind(project_id)
    .bind(record_id)
    .fetch_all(&mut *connection)
    .await?;
    let (mut results, _) = compute_rows(connection, project_id, definitions, &rows).await?;
    Ok(results.remove(&record_id))
}

/// Works out `records` of `collection` again, in the caller's transaction:
/// the ones whose results changed.
pub async fn refresh_records(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    records: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let definitions = super::field_definitions(connection, project_id, collection_id, true).await?;
    if !formula::has_computed(&definitions) {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, Row>(&format!(
        "{ROW_SELECT} WHERE record.project_id = $1 AND record.collection_id = $2 \
           AND record.id = ANY($3) ORDER BY record.id FOR UPDATE OF record"
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(records)
    .fetch_all(&mut *connection)
    .await?;
    let (_, changed) = compute_rows(connection, project_id, &definitions, &rows).await?;
    Ok(changed)
}

/// Queues, after `records` of `collection` changed, the records whose
/// rollups read them (`depth` records away from the first change), and the
/// job that works them out.
pub async fn mark_dependents(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    records: &[Uuid],
    depth: i16,
) -> Result<(), AppError> {
    if records.is_empty() || depth > MAX_DEPTH {
        return Ok(());
    }
    // The relations into this collection that some active rollup reads.
    let relations = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT DISTINCT relation.collection_id, relation.id \
         FROM field_definitions rollup \
         JOIN field_definitions relation ON relation.project_id = rollup.project_id \
           AND relation.id::text = rollup.config -> 'rollup' ->> 'relation' \
         WHERE rollup.project_id = $1 AND rollup.field_type = 'rollup' \
           AND rollup.archived_at IS NULL AND relation.field_type = 'relation' \
           AND relation.config ->> 'collection' = $2::text",
    )
    .bind(project_id)
    .bind(collection_id)
    .fetch_all(&mut *connection)
    .await?;
    if relations.is_empty() {
        return Ok(());
    }
    let ids = records.iter().map(Uuid::to_string).collect::<Vec<_>>();
    let mut queued = 0;
    for (collection, relation) in relations {
        queued += sqlx::query(
            r#"
            INSERT INTO record_recalculations (record_id, workspace_id, space_id, collection_id, depth)
            SELECT record.id, space.organization_id, record.project_id, record.collection_id, $5
            FROM collection_records record
            JOIN projects space ON space.id = record.project_id
            WHERE record.project_id = $1 AND record.collection_id = $2
              AND jsonb_typeof(record.field_values -> $3) = 'array'
              AND EXISTS (
                  SELECT 1 FROM jsonb_array_elements_text(record.field_values -> $3) named(id)
                  WHERE named.id = ANY($4)
              )
            ON CONFLICT (record_id)
                DO UPDATE SET depth = LEAST(record_recalculations.depth, EXCLUDED.depth)
            "#,
        )
        .bind(project_id)
        .bind(collection)
        .bind(relation.to_string())
        .bind(&ids)
        .bind(depth)
        .execute(&mut *connection)
        .await?
        .rows_affected();
    }
    if queued == 0 {
        return Ok(());
    }
    queue_recalculation(connection, project_id).await
}

/// Makes sure a job will work out the space's queued records. A job that
/// hasn't started takes them too; one already running may have taken its
/// last batch, so another is queued behind it.
async fn queue_recalculation(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<(), AppError> {
    let waiting = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM background_jobs \
         WHERE kind = $1 AND project_id = $2 AND state = 'queued')",
    )
    .bind(RECALCULATE_KIND)
    .bind(project_id)
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
        NewJob::new(RECALCULATE_KIND)
            .project(organization_id, project_id)
            .payload(serde_json::json!({})),
    )
    .await?;
    Ok(())
}

/// Works out one batch of the space's queued records: how many it took.
async fn recalculate_batch(pool: &sqlx::PgPool, project_id: Uuid) -> Result<usize, AppError> {
    let mut transaction = db::scoped_tx(pool).await?;
    let taken = sqlx::query_as::<_, (Uuid, Uuid, i16)>(
        "DELETE FROM record_recalculations WHERE record_id IN ( \
             SELECT record_id FROM record_recalculations WHERE space_id = $1 \
             ORDER BY queued_at, record_id LIMIT $2 FOR UPDATE SKIP LOCKED) \
         RETURNING record_id, collection_id, depth",
    )
    .bind(project_id)
    .bind(RECALCULATE_BATCH)
    .fetch_all(&mut *transaction)
    .await?;
    let mut by_collection: HashMap<Uuid, (Vec<Uuid>, i16)> = HashMap::new();
    for (record, collection, depth) in &taken {
        let entry = by_collection
            .entry(*collection)
            .or_insert((Vec::new(), *depth));
        entry.0.push(*record);
        entry.1 = entry.1.min(*depth);
    }
    let mut collections = by_collection.into_iter().collect::<Vec<_>>();
    collections.sort_by_key(|(collection, _)| *collection);
    for (collection, (records, depth)) in collections {
        // Field changes lock the collection for update; this waits for them.
        let found = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM collections WHERE project_id = $1 AND id = $2 FOR SHARE",
        )
        .bind(project_id)
        .bind(collection)
        .fetch_optional(&mut *transaction)
        .await?;
        if found.is_none() {
            continue;
        }
        let changed = refresh_records(&mut transaction, project_id, collection, &records).await?;
        if changed.is_empty() {
            continue;
        }
        crate::outbox::enqueue_records_changed(
            &mut transaction,
            project_id,
            collection,
            &changed,
            "computed",
        )
        .await?;
        mark_dependents(
            &mut transaction,
            project_id,
            collection,
            &changed,
            depth + 1,
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(taken.len())
}

/// Works out every record queued for the space, batch by batch.
pub async fn recalculate_space(pool: &sqlx::PgPool, project_id: Uuid) -> Result<(), AppError> {
    // Bounded, so a loop of changes (only possible if fields changed under
    // the queue) can't keep a worker forever; what's left gets a new job.
    for _ in 0..50 {
        if (recalculate_batch(pool, project_id).await? as i64) < RECALCULATE_BATCH {
            return Ok(());
        }
    }
    let mut transaction = db::scoped_tx(pool).await?;
    queue_recalculation(&mut transaction, project_id).await?;
    transaction.commit().await?;
    Ok(())
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
           AND payload->>'collectionId' = $3::text AND NOT payload ? 'after')",
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
            .payload(RecomputeJob {
                collection_id,
                after: None,
            }),
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
         WHERE project_id = $1 AND id = $2 AND builtin IS DISTINCT FROM 'tasks' FOR SHARE",
    )
    .bind(project_id)
    .bind(collection_id)
    .fetch_optional(&mut *connection)
    .await?;
    if found != Some(false) {
        return Ok(None);
    }
    let definitions = super::field_definitions(connection, project_id, collection_id, true).await?;
    if !formula::has_computed(&definitions) {
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

/// Recalculates one batch of a collection's records after `after`, and
/// queues the job for the next batch in the same transaction, so a run
/// stopped at any point carries on from the last batch it finished.
pub async fn recompute_collection(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    collection_id: Uuid,
    after: Option<Uuid>,
) -> Result<(), AppError> {
    let mut transaction = db::scoped_tx(pool).await?;
    let Some(ids) = recompute_batch(&mut transaction, project_id, collection_id, after).await?
    else {
        transaction.commit().await?;
        return Ok(());
    };
    // Open tables and dashboards fetch the new values (a field's own event
    // can reach them before the run is done).
    if !ids.is_empty() {
        crate::outbox::enqueue_records_changed(
            &mut transaction,
            project_id,
            collection_id,
            &ids,
            "computed",
        )
        .await?;
        mark_dependents(&mut transaction, project_id, collection_id, &ids, 1).await?;
    }
    if (ids.len() as i64) == RECOMPUTE_BATCH {
        let organization_id =
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_one(&mut *transaction)
                .await?;
        jobs::enqueue(
            &mut *transaction,
            NewJob::new(RECOMPUTE_KIND)
                .project(organization_id, project_id)
                .payload(RecomputeJob {
                    collection_id,
                    after: ids.last().copied(),
                }),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Queues the recalculation of every collection with a formula that reads
/// the time, so TODAY() and NOW() move on by themselves.
pub async fn queue_volatile(pool: &sqlx::PgPool) -> Result<(), AppError> {
    let mut transaction = db::scoped_tx(pool).await?;
    let collections = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT DISTINCT field.project_id, field.collection_id FROM field_definitions field \
         JOIN collections collection ON collection.id = field.collection_id \
         WHERE field.field_type = 'formula' AND field.archived_at IS NULL \
           AND (field.config -> 'formula' ->> 'volatile')::boolean IS TRUE \
           AND collection.archived_at IS NULL AND collection.builtin IS DISTINCT FROM 'tasks' \
           AND NOT collection.acts_like_task",
    )
    .fetch_all(&mut *transaction)
    .await?;
    for (project_id, collection_id) in collections {
        enqueue(&mut transaction, project_id, collection_id).await?;
    }
    transaction.commit().await?;
    Ok(())
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
            recompute_collection(
                &context.state.pool,
                project_id,
                payload.collection_id,
                payload.after,
            )
            .await?;
            Ok(())
        },
    );
    registry.register_with_timeout(
        RECALCULATE_KIND,
        Duration::from_secs(10 * 60),
        |context: JobContext| async move {
            let Some(project_id) = context.job.project_id else {
                return Err(JobError::permanent("the job names no project"));
            };
            recalculate_space(&context.state.pool, project_id).await?;
            Ok(())
        },
    );
    registry.register(VOLATILE_KIND, |context: JobContext| async move {
        queue_volatile(&context.state.pool).await?;
        Ok(())
    });
    registry.every(VOLATILE_KIND, Duration::from_secs(24 * 60 * 60));
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
    let value = formula::compute(
        &fields,
        &data(&row, &labels, &row.computed_values),
        Utc::now(),
    )
    .get(field_id.to_string())
    .cloned()
    .unwrap_or(Value::Null);
    Ok(Some((row.number, row.title, value)))
}

//! A task's record owns what a record and a task share: its title, its field
//! values and its archive state (T1.3, owner step 3a of moving tasks into
//! collection records). The task commands write them here, and a database
//! trigger copies them back onto `work_items`, which keeps them as a read
//! cache until every reader has moved to `task_rows` (T1.4). `work_items`
//! refuses a direct write of those columns.
//!
//! The record's version is its own: a write here bumps the record's, and the
//! command that calls it bumps the task's, so `expectedVersion` on the task
//! tools keeps meaning the task's version.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// A new task's record, written before the task itself (inserting the task
/// fills its cached columns from the record).
pub struct NewTaskRecord<'a> {
    pub project_id: Uuid,
    pub task_id: Uuid,
    /// The collection the task is in: `None` for the project's Tasks
    /// collection (made here if it doesn't exist yet).
    pub collection_id: Option<Uuid>,
    /// The task's number, which its record takes too.
    pub number: i64,
    pub title: &'a str,
    pub field_values: &'a serde_json::Value,
    pub actor_id: Uuid,
}

/// What changes on an existing task's record; `None` leaves it as it is.
#[derive(Default)]
pub struct SharedChange<'a> {
    pub title: Option<&'a str>,
    pub field_values: Option<&'a serde_json::Value>,
    /// Archive (`true`, as the writer) or restore (`false`).
    pub archived: Option<bool>,
}

impl SharedChange<'_> {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.field_values.is_none() && self.archived.is_none()
    }
}

/// Writes a new task's record.
pub async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    record: NewTaskRecord<'_>,
) -> Result<(), AppError> {
    let title = crate::work_item_commands::normalize_title(record.title)?;
    sqlx::query(
        r#"
        INSERT INTO collection_records (
            id, project_id, collection_id, number, title, field_values,
            created_by_actor_id, updated_by_actor_id
        ) VALUES (
            $1, $2, COALESCE($3, ensure_tasks_collection($2, $7)), $4, $5, $6, $7, $7
        )
        "#,
    )
    .bind(record.task_id)
    .bind(record.project_id)
    .bind(record.collection_id)
    .bind(record.number)
    .bind(title)
    .bind(record.field_values)
    .bind(record.actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Writes the shared columns of a task's record, bumping the record's
/// version once. Returns false when the record isn't there, or `archived`
/// asks for the state it's already in (nothing is written then). The
/// caller holds the task's lock and bumps the task's own version.
pub async fn write_shared(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    change: SharedChange<'_>,
) -> Result<bool, AppError> {
    if change.is_empty() {
        return Ok(true);
    }
    let title = change
        .title
        .map(crate::work_item_commands::normalize_title)
        .transpose()?;
    let written = sqlx::query(
        r#"
        UPDATE collection_records
        SET title = COALESCE($4, title),
            field_values = COALESCE($5, field_values),
            archived_at = CASE WHEN $6::boolean IS NULL THEN archived_at
                               WHEN $6 THEN now() END,
            archived_by_actor_id = CASE WHEN $6::boolean IS NULL THEN archived_by_actor_id
                                        WHEN $6 THEN $3::uuid END,
            updated_by_actor_id = $3, updated_at = now(), version = version + 1
        WHERE project_id = $1 AND id = $2
          AND ($6::boolean IS NULL OR (archived_at IS NOT NULL) <> $6)
        "#,
    )
    .bind(project_id)
    .bind(task_id)
    .bind(actor_id)
    .bind(title)
    .bind(change.field_values)
    .bind(change.archived)
    .execute(&mut **transaction)
    .await?;
    Ok(written.rows_affected() == 1)
}

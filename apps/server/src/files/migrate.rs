//! Moving files between stores (K5.13): `files-move --to object` queues a
//! job that walks files by id, 100 at a time, copies each one's bytes (and
//! preview) to the new store, checks the length and SHA-256, and switches
//! the row's `store_id`. Each batch is one job that queues the next, so a
//! stopped server picks up where it left off. The old copy stays (the row
//! remembers where, `moved_from_store_id`) until `files-prune-source`
//! deletes it, so going back is possible until then. Reads follow
//! `store_id`, so moved and unmoved files are served side by side.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::FileStore;
use crate::{
    AppState,
    config::AppConfig,
    error::AppError,
    jobs::{self, JobError, JobRegistry, NewJob},
};

pub const MOVE_KIND: &str = "files.move";
/// Files copied per job.
pub const BATCH: i64 = 100;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MovePayload {
    pub to: String,
    /// Only this workspace's files, when set.
    #[serde(default)]
    pub workspace: Option<Uuid>,
    /// The last file id the previous batch looked at.
    #[serde(default)]
    pub after: Option<Uuid>,
}

/// What one batch did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Moved {
    pub moved: u64,
    pub failed: u64,
    /// Where the next batch starts; None when there is nothing after.
    pub next: Option<Uuid>,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(MOVE_KIND, |context| async move {
        let payload = context.job.payload_as::<MovePayload>()?;
        let moved = move_batch(&context.state, &payload)
            .await
            .map_err(JobError::from)?;
        if let Some(next) = moved.next {
            queue(
                &context.state.pool,
                MovePayload {
                    after: Some(next),
                    ..payload
                },
            )
            .await
            .map_err(JobError::from)?;
        }
        Ok(())
    });
}

/// Starts moving every file (or one workspace's) to the store `to`.
pub async fn start(
    pool: &PgPool,
    config: &AppConfig,
    to: &str,
    workspace: Option<Uuid>,
) -> Result<jobs::Enqueued, AppError> {
    let target = FileStore::resolve_with(pool, config, to).await?;
    // A workspace's own bucket takes only that workspace's files.
    if to.starts_with(super::store::WORKSPACE_PREFIX)
        && workspace != super::workspace_store::workspace_of(pool, to).await?
    {
        return Err(AppError::Validation("workspace"));
    }
    drop(target);
    queue(
        pool,
        MovePayload {
            to: to.to_owned(),
            workspace,
            after: None,
        },
    )
    .await
}

/// Queues moving one workspace's files into its own bucket, in the
/// caller's transaction (the bucket was just recorded in it).
pub(crate) async fn start_in(
    connection: &mut sqlx::PgConnection,
    to: &str,
    workspace: Uuid,
) -> Result<jobs::Enqueued, AppError> {
    jobs::enqueue(
        &mut *connection,
        job(MovePayload {
            to: to.to_owned(),
            workspace: Some(workspace),
            after: None,
        }),
    )
    .await
}

async fn queue(pool: &PgPool, payload: MovePayload) -> Result<jobs::Enqueued, AppError> {
    jobs::enqueue(crate::db::scoped(pool), job(payload)).await
}

fn job(payload: MovePayload) -> NewJob {
    let mut job = NewJob::new(MOVE_KIND).payload(&payload).unique(format!(
        "files.move:{}:{}",
        payload.to,
        payload
            .workspace
            .map(|id| id.to_string())
            .unwrap_or_default()
    ));
    if let Some(workspace) = payload.workspace {
        job = job.workspace(workspace);
    }
    job
}

#[derive(sqlx::FromRow)]
struct ToMove {
    id: Uuid,
    store_id: String,
    storage_key: String,
    thumbnail_key: Option<String>,
    byte_size: i64,
    sha256: String,
}

/// Copies the next batch of files that aren't in `to` yet.
pub async fn move_batch(state: &AppState, payload: &MovePayload) -> Result<Moved, AppError> {
    let target = FileStore::resolve(state, &payload.to).await?;
    if payload.to.starts_with(super::store::WORKSPACE_PREFIX)
        && payload.workspace
            != super::workspace_store::workspace_of(&state.pool, &payload.to).await?
    {
        return Err(AppError::Validation("workspace"));
    }
    let batch = sqlx::query_as::<_, ToMove>(
        r#"
        SELECT id, store_id, storage_key, thumbnail_key, byte_size, sha256
        FROM files
        WHERE ($1::uuid IS NULL OR id > $1)
          AND ($2::uuid IS NULL OR organization_id = $2)
          AND blob_deleted_at IS NULL
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(payload.after)
    .bind(payload.workspace)
    .bind(BATCH)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let mut moved = Moved {
        next: (batch.len() as i64 == BATCH)
            .then(|| batch.last().map(|file| file.id))
            .flatten(),
        ..Moved::default()
    };
    for file in batch.iter().filter(|file| file.store_id != payload.to) {
        match copy(state, &target, file).await {
            Ok(()) => moved.moved += 1,
            Err(error) => {
                tracing::warn!(file_id = %file.id, category = error.log_category(), "file not moved");
                moved.failed += 1;
            }
        }
    }
    Ok(moved)
}

async fn copy(state: &AppState, target: &FileStore, file: &ToMove) -> Result<(), AppError> {
    let source = FileStore::resolve(state, &file.store_id).await?;
    let length = u64::try_from(file.byte_size).unwrap_or(0);
    let stored = source.get(&file.storage_key).await?;
    // A key that was copied before a crash is written again from the start.
    target.delete(&file.storage_key).await?;
    let written = target
        .put_stream(&file.storage_key, stored.body.into_data_stream(), length)
        .await?;
    if written.length != length || written.sha256 != file.sha256 {
        let _ = target.delete(&file.storage_key).await;
        return Err(AppError::Internal("a moved file's bytes didn't match"));
    }
    if let Some(thumbnail) = &file.thumbnail_key {
        let bytes = source.read_all(thumbnail, 64 * 1024 * 1024).await?;
        target.delete(thumbnail).await?;
        target.put(thumbnail, &bytes).await?;
        let copied = target.read_all(thumbnail, 64 * 1024 * 1024).await?;
        if Sha256::digest(&copied) != Sha256::digest(&bytes) {
            return Err(AppError::Internal("a moved preview's bytes didn't match"));
        }
    }
    let switched = sqlx::query(
        r#"
        UPDATE files SET store_id = $2, moved_from_store_id = store_id
        WHERE id = $1 AND store_id = $3
        "#,
    )
    .bind(file.id)
    .bind(target.id())
    .bind(&file.store_id)
    .execute(crate::db::scoped(&state.pool))
    .await?;
    if switched.rows_affected() == 0 {
        // Removed or moved meanwhile: the copy isn't anyone's.
        let _ = target.delete(&file.storage_key).await;
    }
    Ok(())
}

/// Deletes the old copies of moved files, so they can't be moved back
/// without copying again. Returns how many files' old copies went.
pub async fn prune_sources(pool: &PgPool, config: &AppConfig) -> Result<u64, AppError> {
    let mut pruned = 0;
    loop {
        let batch = sqlx::query_as::<_, (Uuid, String, String, Option<String>)>(
            r#"
            SELECT id, moved_from_store_id, storage_key, thumbnail_key
            FROM files
            WHERE moved_from_store_id IS NOT NULL
            ORDER BY id
            LIMIT $1
            "#,
        )
        .bind(BATCH)
        .fetch_all(crate::db::scoped(pool))
        .await?;
        if batch.is_empty() {
            return Ok(pruned);
        }
        for (id, from, key, thumbnail) in batch {
            let source = FileStore::resolve_with(pool, config, &from).await?;
            for key in std::iter::once(key).chain(thumbnail) {
                source.delete(&key).await?;
            }
            sqlx::query("UPDATE files SET moved_from_store_id = NULL WHERE id = $1")
                .bind(id)
                .execute(crate::db::scoped(pool))
                .await?;
            pruned += 1;
        }
    }
}

//! Moving files between stores (K5.13): `files-move --to object` queues a
//! job that walks files by id, 100 at a time, copies each one's bytes (and
//! preview) to the new store, checks the length and SHA-256, and switches
//! the row's `store_id`. Each batch is one job that queues the next, so a
//! stopped server picks up where it left off. The old copy stays (the row
//! remembers where, `moved_from_store_id`) until `files-prune-source`
//! deletes it, so going back is possible until then. Reads follow
//! `store_id`, so moved and unmoved files are served side by side.
//!
//! Moves into one store can overlap (a server-wide move and a
//! workspace's) and write the same key, so a move or a prune first takes a
//! lease on the file's row (`move_lease`): only its holder writes or
//! deletes the file's bytes, and a destination object is deleted only
//! while held, or once no row names it.

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
    /// Held by another move or a prune, moved meanwhile, or removed.
    pub skipped: u64,
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
            let queued = queue(
                &context.state.pool,
                MovePayload {
                    after: Some(next),
                    ..payload
                },
            )
            .await
            .map_err(JobError::from)?;
            if matches!(queued, jobs::Enqueued::Duplicate) {
                tracing::debug!(%next, "the next file-move batch was already queued");
            }
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

/// One move at a time per store and workspace; each batch's key names
/// where it starts, so the next batch, queued while this one still runs,
/// doesn't collide with it.
fn unique_key(payload: &MovePayload) -> String {
    format!(
        "files.move:{}:{}:{}",
        payload.to,
        payload
            .workspace
            .map(|id| id.to_string())
            .unwrap_or_default(),
        payload
            .after
            .map_or_else(|| "start".to_owned(), |id| id.to_string())
    )
}

fn job(payload: MovePayload) -> NewJob {
    let mut job = NewJob::new(MOVE_KIND)
        .payload(&payload)
        .unique(unique_key(&payload));
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
    // Files of every workspace, or of one.
    crate::tenancy::system("files.move", Box::pin(move_batch_here(state, payload))).await
}

async fn move_batch_here(state: &AppState, payload: &MovePayload) -> Result<Moved, AppError> {
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
          AND NOT ($4 AND members_only)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(payload.after)
    .bind(payload.workspace)
    .bind(BATCH)
    // A workspace's own bucket never takes a DM's or group's files.
    .bind(payload.to.starts_with(super::store::WORKSPACE_PREFIX))
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
            Ok(true) => moved.moved += 1,
            Ok(false) => moved.skipped += 1,
            Err(error) => {
                tracing::warn!(file_id = %file.id, category = error.log_category(), "file not moved");
                moved.failed += 1;
            }
        }
    }
    Ok(moved)
}

/// How long a move or prune holds a file. A copy that takes longer loses
/// its file to the next taker and switches nothing.
const LEASE: &str = "1 hour";

/// Takes the file for one move or prune, while it is still in `store` and
/// nothing else holds it; for a move (`live`), only while its bytes are kept.
async fn claim(
    pool: &PgPool,
    file: Uuid,
    store: &str,
    live: bool,
) -> Result<Option<Uuid>, AppError> {
    let token = Uuid::now_v7();
    let claimed = sqlx::query(&format!(
        "UPDATE files SET move_lease = $2, move_lease_until = now() + interval '{LEASE}' \
         WHERE id = $1 AND store_id = $3 AND (NOT $4 OR blob_deleted_at IS NULL) \
           AND (move_lease IS NULL OR move_lease_until < now())"
    ))
    .bind(file)
    .bind(token)
    .bind(store)
    .bind(live)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok((claimed.rows_affected() == 1).then_some(token))
}

/// Lets the file go; true when this lease still held it.
async fn release(pool: &PgPool, file: Uuid, token: Uuid) -> Result<bool, AppError> {
    let released = sqlx::query(
        "UPDATE files SET move_lease = NULL, move_lease_until = NULL \
         WHERE id = $1 AND move_lease = $2",
    )
    .bind(file)
    .bind(token)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(released.rows_affected() == 1)
}

/// Copies one file into `target` and switches its row: true when moved,
/// false when another move or a prune holds it or it changed meanwhile.
async fn copy(state: &AppState, target: &FileStore, file: &ToMove) -> Result<bool, AppError> {
    let Some(token) = claim(&state.pool, file.id, &file.store_id, true).await? else {
        return Ok(false);
    };
    let keys = std::iter::once(file.storage_key.as_str())
        .chain(file.thumbnail_key.as_deref())
        .collect::<Vec<_>>();
    match write_copy(state, target, file).await {
        Ok(()) => {}
        Err(error) => {
            // Held, and the row still names the source: nothing names
            // what was written in the target.
            if release(&state.pool, file.id, token).await? {
                for key in &keys {
                    let _ = target.delete(key).await;
                }
            }
            return Err(error);
        }
    }
    let switched = sqlx::query(
        r#"
        UPDATE files SET store_id = $2, moved_from_store_id = store_id,
               move_lease = NULL, move_lease_until = NULL
        WHERE id = $1 AND store_id = $3 AND move_lease = $4 AND blob_deleted_at IS NULL
        "#,
    )
    .bind(file.id)
    .bind(target.id())
    .bind(&file.store_id)
    .bind(token)
    .execute(crate::db::scoped(&state.pool))
    .await?;
    if switched.rows_affected() == 1 {
        return Ok(true);
    }
    // The lease ran out and someone else may hold the file now, or it was
    // removed meanwhile. Its copy here is deleted only when the file is
    // gone and nothing holds it; otherwise it may be another move's.
    release(&state.pool, file.id, token).await?;
    let removed = sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS (SELECT 1 FROM files WHERE id = $1 \
             AND blob_deleted_at IS NULL OR id = $1 AND move_lease_until > now())",
    )
    .bind(file.id)
    .fetch_one(crate::db::scoped(&state.pool))
    .await?;
    if removed {
        for key in &keys {
            let _ = target.delete(key).await;
        }
    }
    Ok(false)
}

/// Writes the file's bytes (and preview) into `target` and checks them.
/// The caller holds the file.
async fn write_copy(state: &AppState, target: &FileStore, file: &ToMove) -> Result<(), AppError> {
    let source = FileStore::resolve(state, &file.store_id).await?;
    let length = u64::try_from(file.byte_size).unwrap_or(0);
    let stored = source.get(&file.storage_key).await?;
    // A key that was copied before a crash is written again from the start.
    target.delete(&file.storage_key).await?;
    let written = target
        .put_stream(&file.storage_key, stored.body.into_data_stream(), length)
        .await?;
    if written.length != length || written.sha256 != file.sha256 {
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
    Ok(())
}

/// Deletes the old copies of moved files, so they can't be moved back
/// without copying again. Returns how many files' old copies went.
pub async fn prune_sources(pool: &PgPool, config: &AppConfig) -> Result<u64, AppError> {
    // Server-wide: the old copies of every moved file.
    crate::tenancy::system("files.move", Box::pin(prune_sources_here(pool, config))).await
}

async fn prune_sources_here(pool: &PgPool, config: &AppConfig) -> Result<u64, AppError> {
    let mut pruned = 0;
    loop {
        let batch = sqlx::query_as::<_, (Uuid, String, String, String, Option<String>)>(
            r#"
            SELECT id, store_id, moved_from_store_id, storage_key, thumbnail_key
            FROM files
            WHERE moved_from_store_id IS NOT NULL
              AND (move_lease IS NULL OR move_lease_until < now())
              AND moved_from_store_id <> store_id
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
        let (mut held, mut done) = (0, 0);
        for (id, store, from, key, thumbnail) in batch {
            // A move holds it: it may be writing back into `from`.
            let Some(token) = claim(pool, id, &store, false).await? else {
                held += 1;
                continue;
            };
            let source = FileStore::resolve_with(pool, config, &from).await?;
            for key in std::iter::once(key).chain(thumbnail) {
                source.delete(&key).await?;
            }
            sqlx::query(
                "UPDATE files SET moved_from_store_id = NULL, move_lease = NULL, \
                 move_lease_until = NULL WHERE id = $1 AND move_lease = $2",
            )
            .bind(id)
            .bind(token)
            .execute(crate::db::scoped(pool))
            .await?;
            done += 1;
        }
        pruned += done;
        if held > 0 && done == 0 {
            // Only held files are left: a later prune takes them.
            return Ok(pruned);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_batch_never_shares_the_running_batchs_key() {
        let start = MovePayload {
            to: "object".to_owned(),
            workspace: None,
            after: None,
        };
        let next = MovePayload {
            after: Some(Uuid::now_v7()),
            ..start.clone()
        };
        assert_ne!(unique_key(&start), unique_key(&next));
        assert_eq!(unique_key(&start), unique_key(&start.clone()));
    }
}

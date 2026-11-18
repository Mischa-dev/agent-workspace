//! The cold log (K3.25, ruling R24 rule 7). Before retention lets a
//! workspace's old facts go, `events.archive` writes them to the
//! workspace's store (its own bucket when it has one) as one compressed,
//! position-ordered file of JSON lines per run, and records the file in
//! `event_archives` with its range and the things it names. Retention
//! deletes a fact only once a file holds it ([`super::upkeep`]).
//!
//! Readers that page back past the live log ([`super::activity`], the
//! audit trail) load the files they reach into a temporary table inside
//! their own transaction and read [`WITH_ARCHIVED`] instead of `events`:
//! the same query, slower, the same rows. Fetching a file is a network
//! call, so it happens between transactions, never inside one. Signals are
//! not history and are never archived.

use std::{
    io::{Read, Write},
    time::Duration,
};

use chrono::{DateTime, Utc};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::upkeep::FACT_DAYS;
use crate::{
    AppState,
    error::AppError,
    files::store::{FileStore, Readers},
    jobs::JobRegistry,
};

pub const ARCHIVE_KIND: &str = "events.archive";

/// Facts are archived a week before retention may let them go, so a run
/// that fails has days to succeed.
pub const ARCHIVE_AFTER_DAYS: i32 = FACT_DAYS - 7;
/// Facts in one file, at most (a run may go over to finish a range).
const ROWS_PER_FILE: i64 = 20_000;
/// Files one run writes, so a run stays short; the next run goes on.
const FILES_PER_RUN: usize = 50;
/// The largest file a reader fetches back.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// The most a cold file may unpack to.
const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;

/// The log with the archived rows a reader has loaded, for a query's FROM.
/// Rows retention hasn't deleted yet are in both: the live one wins.
pub const WITH_ARCHIVED: &str = "(SELECT * FROM events \
    UNION ALL SELECT * FROM events_archived cold \
    WHERE NOT EXISTS (SELECT 1 FROM events live WHERE live.position = cold.position))";

/// One file of archived facts.
#[derive(Clone, Debug, FromRow)]
pub struct Archive {
    pub id: Uuid,
    pub first_position: i64,
    pub last_position: i64,
    pub row_count: i32,
    pub first_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    pub store_id: String,
    pub object_key: String,
    pub byte_size: i64,
    pub sha256: Vec<u8>,
}

/// Write every due workspace's old facts to files, a bounded number per
/// run. Returns how many files it wrote.
pub async fn archive_due(state: &AppState) -> Result<usize, AppError> {
    let mut written = 0;
    while written < FILES_PER_RUN {
        // The due rows, not a loop over workspaces (R24 rule 5): those with
        // a fact past the cut that no file holds yet.
        let due = crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT DISTINCT fact.workspace_id
                FROM events fact
                WHERE fact.class = 'fact'
                  AND fact.created_at < now() - make_interval(days => $1)
                  AND fact.position > COALESCE(
                      (SELECT max(archive.last_position) FROM event_archives archive
                       WHERE archive.workspace_id = fact.workspace_id), 0)
                LIMIT $2
                "#,
            )
            .bind(ARCHIVE_AFTER_DAYS)
            .bind((FILES_PER_RUN - written) as i64)
            .fetch_all(&mut *connection)
            .await
        })
        .await?;
        if due.is_empty() {
            break;
        }
        for workspace in due {
            if archive_once(state, workspace, ARCHIVE_AFTER_DAYS).await? {
                written += 1;
            }
        }
    }
    Ok(written)
}

/// Write one file of `workspace`'s facts older than `days`, from where its
/// last file ended. Returns whether there was anything to write.
pub async fn archive_once(state: &AppState, workspace: Uuid, days: i32) -> Result<bool, AppError> {
    crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: None,
            workspace,
        },
        archive_once_here(state, workspace, days),
    )
    .await
}

async fn archive_once_here(state: &AppState, workspace: Uuid, days: i32) -> Result<bool, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let through = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(last_position) FROM event_archives WHERE workspace_id = $1",
    )
    .bind(workspace)
    .fetch_one(&mut *connection)
    .await?
    .unwrap_or(0);
    // The range ends at the newest of the next batch of old facts, and the
    // file holds every fact of the workspace inside it, whatever its time:
    // retention then deletes by range and never meets a fact no file holds.
    let rows = sqlx::query_as::<_, (i64, DateTime<Utc>, Option<Uuid>, Value)>(
        r#"
        SELECT fact.position, fact.created_at, fact.thing_id, to_jsonb(fact)
        FROM events fact
        WHERE fact.workspace_id = $1 AND fact.class = 'fact'
          AND fact.position > $2
          AND fact.position <= (
              SELECT max(position) FROM (
                  SELECT position FROM events
                  WHERE workspace_id = $1 AND class = 'fact' AND position > $2
                    AND created_at < now() - make_interval(days => $3)
                  ORDER BY position LIMIT $4) batch)
        ORDER BY fact.position
        "#,
    )
    .bind(workspace)
    .bind(through)
    .bind(days)
    .bind(ROWS_PER_FILE)
    .fetch_all(&mut *connection)
    .await?;
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Ok(false);
    };
    let (first_position, last_position) = (first.0, last.0);
    let first_at = rows.iter().map(|row| row.1).min().unwrap_or(first.1);
    let last_at = rows.iter().map(|row| row.1).max().unwrap_or(last.1);
    let mut things = rows.iter().filter_map(|row| row.2).collect::<Vec<_>>();
    things.sort_unstable();
    things.dedup();
    let store =
        FileStore::for_uploads(state, &mut connection, workspace, Readers::Workspace).await?;
    connection.commit().await?;

    let mut lines = Vec::new();
    for (_, _, _, row) in &rows {
        serde_json::to_writer(&mut lines, row)
            .map_err(|_| AppError::Internal("an archived fact could not be written"))?;
        lines.push(b'\n');
    }
    let bytes = gzip(&lines)?;
    let id = Uuid::now_v7();
    let key = format!(
        "{}/events/{first_position}-{last_position}-{}.jsonl.gz",
        workspace.simple(),
        id.simple()
    );
    store.put(&key, &bytes).await?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let recorded = sqlx::query(
        r#"
        INSERT INTO event_archives (
            id, workspace_id, first_position, last_position, first_at, last_at,
            row_count, thing_ids, store_id, object_key, byte_size, sha256
        )
        SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12
        -- Another run got there first: its file stands, this one goes.
        WHERE NOT EXISTS (SELECT 1 FROM event_archives
                          WHERE workspace_id = $2 AND last_position >= $3)
        "#,
    )
    .bind(id)
    .bind(workspace)
    .bind(first_position)
    .bind(last_position)
    .bind(first_at)
    .bind(last_at)
    .bind(i32::try_from(rows.len()).unwrap_or(i32::MAX))
    .bind(&things)
    .bind(store.id())
    .bind(&key)
    .bind(i64::try_from(bytes.len()).unwrap_or(i64::MAX))
    .bind(Sha256::digest(&bytes).to_vec())
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    transaction.commit().await?;
    if recorded == 0 {
        store.delete(&key).await?;
    }
    Ok(true)
}

/// `workspace`'s files a reader paging back may need, newest first: those
/// holding positions before `before` (when given), naming `thing` (when
/// given), and starting before `before_at` (when given).
pub async fn reaching_back(
    connection: &mut PgConnection,
    workspace: Uuid,
    thing: Option<Uuid>,
    before: Option<i64>,
    before_at: Option<DateTime<Utc>>,
) -> Result<Vec<Archive>, AppError> {
    Ok(sqlx::query_as::<_, Archive>(
        r#"
        SELECT id, first_position, last_position, row_count, first_at, last_at,
               store_id, object_key, byte_size, sha256
        FROM event_archives
        WHERE workspace_id = $1
          AND ($2::uuid IS NULL OR thing_ids @> ARRAY[$2::uuid])
          AND ($3::bigint IS NULL OR first_position < $3)
          AND ($4::timestamptz IS NULL OR first_at < $4)
        ORDER BY last_position DESC
        "#,
    )
    .bind(workspace)
    .bind(thing)
    .bind(before)
    .bind(before_at)
    .fetch_all(&mut *connection)
    .await?)
}

/// One file's rows, fetched from its store. A network call: never inside a
/// transaction.
pub async fn fetch(state: &AppState, archive: &Archive) -> Result<Vec<Value>, AppError> {
    let store = FileStore::resolve(state, &archive.store_id).await?;
    read_cold(
        &store,
        &archive.object_key,
        archive.byte_size,
        &archive.sha256,
    )
    .await?
    .split(|byte| *byte == b'\n')
    .filter(|line| !line.is_empty())
    .map(|line| {
        serde_json::from_slice(line)
            .map_err(|_| AppError::Internal("an archived log file is damaged"))
    })
    .collect()
}

/// Puts fetched rows where [`WITH_ARCHIVED`] reads them, for the rest of
/// this transaction.
pub async fn load(connection: &mut PgConnection, rows: Vec<Value>) -> Result<(), AppError> {
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS events_archived (LIKE events) ON COMMIT DROP")
        .execute(&mut *connection)
        .await?;
    for chunk in rows.chunks(5_000) {
        sqlx::query(
            "INSERT INTO events_archived \
             SELECT * FROM jsonb_populate_recordset(NULL::events_archived, $1)",
        )
        .bind(Value::Array(chunk.to_vec()))
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

pub(crate) fn gzip(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish())
        .map_err(|_| AppError::Internal("an archive could not be compressed"))
}

/// Reads a cold file back only as it was written: exactly its recorded
/// size from the store, its SHA-256 checked, then unpacked under
/// [`MAX_UNPACKED_BYTES`]. A store that serves anything else is refused.
pub(crate) async fn read_cold(
    store: &FileStore,
    key: &str,
    byte_size: i64,
    sha256: &[u8],
) -> Result<Vec<u8>, AppError> {
    let size = u64::try_from(byte_size)
        .ok()
        .filter(|size| *size <= MAX_FILE_BYTES)
        .ok_or(AppError::Internal("an archive is damaged"))?;
    let bytes = store.read_all(key, size).await?;
    if u64::try_from(bytes.len()).ok() != Some(size) || Sha256::digest(&bytes).as_slice() != sha256
    {
        return Err(AppError::Internal("an archive is damaged"));
    }
    gunzip(&bytes)
}

/// Unpacks a cold file, refusing one that unpacks past
/// [`MAX_UNPACKED_BYTES`].
pub(crate) fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    gunzip_within(bytes, MAX_UNPACKED_BYTES)
}

fn gunzip_within(bytes: &[u8], limit: u64) -> Result<Vec<u8>, AppError> {
    let mut text = Vec::new();
    GzDecoder::new(bytes)
        .take(limit + 1)
        .read_to_end(&mut text)
        .map_err(|_| AppError::Internal("an archive is damaged"))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > limit {
        return Err(AppError::Internal("an archive unpacks past its limit"));
    }
    Ok(text)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(ARCHIVE_KIND, |context| async move {
        archive_due(&context.state).await?;
        Ok(())
    });
    registry.every(ARCHIVE_KIND, Duration::from_secs(6 * 60 * 60));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_unpacks_past_its_limit_is_refused() {
        let packed = gzip(&[b'a'; 4096]).expect("gzip");
        assert_eq!(gunzip_within(&packed, 4096).expect("fits").len(), 4096);
        assert!(
            gunzip_within(&packed, 4095).is_err(),
            "one byte over is refused"
        );
        assert!(gunzip(b"not gzip").is_err(), "damaged input is refused");
    }
}

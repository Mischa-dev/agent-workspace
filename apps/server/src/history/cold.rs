//! Cold history (K3.25, ruling R24 rule 7). A page version older than a
//! year (never a page's newest) has its body moved into a compressed file
//! of that page's old versions in the workspace's store, recorded in
//! `history_archives`; the row stays, with the body's digest, so the
//! history list is as long as ever and still says what changed.
//!
//! Anything that reads an old body (showing a version, restoring it)
//! first calls [`warm_page`], which puts the body back in its row: a
//! network call, so it runs before the reader's transaction, never inside
//! one. Warming reveals nothing (it returns nothing), so it may run before
//! the reader's access check. A warmed version keeps its file and is not
//! archived again.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    events::archive::{gunzip, gzip},
    files::store::FileStore,
    jobs::JobRegistry,
};

pub const ARCHIVE_KIND: &str = "history.archive";

/// Page versions older than this move to the store.
pub const COLD_AFTER_DAYS: i32 = 365;
/// Pages one run archives; the next run goes on.
const PAGES_PER_RUN: i64 = 100;
/// Versions in one file, at most.
const VERSIONS_PER_FILE: i64 = 500;
/// The largest file read back.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// One file of a page's old versions.
#[derive(Debug, Deserialize, Serialize)]
struct PageFile {
    kind: String,
    thing: Uuid,
    versions: Vec<PageVersion>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PageVersion {
    version: i64,
    body: String,
}

/// Move due page versions out, a bounded number of pages per run. Returns
/// how many files it wrote.
pub async fn archive_due(state: &AppState) -> Result<usize, AppError> {
    let due = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, (Uuid, Uuid)>(
            r#"
            SELECT DISTINCT revision.workspace_id, revision.page_id
            FROM project_page_revisions revision
            WHERE revision.archive_id IS NULL
              AND revision.created_at < now() - make_interval(days => $1)
              AND revision.version < (SELECT max(newest.version)
                                      FROM project_page_revisions newest
                                      WHERE newest.page_id = revision.page_id)
            LIMIT $2
            "#,
        )
        .bind(COLD_AFTER_DAYS)
        .bind(PAGES_PER_RUN)
        .fetch_all(&mut *connection)
        .await
    })
    .await?;
    let mut written = 0;
    for (workspace, page) in due {
        if archive_page(state, workspace, page, COLD_AFTER_DAYS).await? {
            written += 1;
        }
    }
    Ok(written)
}

/// Move `page`'s versions older than `days` (but its newest) into one
/// file. Returns whether there was anything to move.
pub async fn archive_page(
    state: &AppState,
    workspace: Uuid,
    page: Uuid,
    days: i32,
) -> Result<bool, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let versions = sqlx::query_as::<_, (i64, String)>(
        r#"
        SELECT revision.version, revision.body
        FROM project_page_revisions revision
        WHERE revision.page_id = $1
          AND revision.archive_id IS NULL AND revision.body IS NOT NULL
          AND revision.created_at < now() - make_interval(days => $2)
          AND revision.version < (SELECT max(newest.version)
                                  FROM project_page_revisions newest
                                  WHERE newest.page_id = $1)
        ORDER BY revision.version
        LIMIT $3
        "#,
    )
    .bind(page)
    .bind(days)
    .bind(VERSIONS_PER_FILE)
    .fetch_all(&mut *connection)
    .await?;
    let (Some(first), Some(last)) = (versions.first(), versions.last()) else {
        return Ok(false);
    };
    let (first, last) = (first.0, last.0);
    let store = FileStore::for_uploads(state, &mut connection, workspace).await?;
    connection.commit().await?;

    let numbers = versions
        .iter()
        .map(|(version, _)| *version)
        .collect::<Vec<_>>();
    let file = PageFile {
        kind: "page".to_owned(),
        thing: page,
        versions: versions
            .into_iter()
            .map(|(version, body)| PageVersion { version, body })
            .collect(),
    };
    let json = serde_json::to_vec(&file)
        .map_err(|_| AppError::Internal("page versions could not be written"))?;
    let id = Uuid::now_v7();
    let key = format!(
        "{}/history/page/{}/{first}-{last}-{}.json.gz",
        workspace.simple(),
        page.simple(),
        id.simple()
    );
    store.put(&key, &gzip(&json)?).await?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    sqlx::query(
        "INSERT INTO history_archives (id, workspace_id, kind, thing_id, first_version, \
         last_version, version_count, store_id, object_key) \
         VALUES ($1, $2, 'page', $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(workspace)
    .bind(page)
    .bind(first)
    .bind(last)
    .bind(i32::try_from(numbers.len()).unwrap_or(i32::MAX))
    .bind(store.id())
    .bind(&key)
    .execute(&mut *transaction)
    .await?;
    let moved = sqlx::query(
        "UPDATE project_page_revisions \
         SET body_md5 = md5(body), body = NULL, archive_id = $3 \
         WHERE page_id = $1 AND version = ANY($2) AND archive_id IS NULL AND body IS NOT NULL",
    )
    .bind(page)
    .bind(&numbers)
    .bind(id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if moved == 0 {
        // Another run moved them first: its file stands, this one goes.
        transaction.rollback().await?;
        store.delete(&key).await?;
        return Ok(false);
    }
    transaction.commit().await?;
    Ok(true)
}

/// Put an archived page version's body back in its row, so readers find
/// it where they always did. Does nothing for a version that isn't cold.
/// A network call: never inside a transaction.
pub async fn warm_page(state: &AppState, page: Uuid, version: i64) -> Result<(), AppError> {
    let archive = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, (String, String)>(
            "SELECT archive.store_id, archive.object_key \
             FROM project_page_revisions revision \
             JOIN history_archives archive ON archive.id = revision.archive_id \
             WHERE revision.page_id = $1 AND revision.version = $2 AND revision.body IS NULL",
        )
        .bind(page)
        .bind(version)
        .fetch_optional(&mut *connection)
        .await
    })
    .await?;
    let Some((store_id, key)) = archive else {
        return Ok(());
    };
    let store = FileStore::resolve(state, &store_id).await?;
    let file: PageFile =
        serde_json::from_slice(&gunzip(&store.read_all(&key, MAX_FILE_BYTES).await?)?)
            .map_err(|_| AppError::Internal("an archived page history file is damaged"))?;
    let body = file
        .versions
        .into_iter()
        .find(|kept| kept.version == version)
        .map(|kept| kept.body)
        .ok_or(AppError::Internal(
            "an archived page version is missing from its file",
        ))?;
    crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query(
            "UPDATE project_page_revisions SET body = $3 \
             WHERE page_id = $1 AND version = $2 AND body IS NULL",
        )
        .bind(page)
        .bind(version)
        .bind(&body)
        .execute(&mut *connection)
        .await
    })
    .await?;
    Ok(())
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(ARCHIVE_KIND, |context| async move {
        archive_due(&context.state).await?;
        Ok(())
    });
    registry.every(ARCHIVE_KIND, Duration::from_secs(24 * 60 * 60));
}

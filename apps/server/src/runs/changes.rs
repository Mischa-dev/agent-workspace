//! A1.17: what a run changed, as the host doing it reports it
//! (`run_changes_report`): the files and the patch, latest report only. The
//! server reads nothing on the host's disk to show a run's changes.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// The most files one report keeps.
pub const MAX_FILES: usize = 2_000;
/// The largest patch kept; a bigger one is dropped and the report says it
/// was truncated.
pub const MAX_PATCH_BYTES: usize = 768 * 1024;
const MAX_PATH_CHARS: usize = 1_000;
/// What a file can have undergone.
pub const STATUSES: [&str; 4] = ["added", "modified", "deleted", "renamed"];

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangedFile {
    /// The file's path in the repository.
    pub path: String,
    /// added, modified, deleted or renamed.
    pub status: String,
    /// Lines added; null for a binary file.
    #[serde(default)]
    pub added: Option<u32>,
    /// Lines removed; null for a binary file.
    #[serde(default)]
    pub removed: Option<u32>,
}

/// A run's changes as last reported.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub files: Vec<ChangedFile>,
    /// The unified diff, or empty when it was too big.
    pub patch: String,
    pub truncated: bool,
    pub reported_at: DateTime<Utc>,
}

/// What's wrong with a report, if anything: the field and what to do.
pub fn problem(files: &[ChangedFile]) -> Option<(&'static str, &'static str)> {
    if files.len() > MAX_FILES {
        return Some((
            "files",
            "Send at most 2,000 files; leave the rest out and set truncated.",
        ));
    }
    for file in files {
        if file.path.is_empty()
            || file.path.chars().count() > MAX_PATH_CHARS
            || file.path.chars().any(char::is_control)
        {
            return Some(("files", "Each path is 1 to 1,000 characters on one line."));
        }
        if !STATUSES.contains(&file.status.as_str()) {
            return Some((
                "files",
                "A file's status is added, modified, deleted or renamed.",
            ));
        }
    }
    None
}

/// Keeps `files` and `patch` as the run's changes, replacing any earlier
/// report. A patch over the limit is dropped and the report marked
/// truncated.
pub async fn put(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    reporter: Uuid,
    files: &[ChangedFile],
    patch: &str,
    truncated: bool,
) -> Result<(), AppError> {
    let (patch, truncated) = if patch.len() > MAX_PATCH_BYTES {
        ("", true)
    } else {
        (patch, truncated)
    };
    sqlx::query(
        r#"
        INSERT INTO run_changes (
            run_id, workspace_id, space_id, files, patch, truncated, reported_by_account_id
        )
        SELECT run.id, run.workspace_id, run.space_id, $2, $3, $4, $5
        FROM runs run WHERE run.id = $1
        ON CONFLICT (run_id) DO UPDATE SET
            files = EXCLUDED.files,
            patch = EXCLUDED.patch,
            truncated = EXCLUDED.truncated,
            reported_by_account_id = EXCLUDED.reported_by_account_id,
            reported_at = now()
        "#,
    )
    .bind(run_id)
    .bind(sqlx::types::Json(files))
    .bind(patch)
    .bind(truncated)
    .bind(reporter)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The run's changes as last reported, if any were. The caller has checked
/// the reader may see the run and its code.
pub async fn get(connection: &mut PgConnection, run_id: Uuid) -> Result<Option<Changes>, AppError> {
    let row =
        sqlx::query_as::<
            _,
            (
                sqlx::types::Json<Vec<ChangedFile>>,
                String,
                bool,
                DateTime<Utc>,
            ),
        >("SELECT files, patch, truncated, reported_at FROM run_changes WHERE run_id = $1")
        .bind(run_id)
        .fetch_optional(connection)
        .await?;
    Ok(row.map(|(files, patch, truncated, reported_at)| Changes {
        files: files.0,
        patch,
        truncated,
        reported_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::{ChangedFile, problem};

    fn file(path: &str, status: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            status: status.to_owned(),
            added: Some(1),
            removed: None,
        }
    }

    #[test]
    fn a_report_names_real_paths_and_statuses() {
        assert!(problem(&[file("src/main.rs", "modified")]).is_none());
        assert!(problem(&[file("", "added")]).is_some());
        assert!(problem(&[file("a\nb", "added")]).is_some());
        assert!(problem(&[file("a", "copied")]).is_some());
    }
}

//! A1.19: the pull request read model behind the code-host seam (A1
//! section 3.14). What the server shows about a pull request, wherever its
//! code host keeps it (`github_pull_requests` today), and how a pull
//! request a host reported for a run is tied to that run's thread and task.
//!
//! A reported pull request is tied when both are known: when the run
//! reports it (`link_reported`), if the space already has it, or when the
//! space picks it up afterwards (`link_runs_reporting`), whichever comes
//! second.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, runs::store::RunRow};

/// A pull request as the server shows it.
#[derive(Clone, Debug, FromRow, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// The space (project) its repository is connected in.
    #[schemars(with = "String")]
    pub space_id: Uuid,
    #[schemars(with = "String")]
    pub repository_id: Uuid,
    /// `owner/name`.
    pub repository: String,
    pub number: i64,
    pub url: String,
    pub title: String,
    /// open, closed or merged.
    pub state: String,
    pub draft: bool,
    /// unknown, pending, success or failure.
    pub checks_state: String,
    /// What reviewers said so far, as the code host sums it up.
    pub review_state: String,
    pub head_ref: String,
    pub base_ref: String,
    pub merged_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

const SELECT: &str = r#"
    SELECT pull_request.id, pull_request.project_id AS space_id,
           pull_request.repository_id,
           repository.owner || '/' || repository.name AS repository,
           pull_request.pull_request_number AS number, pull_request.html_url AS url,
           pull_request.title, pull_request.state, pull_request.draft,
           pull_request.checks_state, pull_request.review_state,
           pull_request.head_ref, pull_request.base_ref, pull_request.merged_at,
           pull_request.external_updated_at AS updated_at
    FROM github_pull_requests pull_request
    JOIN repositories repository ON repository.id = pull_request.repository_id
"#;

/// The pull request an address names, among the repositories connected in
/// `space_id`; none when no code host knows the address or the space
/// hasn't synced that pull request.
pub async fn by_url(
    connection: &mut PgConnection,
    space_id: Uuid,
    url: &str,
) -> Result<Option<PullRequest>, AppError> {
    let Some((repo, number)) = super::pull_request_by_url(url) else {
        return Ok(None);
    };
    Ok(sqlx::query_as::<_, PullRequest>(&format!(
        r#"{SELECT}
        WHERE pull_request.project_id = $1 AND repository.code_host = $2
          AND lower(repository.owner || '/' || repository.name) = lower($3)
          AND pull_request.pull_request_number = $4"#
    ))
    .bind(space_id)
    .bind(repo.code_host)
    .bind(&repo.full_name)
    .bind(number)
    .fetch_optional(connection)
    .await?)
}

/// The pull requests a run produced that `viewer` may see: the ones a
/// built-in run's worktree opened or asked for, and the ones the run
/// reported as results (`reported_urls`), newest first.
pub async fn for_run(
    connection: &mut PgConnection,
    viewer: Uuid,
    run_id: Uuid,
    space_id: Option<Uuid>,
    reported_urls: &[String],
) -> Result<Vec<PullRequest>, AppError> {
    let mut found = sqlx::query_as::<_, PullRequest>(&format!(
        r#"{SELECT}
        WHERE pull_request.id IN (
            SELECT link.github_pull_request_id FROM github_pull_request_workspace_links link
            WHERE link.agent_run_id = $1
            UNION
            SELECT asked.github_pull_request_id FROM github_pull_request_requests asked
            WHERE asked.agent_run_id = $1 AND asked.github_pull_request_id IS NOT NULL
        )
        LIMIT 50"#
    ))
    .bind(run_id)
    .fetch_all(&mut *connection)
    .await?;
    if let Some(space_id) = space_id {
        for url in reported_urls.iter().take(50) {
            if let Some(pull_request) = by_url(&mut *connection, space_id, url).await?
                && !found.iter().any(|known| known.id == pull_request.id)
            {
                found.push(pull_request);
            }
        }
    }
    let mut visible = Vec::with_capacity(found.len());
    for pull_request in found {
        // -- visibility: the run was checked by the caller; a pull request
        // a private thread's run opened stays with that thread (0050).
        let shown = sqlx::query_scalar::<_, bool>("SELECT pull_request_visible_to($1, $2)")
            .bind(pull_request.id)
            .bind(viewer)
            .fetch_one(&mut *connection)
            .await?;
        if shown {
            visible.push(pull_request);
        }
    }
    visible.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
    Ok(visible)
}

/// A run reported a pull request (`result {kind: "pull_request", url}`):
/// when its space has that pull request, tie it to the run's thread and
/// task the way a built-in run's pull request is, so checks, reviews, task
/// moves and notifications follow it. Returns the pull request when found.
pub async fn link_reported(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RunRow,
    url: &str,
) -> Result<Option<PullRequest>, AppError> {
    let Some(space_id) = run.space_id else {
        return Ok(None);
    };
    let Some(pull_request) = by_url(transaction, space_id, url).await? else {
        return Ok(None);
    };
    tie(transaction, space_id, pull_request.id, run).await?;
    Ok(Some(pull_request))
}

/// A space just picked up a pull request: tie it to every run in the space
/// that reported it before it was known.
pub async fn link_runs_reporting(
    transaction: &mut Transaction<'_, Postgres>,
    space_id: Uuid,
    pull_request_id: Uuid,
    url: &str,
) -> Result<usize, AppError> {
    let runs = crate::runs::ops::reporting_pull_request(transaction, space_id, url).await?;
    for run in &runs {
        tie(transaction, space_id, pull_request_id, run).await?;
    }
    Ok(runs.len())
}

async fn tie(
    transaction: &mut Transaction<'_, Postgres>,
    space_id: Uuid,
    pull_request_id: Uuid,
    run: &RunRow,
) -> Result<(), AppError> {
    if let Some(conversation_id) = run.conversation_id {
        // Pull requests link to threads only (0023's trigger), and the
        // link is made in the run's agent's name, so it must be a member.
        sqlx::query(
            r#"
            INSERT INTO github_pull_request_conversation_links (
                project_id, github_pull_request_id, conversation_id, source, created_by_actor_id
            )
            SELECT $1, $2, $3, 'run_artifact', $4
            WHERE EXISTS (SELECT 1 FROM project_memberships
                          WHERE project_id = $1 AND actor_id = $4)
              AND EXISTS (SELECT 1 FROM conversations
                          WHERE project_id = $1 AND id = $3 AND kind = 'thread')
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(space_id)
        .bind(pull_request_id)
        .bind(conversation_id)
        .bind(run.account_id)
        .execute(&mut **transaction)
        .await?;
    }
    if let Some(task_id) = run.work_item_id {
        let linked = sqlx::query(
            r#"
            INSERT INTO github_pull_request_work_item_links (
                project_id, github_pull_request_id, work_item_id, source, confirmed,
                created_by_actor_id
            )
            SELECT $1, $2, $3, 'run_artifact', true, $4
            WHERE EXISTS (SELECT 1 FROM project_memberships
                          WHERE project_id = $1 AND actor_id = $4)
            ON CONFLICT (github_pull_request_id, work_item_id) DO UPDATE
                SET source = 'run_artifact', confirmed = true
                WHERE github_pull_request_work_item_links.source IN ('branch_key', 'body_marker')
            "#,
        )
        .bind(space_id)
        .bind(pull_request_id)
        .bind(task_id)
        .bind(run.account_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected();
        if linked > 0
            && let Some(version) = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM task_rows WHERE project_id = $1 AND id = $2",
            )
            .bind(space_id)
            .bind(task_id)
            .fetch_optional(&mut **transaction)
            .await?
        {
            crate::outbox::enqueue_work_item_updated(transaction, space_id, task_id, version)
                .await?;
        }
    }
    Ok(())
}

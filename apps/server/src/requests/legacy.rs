//! A1.7: the old approval and question tables, kept equal to `requests`
//! until A1.22 makes them views. Every writer of `agent_approvals` or
//! `person_questions` calls one of these in the same transaction, right
//! after its write. In code, not a trigger (A1 section 4.1).

use sqlx::PgExecutor;
use uuid::Uuid;

use crate::error::AppError;

/// After writing one approval.
pub async fn approval_written(executor: impl PgExecutor<'_>, id: Uuid) -> Result<(), AppError> {
    sqlx::query("SELECT requests_sync_approval($1)")
        .bind(id)
        .execute(executor)
        .await?;
    Ok(())
}

/// After writing every approval of a run (cancelling the pending ones).
pub async fn run_approvals_written(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "SELECT requests_sync_approval(id) FROM agent_approvals WHERE agent_run_id = $1 ORDER BY id",
    )
    .bind(run_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// After writing the approvals of sponsored run requests.
pub async fn request_approvals_written(
    executor: impl PgExecutor<'_>,
    external_request_ids: &[Uuid],
) -> Result<(), AppError> {
    sqlx::query(
        "SELECT requests_sync_approval(id) FROM agent_approvals WHERE external_execution_request_id = ANY($1) ORDER BY id",
    )
    .bind(external_request_ids)
    .execute(executor)
    .await?;
    Ok(())
}

/// After a sweep that changed approvals of many runs: every open request
/// whose approval is no longer pending.
pub async fn settled_approvals_written(executor: impl PgExecutor<'_>) -> Result<(), AppError> {
    sqlx::query(
        r#"
        SELECT requests_sync_approval(approval.id)
        FROM agent_approvals approval
        JOIN requests request ON request.id = approval.id
        WHERE request.status = 'open' AND approval.status <> 'pending'
        ORDER BY approval.id
        "#,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// After writing one person question.
pub async fn question_written(executor: impl PgExecutor<'_>, id: Uuid) -> Result<(), AppError> {
    sqlx::query("SELECT requests_sync_question($1)")
        .bind(id)
        .execute(executor)
        .await?;
    Ok(())
}

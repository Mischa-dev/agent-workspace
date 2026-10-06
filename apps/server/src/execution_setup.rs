//! What a project needs before Codex can run in it: the same-project local
//! Codex provider connection and the default `@agent` execution worker.
//!
//! The dev seed and the first person to connect their Codex account in a
//! project both create them through here, so a fresh project becomes
//! execution-ready without seed data or database edits. Every step is an
//! upsert, and callers hold the project's setup lock, so repeating or racing
//! setup never creates duplicates.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// Serialize execution setup for one project for the rest of the transaction.
pub async fn lock_project_setup(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("execution-setup:{project_id}"))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// The project's local Codex App Server connection, created if missing.
pub async fn ensure_local_provider(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO provider_connections (
            id, project_id, kind, host, capabilities
        )
        VALUES ($1, $2, 'codex_app_server', 'local_project_host', $3)
        ON CONFLICT (project_id, kind, host)
        DO UPDATE SET id = provider_connections.id
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(serde_json::json!({
        "supports_streaming": true,
        "supports_commands": true,
        "supports_file_changes": true,
        "supports_approvals": true,
        "supports_questions": true,
        "supports_resume": true,
        "supports_interrupt": true,
        "supports_pull_request_creation": false,
        "supports_remote_execution": false
    }))
    .fetch_one(&mut **transaction)
    .await?)
}

/// The project's default `@agent` worker on `provider_connection_id`, created
/// with its worker actor and membership if missing. `execution_profile_id` is
/// its default profile; runs still use the profile of whoever starts them.
/// An existing worker keeps its state. A new one is enabled only when
/// `profile_available`; otherwise it waits in `pending_setup` until
/// `activate_pending_worker` turns it on.
pub async fn ensure_default_worker(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    provider_connection_id: Uuid,
    execution_profile_id: Uuid,
    profile_available: bool,
) -> Result<(Uuid, Uuid), AppError> {
    let existing_worker_actor = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT pm.actor_id
          FROM project_memberships pm
         WHERE pm.project_id = $1
           AND lower(pm.handle) = 'agent'
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let worker_actor_id = existing_worker_actor.unwrap_or_else(Uuid::now_v7);
    sqlx::query(
        r#"
        INSERT INTO actors (id, kind)
        VALUES ($1, 'execution_worker')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(worker_actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'worker', 'Agent Workspace Worker', 'agent')
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET updated_at = now()
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(worker_actor_id)
    .execute(&mut **transaction)
    .await?;
    let execution_worker_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id,
            execution_profile_id, enabled, pending_setup
        )
        VALUES (
            $1, $2, $3, $4, $5,
            COALESCE(
                (
                    SELECT existing.enabled
                    FROM execution_workers existing
                    WHERE existing.project_id = $2 AND existing.actor_id = $3
                ),
                $6
            ),
            NOT $6
        )
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET id = execution_workers.id
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(worker_actor_id)
    .bind(provider_connection_id)
    .bind(execution_profile_id)
    .bind(profile_available)
    .fetch_one(&mut **transaction)
    .await?;
    Ok((worker_actor_id, execution_worker_id))
}

/// Once a profile is signed in and available, a worker still waiting for
/// setup on the same provider switches to it and turns on.
pub async fn activate_pending_worker(
    pool: &sqlx::PgPool,
    execution_profile_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE execution_workers worker
        SET execution_profile_id = profile.id, enabled = true, pending_setup = false,
            updated_at = now()
        FROM execution_profiles profile
        WHERE profile.id = $1
          AND profile.status = 'available'
          AND worker.project_id = profile.project_id
          AND worker.provider_connection_id = profile.provider_connection_id
          AND worker.pending_setup
        "#,
    )
    .bind(execution_profile_id)
    .execute(pool)
    .await?;
    Ok(())
}

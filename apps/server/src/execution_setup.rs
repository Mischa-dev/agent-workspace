//! What a project needs before an agent can run in it: a connection to each
//! harness its people use and the default `@agent` execution worker.
//!
//! The dev seed and the first person to connect a provider account in a
//! project both create them through here, so a fresh project becomes
//! execution-ready without seed data or database edits. Every step is an
//! upsert, and callers hold the project's setup lock, so repeating or racing
//! setup never creates duplicates.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, provider::manifest::AdapterManifest};

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

/// The project's connection to one harness, created if missing. Its host and
/// capabilities come from the adapter's manifest.
pub async fn ensure_provider_connection(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    manifest: &AdapterManifest,
) -> Result<Uuid, AppError> {
    let features = manifest.features;
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO provider_connections (
            id, project_id, kind, host, capabilities, binary_version
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (project_id, kind, host)
        DO UPDATE SET id = provider_connections.id
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(&manifest.kind)
    .bind(manifest.runs_on.host())
    .bind(serde_json::json!({
        "supports_streaming": features.streaming,
        "supports_commands": features.commands,
        "supports_file_changes": features.file_changes,
        "supports_approvals": features.approvals,
        "supports_questions": features.questions,
        "supports_resume": features.resume,
        "supports_interrupt": features.interrupt,
        "supports_pull_request_creation": features.pull_request_creation,
        "supports_remote_execution": features.cloud
    }))
    .bind(&manifest.versions.minimum)
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
    // The dev seed (profile already available) also runs against schemas from
    // before migration 0054 in upgrade tests, so only the pending path names
    // the pending_setup column.
    let insert = if profile_available {
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id,
            execution_profile_id, enabled
        )
        VALUES (
            $1, $2, $3, $4, $5,
            COALESCE(
                (
                    SELECT existing.enabled
                    FROM execution_workers existing
                    WHERE existing.project_id = $2 AND existing.actor_id = $3
                ),
                true
            )
        )
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET id = execution_workers.id
        RETURNING id
        "#
    } else {
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id,
            execution_profile_id, enabled, pending_setup
        )
        VALUES ($1, $2, $3, $4, $5, false, true)
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET id = execution_workers.id
        RETURNING id
        "#
    };
    let execution_worker_id = sqlx::query_scalar::<_, Uuid>(insert)
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(worker_actor_id)
        .bind(provider_connection_id)
        .bind(execution_profile_id)
        .fetch_one(&mut **transaction)
        .await?;
    Ok((worker_actor_id, execution_worker_id))
}

/// Once a profile is signed in and available, a worker in the same project
/// still waiting for setup switches to it and turns on.
pub async fn activate_pending_worker(
    pool: &sqlx::PgPool,
    execution_profile_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE execution_workers worker
        SET execution_profile_id = profile.id,
            provider_connection_id = profile.provider_connection_id,
            enabled = true, pending_setup = false,
            updated_at = now()
        FROM execution_profiles profile
        WHERE profile.id = $1
          AND profile.status = 'available'
          AND worker.project_id = profile.project_id
          AND worker.pending_setup
        "#,
    )
    .bind(execution_profile_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// `activate_pending_worker` for every profile that uses an account, so
/// signing in once turns on waiting workers in each of its projects.
pub async fn activate_pending_worker_for_account(
    pool: &sqlx::PgPool,
    provider_account_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE execution_workers worker
        SET execution_profile_id = profile.id,
            provider_connection_id = profile.provider_connection_id,
            enabled = true, pending_setup = false,
            updated_at = now()
        FROM execution_profiles profile
        WHERE profile.provider_account_id = $1
          AND profile.status = 'available'
          AND worker.project_id = profile.project_id
          AND worker.pending_setup
        "#,
    )
    .bind(provider_account_id)
    .execute(pool)
    .await?;
    Ok(())
}

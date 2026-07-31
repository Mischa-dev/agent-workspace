use std::path::PathBuf;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{config::AppConfig, error::AppError};

#[derive(Debug, Serialize)]
pub struct DevSeed {
    pub human_actor_id: Uuid,
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub channel_id: Uuid,
    pub thread_id: Uuid,
    pub worker_actor_id: Uuid,
    pub execution_worker_id: Uuid,
    pub provider_connection_id: Uuid,
    pub execution_profile_id: Uuid,
    pub repository_id: Uuid,
}

pub async fn dev_seed(pool: &PgPool, config: &AppConfig) -> Result<DevSeed, AppError> {
    if !config.environment.is_development() || !config.server_addr.ip().is_loopback() {
        return Err(AppError::DevelopmentAuthUnavailable);
    }

    let retained_profile = config.codex_home_root.join("m0-retained");
    std::fs::create_dir_all(&retained_profile)
        .map_err(|_| AppError::Internal("create retained Codex profile"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&retained_profile, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| AppError::Internal("protect retained Codex profile"))?;
    }

    let mut transaction = pool.begin().await?;

    let organization_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO organizations (id, name, slug)
        VALUES ($1, 'Mischa Personal', 'mischa-personal')
        ON CONFLICT (slug) DO UPDATE SET updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .fetch_one(&mut *transaction)
    .await?;

    let project_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES ($1, $2, 'AW', 'Agent Workspace')
        ON CONFLICT (organization_id, lower(project_key))
        DO UPDATE SET name = EXCLUDED.name, updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .fetch_one(&mut *transaction)
    .await?;

    let existing_human_actor = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT pm.actor_id
          FROM project_memberships pm
         WHERE pm.project_id = $1
           AND lower(pm.handle) = 'mischa'
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let human_actor_id = existing_human_actor.unwrap_or_else(Uuid::now_v7);
    sqlx::query(
        r#"
        INSERT INTO actors (id, kind)
        VALUES ($1, 'human')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(human_actor_id)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', 'Mischa Nelson', 'mischa')
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET enabled = true, updated_at = now()
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(human_actor_id)
    .execute(&mut *transaction)
    .await?;

    let mirror_path = seed_mirror_path(config);
    let repository_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, github_app_installation_id, mirror_path, trusted
        )
        VALUES (
            $1, $2, 1317905810, 'Mischa-dev', 'agent-workspace-m0-spike',
            'https://github.com/Mischa-dev/agent-workspace-m0-spike.git',
            'main', 150232852, $3, true
        )
        ON CONFLICT (project_id, owner, name)
        DO UPDATE SET
            mirror_path = EXCLUDED.mirror_path,
            github_app_installation_id = EXCLUDED.github_app_installation_id,
            trusted = true,
            enabled = true,
            updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(mirror_path.to_string_lossy().as_ref())
    .fetch_one(&mut *transaction)
    .await?;

    let channel_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO conversations (id, project_id, kind, name)
        VALUES ($1, $2, 'channel', 'implementation')
        ON CONFLICT (project_id, lower(name))
            WHERE kind = 'channel' AND archived_at IS NULL
        DO UPDATE SET updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .fetch_one(&mut *transaction)
    .await?;

    let thread_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, repository_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, 'M0 walking skeleton')
        ON CONFLICT (parent_channel_id, lower(name))
            WHERE kind = 'thread' AND archived_at IS NULL
        DO UPDATE SET repository_id = EXCLUDED.repository_id, updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(channel_id)
    .bind(repository_id)
    .fetch_one(&mut *transaction)
    .await?;

    let provider_connection_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO provider_connections (
            id, project_id, kind, host, capabilities
        )
        VALUES (
            $1, $2, 'codex_app_server', 'local_project_host',
            $3
        )
        ON CONFLICT (project_id, kind, host)
        DO UPDATE SET capabilities = EXCLUDED.capabilities, enabled = true, updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(serde_json::json!({
        "streaming": true,
        "commands": true,
        "file_changes": true,
        "approvals": true,
        "questions": true,
        "resume": true,
        "interrupt": true
    }))
    .fetch_one(&mut *transaction)
    .await?;

    let execution_profile_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id, name,
            codex_home_path, status
        )
        VALUES ($1, $2, $3, $4, 'M0 retained profile', $5, 'available')
        ON CONFLICT (project_id, name)
        DO UPDATE SET
            provider_connection_id = EXCLUDED.provider_connection_id,
            owner_actor_id = EXCLUDED.owner_actor_id,
            codex_home_path = EXCLUDED.codex_home_path,
            updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(provider_connection_id)
    .bind(human_actor_id)
    .bind(retained_profile.to_string_lossy().as_ref())
    .fetch_one(&mut *transaction)
    .await?;

    let existing_worker_actor = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT pm.actor_id
          FROM project_memberships pm
         WHERE pm.project_id = $1
           AND lower(pm.handle) = 'agent'
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut *transaction)
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
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'worker', 'Agent Workspace Worker', 'agent')
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET enabled = true, updated_at = now()
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(worker_actor_id)
    .execute(&mut *transaction)
    .await?;

    let execution_worker_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id,
            execution_profile_id
        )
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET
            provider_connection_id = EXCLUDED.provider_connection_id,
            execution_profile_id = EXCLUDED.execution_profile_id,
            enabled = true,
            updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(worker_actor_id)
    .bind(provider_connection_id)
    .bind(execution_profile_id)
    .fetch_one(&mut *transaction)
    .await?;

    transaction.commit().await?;

    Ok(DevSeed {
        human_actor_id,
        organization_id,
        project_id,
        channel_id,
        thread_id,
        worker_actor_id,
        execution_worker_id,
        provider_connection_id,
        execution_profile_id,
        repository_id,
    })
}

fn seed_mirror_path(config: &AppConfig) -> PathBuf {
    let configured = std::env::var_os("AGENT_WORKSPACE_DEV_REPOSITORY_MIRROR");
    configured.map_or_else(
        || {
            config
                .data_root
                .join("repositories")
                .join("m0-spike")
                .join("mirror.git")
        },
        PathBuf::from,
    )
}

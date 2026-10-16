use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::AppConfig, error::AppError, git_runtime::expected_repository_mirror_path,
    profile_runtime::prepare_private_profile_directory,
};

#[derive(Debug, Serialize)]
pub struct DevSeed {
    pub human_actor_id: Uuid,
    pub collaborator_actor_id: Uuid,
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

    let retained_profile = prepare_private_profile_directory(
        &config.data_root,
        &config.codex_home_root,
        "m0-retained",
    )?;

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
        INSERT INTO organization_memberships (
            organization_id, actor_id, role, enabled
        )
        VALUES ($1, $2, 'owner', true)
        ON CONFLICT (organization_id, actor_id)
        DO UPDATE SET role = 'owner', enabled = true, updated_at = now()
        "#,
    )
    .bind(organization_id)
    .bind(human_actor_id)
    .execute(&mut *transaction)
    .await?;

    let existing_collaborator_actor = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT pm.actor_id
          FROM project_memberships pm
         WHERE pm.project_id = $1
           AND lower(pm.handle) = 'm2_collaborator'
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let collaborator_actor_id = existing_collaborator_actor.unwrap_or_else(Uuid::now_v7);
    sqlx::query(
        r#"
        INSERT INTO actors (id, kind)
        VALUES ($1, 'human')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(collaborator_actor_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (
            organization_id, actor_id, role, enabled
        )
        VALUES ($1, $2, 'member', true)
        ON CONFLICT (organization_id, actor_id)
        DO UPDATE SET enabled = true, updated_at = now()
        "#,
    )
    .bind(organization_id)
    .bind(collaborator_actor_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'member', 'M2 Collaborator', 'm2_collaborator')
        ON CONFLICT (project_id, actor_id)
        DO UPDATE SET enabled = true, updated_at = now()
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(collaborator_actor_id)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO project_counters (project_id) VALUES ($1) ON CONFLICT (project_id) DO NOTHING",
    )
    .bind(project_id)
    .execute(&mut *transaction)
    .await?;
    if crate::work_items::schema_is_available(&mut transaction).await? {
        crate::work_items::ensure_default_workflow(&mut transaction, project_id).await?;
    }

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

    let repository_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM repositories WHERE project_id = $1 AND lower(owner) = 'mischa-dev' AND lower(name) = 'agent-workspace-m0-spike'",
    )
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .unwrap_or_else(Uuid::now_v7);
    let mirror_path = expected_repository_mirror_path(&config.data_root, repository_id)?;
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
        DO UPDATE SET id = repositories.id
        RETURNING id
        "#,
    )
    .bind(repository_id)
    .bind(project_id)
    .bind(runtime_path_text(&mirror_path)?)
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

    crate::execution_setup::lock_project_setup(&mut transaction, project_id).await?;
    let codex_manifest = crate::provider::codex_manifest();
    // Upgrade tests seed schemas from before provider accounts (S6), where
    // the profile itself held the private home directory, and from before
    // connections recorded a binary version.
    let has_provider_accounts =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('provider_accounts') IS NOT NULL")
            .fetch_one(&mut *transaction)
            .await?;
    let provider_connection_id = if has_provider_accounts {
        crate::execution_setup::ensure_provider_connection(
            &mut transaction,
            project_id,
            &codex_manifest,
        )
        .await?
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO provider_connections (id, project_id, kind, host, capabilities)
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
        .fetch_one(&mut *transaction)
        .await?
    };
    let profile_path = runtime_path_text(&retained_profile)?;
    let (execution_profile_id, stored_profile_path) = if has_provider_accounts {
        let account_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO provider_accounts (
                id, organization_id, owner_actor_id, adapter_kind, name,
                home_path, status
            )
            VALUES ($1, $2, $3, $4, 'M0 retained account', $5, 'available')
            ON CONFLICT (home_path)
            DO UPDATE SET id = provider_accounts.id
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(organization_id)
        .bind(human_actor_id)
        .bind(&codex_manifest.kind)
        .bind(profile_path)
        .fetch_one(&mut *transaction)
        .await?;
        let profile_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO execution_profiles (
                id, project_id, provider_connection_id, owner_actor_id, name,
                provider_account_id, status, preferred
            )
            VALUES ($1, $2, $3, $4, 'M0 retained profile', $5, 'available', true)
            ON CONFLICT (project_id, name)
            DO UPDATE SET id = execution_profiles.id
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(provider_connection_id)
        .bind(human_actor_id)
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        let stored = sqlx::query_scalar::<_, String>(
            r#"
            SELECT account.home_path
            FROM execution_profiles profile
            JOIN provider_accounts account ON account.id = profile.provider_account_id
            WHERE profile.id = $1
            "#,
        )
        .bind(profile_id)
        .fetch_one(&mut *transaction)
        .await?;
        (profile_id, stored)
    } else {
        let profile_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO execution_profiles (
                id, project_id, provider_connection_id, owner_actor_id, name,
                codex_home_path, status
            )
            VALUES ($1, $2, $3, $4, 'M0 retained profile', $5, 'available')
            ON CONFLICT (project_id, name)
            DO UPDATE SET id = execution_profiles.id
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(provider_connection_id)
        .bind(human_actor_id)
        .bind(profile_path)
        .fetch_one(&mut *transaction)
        .await?;
        let stored = sqlx::query_scalar::<_, String>(
            "SELECT codex_home_path FROM execution_profiles WHERE id = $1",
        )
        .bind(profile_id)
        .fetch_one(&mut *transaction)
        .await?;
        (profile_id, stored)
    };
    if stored_profile_path != profile_path {
        return Err(AppError::Conflict(
            "execution_profile",
            "stored profile path does not match the configured data root",
        ));
    }

    let stored_mirror_path =
        sqlx::query_scalar::<_, String>("SELECT mirror_path FROM repositories WHERE id = $1")
            .bind(repository_id)
            .fetch_one(&mut *transaction)
            .await?;
    if stored_mirror_path != runtime_path_text(&mirror_path)? {
        return Err(AppError::Conflict(
            "repository",
            "stored mirror path does not match the configured data root",
        ));
    }

    let (worker_actor_id, execution_worker_id) = crate::execution_setup::ensure_default_worker(
        &mut transaction,
        project_id,
        provider_connection_id,
        execution_profile_id,
        true,
    )
    .await?;

    transaction.commit().await?;

    Ok(DevSeed {
        human_actor_id,
        collaborator_actor_id,
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

pub(crate) fn seed_mirror_path(config: &AppConfig) -> PathBuf {
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

pub(crate) fn runtime_path_text(path: &Path) -> Result<&str, AppError> {
    path.to_str()
        .ok_or(AppError::Invalid("runtime paths must be valid UTF-8"))
}

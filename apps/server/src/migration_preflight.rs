use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    config::AppConfig,
    error::AppError,
    profile_runtime::{prepare_private_profile_directory, resolve_path_within_data_root},
    seed::{runtime_path_text, seed_mirror_path},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct M3UpgradePreflight {
    pub required: bool,
    pub profile_paths_repaired: u64,
    pub repository_paths_repaired: u64,
    pub workers_disabled: u64,
}

#[derive(Debug, FromRow)]
struct LegacyProfilePath {
    id: Uuid,
    project_id: Uuid,
    name: String,
    codex_home_path: String,
}

#[derive(Debug, FromRow)]
struct LegacyRepositoryPath {
    id: Uuid,
    project_id: Uuid,
    owner: String,
    name: String,
    mirror_path: String,
}

/// Repairs only the two path forms written by the pre-M3 development seed and
/// conservatively disables Workers whose legacy dependencies are unavailable.
/// Unknown legacy path layouts fail closed for explicit operator repair.
pub async fn prepare_m3_upgrade(
    pool: &PgPool,
    config: &AppConfig,
) -> Result<M3UpgradePreflight, AppError> {
    if !m3_preflight_required(pool).await? {
        return Ok(M3UpgradePreflight::default());
    }

    let desired_profile = prepare_private_profile_directory(
        &config.data_root,
        &config.codex_home_root,
        "m0-retained",
    )?;
    let legacy_profile = config.codex_home_root.join("m0-retained");
    let desired_mirror =
        resolve_path_within_data_root(&config.data_root, &seed_mirror_path(config))?;
    let legacy_mirror = seed_mirror_path(config);

    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(
        r#"
        LOCK TABLE provider_connections,
                   execution_profiles,
                   execution_workers,
                   repositories,
                   project_memberships,
                   actors
        IN SHARE ROW EXCLUSIVE MODE
        "#,
    )
    .execute(&mut *transaction)
    .await?;

    let profiles = sqlx::query_as::<_, LegacyProfilePath>(
        "SELECT id, project_id, name, codex_home_path FROM execution_profiles",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let mut profile_paths_repaired = 0;
    for profile in profiles {
        let stored = PathBuf::from(&profile.codex_home_path);
        if stored.is_absolute() {
            require_contained_exact_path(&config.codex_home_root, &stored)?;
            continue;
        }
        if profile.name != "M0 retained profile" || stored != legacy_profile {
            tracing::error!(
                profile_id = %profile.id,
                project_id = %profile.project_id,
                "M3 migration preflight rejected an unknown relative profile path"
            );
            return Err(AppError::Conflict(
                "database_migration",
                "legacy Execution Profile paths require operator repair",
            ));
        }
        sqlx::query("UPDATE execution_profiles SET codex_home_path = $2 WHERE id = $1")
            .bind(profile.id)
            .bind(runtime_path_text(&desired_profile)?)
            .execute(&mut *transaction)
            .await?;
        profile_paths_repaired += 1;
    }

    let repositories = sqlx::query_as::<_, LegacyRepositoryPath>(
        "SELECT id, project_id, owner, name, mirror_path FROM repositories",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let mut repository_paths_repaired = 0;
    for repository in repositories {
        let stored = PathBuf::from(&repository.mirror_path);
        if stored.is_absolute() {
            require_contained_exact_path(&config.data_root, &stored)?;
            continue;
        }
        if repository.owner != "Mischa-dev"
            || repository.name != "agent-workspace-m0-spike"
            || stored != legacy_mirror
        {
            tracing::error!(
                repository_id = %repository.id,
                project_id = %repository.project_id,
                "M3 migration preflight rejected an unknown relative repository path"
            );
            return Err(AppError::Conflict(
                "database_migration",
                "legacy Repository paths require operator repair",
            ));
        }
        sqlx::query("UPDATE repositories SET mirror_path = $2 WHERE id = $1")
            .bind(repository.id)
            .bind(runtime_path_text(&desired_mirror)?)
            .execute(&mut *transaction)
            .await?;
        repository_paths_repaired += 1;
    }

    let workers_disabled = sqlx::query(
        r#"
        UPDATE execution_workers worker
           SET enabled = false,
               updated_at = now()
          FROM provider_connections connection,
               execution_profiles profile,
               project_memberships membership,
               actors actor
         WHERE worker.provider_connection_id = connection.id
           AND worker.execution_profile_id = profile.id
           AND worker.project_id = connection.project_id
           AND worker.project_id = profile.project_id
           AND membership.project_id = worker.project_id
           AND membership.actor_id = worker.actor_id
           AND actor.id = worker.actor_id
           AND worker.enabled
           AND (
               NOT connection.enabled
               OR profile.status <> 'available'
               OR NOT membership.enabled
               OR membership.role <> 'worker'
               OR actor.status <> 'active'
               OR actor.kind <> 'execution_worker'
           )
        "#,
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    transaction.commit().await?;
    Ok(M3UpgradePreflight {
        required: true,
        profile_paths_repaired,
        repository_paths_repaired,
        workers_disabled,
    })
}

async fn m3_preflight_required(pool: &PgPool) -> Result<bool, AppError> {
    let migrations_table_exists =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !migrations_table_exists {
        return Ok(false);
    }
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
                   SELECT 1 FROM _sqlx_migrations
                    WHERE version = 8 AND success
               )
           AND NOT EXISTS (
                   SELECT 1 FROM _sqlx_migrations
                    WHERE version = 9 AND success
               )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::from)
}

fn require_contained_exact_path(data_root: &Path, stored: &Path) -> Result<(), AppError> {
    let normalized = resolve_path_within_data_root(data_root, stored)?;
    if normalized != stored {
        return Err(AppError::Conflict(
            "database_migration",
            "legacy runtime paths require operator repair",
        ));
    }
    Ok(())
}

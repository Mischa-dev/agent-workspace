use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    config::AppConfig,
    error::AppError,
    git_runtime::{RegisteredRepository, adopt_legacy_mirror, expected_repository_mirror_path},
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct M3RepositoryUpgradePreflight {
    pub required: bool,
    pub mirrors_adopted: u64,
}

#[derive(Debug, FromRow)]
struct RepositoryMirrorUpgrade {
    id: Uuid,
    clone_url: String,
    default_branch: String,
    mirror_path: String,
}

pub async fn prepare_m3_repository_upgrade(
    pool: &PgPool,
    config: &AppConfig,
) -> Result<M3RepositoryUpgradePreflight, AppError> {
    if !migration_pending(pool, 8, 11).await? {
        return Ok(M3RepositoryUpgradePreflight::default());
    }
    let repositories = sqlx::query_as::<_, RepositoryMirrorUpgrade>(
        "SELECT id, clone_url, default_branch, mirror_path FROM repositories",
    )
    .fetch_all(pool)
    .await?;
    let mut upgrades = Vec::new();
    for repository in repositories {
        let canonical = expected_repository_mirror_path(&config.data_root, repository.id)?;
        let legacy = PathBuf::from(&repository.mirror_path);
        if legacy == canonical {
            continue;
        }
        let registration = RegisteredRepository {
            id: repository.id,
            clone_url: repository.clone_url,
            default_branch: repository.default_branch,
            mirror_path: canonical.clone(),
        };
        let data_root = config.data_root.clone();
        let legacy_for_adoption = legacy.clone();
        tokio::task::spawn_blocking(move || {
            adopt_legacy_mirror(&data_root, &registration, &legacy_for_adoption)
        })
        .await
        .map_err(|_| AppError::Internal("join legacy mirror adoption"))??;
        upgrades.push((repository.id, legacy, canonical));
    }
    if upgrades.is_empty() {
        return Ok(M3RepositoryUpgradePreflight {
            required: true,
            mirrors_adopted: 0,
        });
    }
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS m3_repository_path_upgrades (
            repository_id UUID PRIMARY KEY,
            legacy_path TEXT NOT NULL,
            canonical_path TEXT NOT NULL
        )
        "#,
    )
    .execute(&mut *transaction)
    .await?;
    for (repository_id, legacy, canonical) in &upgrades {
        sqlx::query(
            r#"
            INSERT INTO m3_repository_path_upgrades (
                repository_id, legacy_path, canonical_path
            )
            VALUES ($1, $2, $3)
            ON CONFLICT (repository_id) DO UPDATE
            SET legacy_path = EXCLUDED.legacy_path,
                canonical_path = EXCLUDED.canonical_path
            "#,
        )
        .bind(repository_id)
        .bind(runtime_path_text(legacy)?)
        .bind(runtime_path_text(canonical)?)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(M3RepositoryUpgradePreflight {
        required: true,
        mirrors_adopted: u64::try_from(upgrades.len()).unwrap_or(u64::MAX),
    })
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

async fn migration_pending(
    pool: &PgPool,
    completed_version: i64,
    pending_version: i64,
) -> Result<bool, AppError> {
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
                    WHERE version = $1 AND success
               )
           AND NOT EXISTS (
                   SELECT 1 FROM _sqlx_migrations
                    WHERE version = $2 AND success
               )
        "#,
    )
    .bind(completed_version)
    .bind(pending_version)
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    /// Numbered migrations stop at 0054. Later ones are named by UTC
    /// timestamp (`20261020143000_settings.sql`), so branches that each add
    /// one merge in either order: sqlx orders them after 0054 and applies any
    /// version it hasn't run, whichever merged first. The preflight checks
    /// above name exact versions below 0054, so they still hold.
    #[test]
    fn migration_versions_are_unique_and_new_ones_are_timestamps() {
        const LAST_NUMBERED: i64 = 54;
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
        let mut versions = BTreeMap::new();
        for entry in std::fs::read_dir(directory).expect("migrations directory") {
            let name = entry.expect("migration entry").file_name();
            let name = name.to_str().expect("UTF-8 migration name").to_owned();
            let (version, rest) = name.split_once('_').expect("version_description.sql");
            assert!(
                rest.ends_with(".sql") && !rest.ends_with(".down.sql"),
                "{name}: migrations are forward-only .sql files"
            );
            let number: i64 = version.parse().expect("numeric migration version");
            if version.len() == 4 {
                assert!(
                    number <= LAST_NUMBERED,
                    "{name}: name new migrations by timestamp, such as 20261020143000_{rest}"
                );
            } else {
                assert_eq!(version.len(), 14, "{name}: use YYYYMMDDHHMMSS");
                let field = |at: usize| version[at..at + 2].parse::<u32>().unwrap();
                let year = number / 10_000_000_000;
                let (month, day, hour, minute, second) =
                    (field(4), field(6), field(8), field(10), field(12));
                assert!(
                    (2026..2100).contains(&year)
                        && (1..=12).contains(&month)
                        && (1..=31).contains(&day)
                        && hour < 24
                        && minute < 60
                        && second < 60,
                    "{name}: {version} is not a YYYYMMDDHHMMSS timestamp"
                );
            }
            if let Some(other) = versions.insert(number, name.clone()) {
                panic!("{name} and {other} share version {number}");
            }
        }
        assert!(
            versions.contains_key(&LAST_NUMBERED),
            "migration 0054 is missing"
        );
    }
}

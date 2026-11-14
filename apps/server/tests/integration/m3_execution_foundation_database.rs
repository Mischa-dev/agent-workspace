use std::{collections::BTreeMap, time::Duration};

use agent_workspace_server::{
    config::AppConfig, migration_preflight::prepare_m3_upgrade, seed::dev_seed,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tempfile::TempDir;
use tokio::sync::oneshot;
use uuid::Uuid;

#[tokio::test]
async fn m3_hardening_rejects_duplicate_host_runtime_paths() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };

    for duplicate_kind in ["profile", "mirror"] {
        let schema = format!(
            "m3_duplicate_{}_{}",
            duplicate_kind,
            Uuid::now_v7().simple()
        );
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&administrative_pool)
            .await
            .expect("create duplicate-path M3 schema");
        let pool = schema_pool(&database_url, &schema).await;
        apply_through_m2(&pool).await;
        let temporary_root = tempfile::tempdir().expect("create duplicate-path data root");
        let config = development_config(&database_url, &temporary_root);
        let seed = dev_seed(&pool, &config)
            .await
            .expect("seed duplicate-path fixture");
        apply_m3_adoption(&pool)
            .await
            .expect("apply frozen M3 adoption migration");

        if duplicate_kind == "profile" {
            sqlx::query(
                r#"
                INSERT INTO execution_profiles (
                    id, project_id, provider_connection_id, owner_actor_id,
                    name, codex_home_path, status
                )
                SELECT
                    $1, project_id, provider_connection_id, owner_actor_id,
                    'Duplicate CODEX_HOME', codex_home_path, 'disabled'
                FROM execution_profiles
                WHERE id = $2
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(seed.execution_profile_id)
            .execute(&pool)
            .await
            .expect("insert duplicate CODEX_HOME fixture");
        } else {
            sqlx::query(
                r#"
                INSERT INTO repositories (
                    id, project_id, owner, name, clone_url, default_branch,
                    mirror_path, enabled, trusted
                )
                SELECT
                    $1, project_id, 'duplicate-owner', 'duplicate-mirror',
                    'https://example.invalid/duplicate.git', 'main',
                    mirror_path, false, false
                FROM repositories
                WHERE id = $2
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(seed.repository_id)
            .execute(&pool)
            .await
            .expect("insert duplicate mirror fixture");
        }

        let hardening = apply_m3_hardening(&pool).await;
        assert!(
            hardening.is_err(),
            "0010 must reject duplicate {duplicate_kind} host paths"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                r#"
                SELECT count(*)
                FROM pg_constraint
                WHERE conname IN (
                    'execution_profiles_codex_home_path_unique',
                    'repositories_mirror_path_unique',
                    'agent_runs_worker_profile_fk'
                )
                  AND connamespace = current_schema()::regnamespace
                "#,
            )
            .fetch_one(&pool)
            .await
            .expect("inspect rolled-back 0010 constraints"),
            0,
            "failed 0010 path preflight must remain atomic"
        );

        pool.close().await;
        agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
            .await
            .expect("drop duplicate-path M3 schema");
    }
}

#[tokio::test]
async fn pre_m3_upgrade_repair_handles_only_the_known_relative_seed_paths() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_preflight_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create M3 preflight schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_through_m2(&pool).await;

    let current = std::env::current_dir().expect("current test directory");
    let temporary_root = tempfile::Builder::new()
        .prefix(".m3-upgrade-preflight-")
        .tempdir_in(&current)
        .expect("create relative M3 preflight root");
    let relative_data_root = temporary_root
        .path()
        .strip_prefix(&current)
        .expect("temporary root beneath current directory")
        .to_path_buf();
    let relative_codex_root = relative_data_root.join("codex-profiles");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            relative_data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            relative_codex_root.to_string_lossy().into_owned(),
        ),
    ]))
    .expect("relative legacy configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed preflight fixture");

    sqlx::query(
        "UPDATE execution_profiles SET codex_home_path = $2, status = 'unavailable' WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .bind(
        relative_codex_root
            .join("m0-retained")
            .to_string_lossy()
            .as_ref(),
    )
    .execute(&pool)
    .await
    .expect("restore pre-M3 relative Profile path");
    sqlx::query("UPDATE repositories SET mirror_path = $2 WHERE id = $1")
        .bind(seed.repository_id)
        .bind(
            relative_data_root
                .join("repositories/m0-spike/mirror.git")
                .to_string_lossy()
                .as_ref(),
        )
        .execute(&pool)
        .await
        .expect("restore pre-M3 relative Repository path");
    sqlx::raw_sql(
        r#"
        CREATE TABLE _sqlx_migrations (version BIGINT NOT NULL, success BOOLEAN NOT NULL);
        INSERT INTO _sqlx_migrations (version, success) VALUES (8, true);
        "#,
    )
    .execute(&pool)
    .await
    .expect("record legacy migration boundary");

    let repaired = prepare_m3_upgrade(&pool, &config)
        .await
        .expect("repair known legacy M0 paths");
    assert!(repaired.required);
    assert_eq!(repaired.profile_paths_repaired, 1);
    assert_eq!(repaired.repository_paths_repaired, 1);
    assert_eq!(repaired.workers_disabled, 1);
    let repaired_paths = sqlx::query_as::<_, (String, String, bool)>(
        r#"
        SELECT profile.codex_home_path, repository.mirror_path, worker.enabled
          FROM execution_profiles profile
          JOIN execution_workers worker ON worker.execution_profile_id = profile.id
          JOIN repositories repository ON repository.id = $3
         WHERE profile.id = $1 AND worker.id = $2
        "#,
    )
    .bind(seed.execution_profile_id)
    .bind(seed.execution_worker_id)
    .bind(seed.repository_id)
    .fetch_one(&pool)
    .await
    .expect("read repaired legacy paths");
    assert!(std::path::Path::new(&repaired_paths.0).is_absolute());
    assert!(std::path::Path::new(&repaired_paths.1).is_absolute());
    assert!(!repaired_paths.2);
    apply_m3_adoption(&pool)
        .await
        .expect("frozen 0009 accepts conservatively repaired legacy rows");

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop M3 preflight schema");
}

#[tokio::test]
async fn deferred_repository_run_guard_closes_the_concurrent_archive_race() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_archive_race_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create archive-race M3 schema");
    let pool = schema_pool(&database_url, &schema).await;
    // Today's schema, so the race is tested against the guard that runs now.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply every migration");
    let temporary_root = tempfile::tempdir().expect("create archive-race data root");
    let config = development_config(&database_url, &temporary_root);
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed archive-race fixture");
    prepare_seed_execution(&pool, &seed).await;

    let run_id = Uuid::now_v7();
    let mut run_transaction = pool.begin().await.expect("begin run admission transaction");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'queued',
                'concurrent archive guard', '{}'::jsonb)
        "#,
    )
    .bind(run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .execute(&mut *run_transaction)
    .await
    .expect("stage non-terminal Agent Run");

    let archive_pool = pool.clone();
    let repository_id = seed.repository_id;
    let actor_id = seed.human_actor_id;
    let (archive_started_tx, archive_started_rx) = oneshot::channel();
    let archive_task = tokio::spawn(async move {
        let mut transaction = archive_pool.begin().await?;
        let _ = archive_started_tx.send(());
        sqlx::query(
            r#"
            UPDATE repositories
            SET enabled = false,
                archived_at = now(),
                archived_by_actor_id = $2,
                version = version + 1
            WHERE id = $1
            "#,
        )
        .bind(repository_id)
        .bind(actor_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    });
    archive_started_rx
        .await
        .expect("archive transaction reached the conflicting update");
    tokio::time::sleep(Duration::from_millis(100)).await;

    run_transaction
        .commit()
        .await
        .expect("commit concurrent Agent Run first");
    let archive_result = tokio::time::timeout(Duration::from_secs(5), archive_task)
        .await
        .expect("archive transaction must not deadlock")
        .expect("join archive transaction");
    assert!(
        archive_result.is_err(),
        "concurrent archive cannot commit after a non-terminal Run admission"
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NULL AND enabled FROM repositories WHERE id = $1",
        )
        .bind(seed.repository_id)
        .fetch_one(&pool)
        .await
        .expect("inspect Repository after rejected archive")
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("inspect concurrently admitted Run"),
        "queued"
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop archive-race M3 schema");
}

async fn prepare_seed_execution(pool: &PgPool, seed: &agent_workspace_server::seed::DevSeed) {
    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'authenticated', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(pool)
    .await
    .expect("authenticate seeded Execution Profile");
    sqlx::query(
        r#"
        UPDATE repositories
        SET preparation_state = 'ready',
            prepared_at = now(),
            preparation_failure_category = NULL,
            version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(seed.repository_id)
    .execute(pool)
    .await
    .expect("prepare seeded Repository");
}

async fn apply_m3_adoption(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(include_str!(
        "../../migrations/0009_m3_execution_foundation.sql"
    ))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn apply_m3_hardening(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(include_str!(
        "../../migrations/0010_m3_execution_admission_hardening.sql"
    ))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn apply_through_m2(pool: &PgPool) {
    for migration in [
        include_str!("../../migrations/0001_foundation.sql"),
        include_str!("../../migrations/0002_walking_skeleton.sql"),
        include_str!("../../migrations/0003_m0_consistency.sql"),
        include_str!("../../migrations/0004_m1_identity_projects.sql"),
        include_str!("../../migrations/0005_m2_conversations.sql"),
        include_str!("../../migrations/0006_m2_message_mentions_projection.sql"),
        include_str!("../../migrations/0007_m2_archived_parent_navigation.sql"),
        include_str!("../../migrations/0008_m2_conversation_lock_boundaries.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(pool)
            .await
            .expect("apply migration through M2");
    }
}

fn development_config(database_url: &str, temporary_root: &TempDir) -> AppConfig {
    AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.to_owned(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            temporary_root.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            temporary_root
                .path()
                .join("codex-profiles")
                .to_string_lossy()
                .into_owned(),
        ),
    ]))
    .expect("build development M3 config")
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect to isolated M3 schema")
}

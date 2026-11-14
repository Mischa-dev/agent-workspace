use std::{collections::BTreeMap, fs, process::Command};

use agent_workspace_server::{
    config::AppConfig, git_runtime::expected_repository_mirror_path,
    migration_preflight::prepare_m3_repository_upgrade, seed::dev_seed,
};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

#[cfg(unix)]
#[tokio::test]
async fn legacy_mirror_is_copied_to_canonical_layout_without_removing_history() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_repository_upgrade_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create repository-upgrade schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_through_m2(&pool).await;

    let temporary = tempfile::tempdir().expect("temporary repository-upgrade root");
    let data_root = temporary.path().join("data");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data_root
                .join("codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            "/usr/bin/false".to_owned(),
        ),
    ]))
    .expect("repository-upgrade configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed pre-M3 repository rows");

    let source = temporary.path().join("source");
    fs::create_dir(&source).expect("create repository-upgrade source");
    git(&source, ["init", "-b", "main"]);
    fs::write(source.join("known.txt"), "known\n").expect("write upgrade source");
    git(&source, ["add", "known.txt"]);
    git(
        &source,
        [
            "-c",
            "user.name=Agent Workspace",
            "-c",
            "user.email=agent-workspace@example.invalid",
            "commit",
            "-m",
            "baseline",
        ],
    );
    let legacy = data_root.join("repositories/m0-spike/mirror.git");
    fs::create_dir_all(legacy.parent().expect("legacy mirror parent"))
        .expect("create legacy mirror parent");
    let clone = Command::new("/usr/bin/git")
        .args([
            "clone",
            "--mirror",
            "--no-local",
            source.to_string_lossy().as_ref(),
            legacy.to_string_lossy().as_ref(),
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("clone legacy mirror");
    assert!(clone.status.success());
    let set_origin = Command::new("/usr/bin/git")
        .args([
            "--git-dir",
            legacy.to_string_lossy().as_ref(),
            "remote",
            "set-url",
            "origin",
            agent_workspace_server::seed::DEMO_CLONE_URL,
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("set legacy mirror origin");
    assert!(set_origin.status.success());
    sqlx::query("UPDATE repositories SET mirror_path = $2 WHERE id = $1")
        .bind(seed.repository_id)
        .bind(legacy.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .expect("set pre-M3 legacy mirror path");

    for migration in [
        include_str!("../../migrations/0009_m3_execution_foundation.sql"),
        include_str!("../../migrations/0010_m3_execution_admission_hardening.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(&pool)
            .await
            .expect("apply M3 migration before canonical mirrors");
    }
    sqlx::raw_sql(
        r#"
        CREATE TABLE _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        );
        INSERT INTO _sqlx_migrations (
            version, description, success, checksum, execution_time
        ) VALUES
            (8, 'M2 lock boundaries', true, ''::bytea, 0),
            (10, 'M3 hardening', true, ''::bytea, 0);
        "#,
    )
    .execute(&pool)
    .await
    .expect("record migration 10 for preflight");

    let preflight = prepare_m3_repository_upgrade(&pool, &config)
        .await
        .expect("prepare canonical mirror upgrade");
    assert!(preflight.required);
    assert_eq!(preflight.mirrors_adopted, 1);
    sqlx::raw_sql(include_str!(
        "../../migrations/0011_m3_canonical_repository_mirrors.sql"
    ))
    .execute(&pool)
    .await
    .expect("apply canonical mirror migration");

    let canonical = expected_repository_mirror_path(&data_root, seed.repository_id)
        .expect("canonical repository path");
    let stored = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT mirror_path, legacy_mirror_path FROM repositories WHERE id = $1",
    )
    .bind(seed.repository_id)
    .fetch_one(&pool)
    .await
    .expect("read adopted repository paths");
    assert_eq!(stored.0, canonical.to_string_lossy());
    assert_eq!(stored.1.as_deref(), Some(legacy.to_string_lossy().as_ref()));
    assert!(canonical.is_dir());
    assert!(legacy.is_dir(), "legacy history must remain in place");

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop repository-upgrade schema");
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

fn git<const N: usize>(repository: &std::path::Path, arguments: [&str; N]) {
    let output = Command::new("/usr/bin/git")
        .current_dir(repository)
        .args(arguments)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("run repository-upgrade Git fixture");
    assert!(output.status.success());
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
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
        .expect("connect schema-scoped repository-upgrade pool")
}

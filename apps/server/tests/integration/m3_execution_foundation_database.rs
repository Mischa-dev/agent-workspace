use std::{collections::BTreeMap, time::Duration};

use agent_workspace_server::{
    config::AppConfig, migration_preflight::prepare_m3_upgrade, seed::dev_seed,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tempfile::TempDir;
use tokio::sync::oneshot;
use uuid::Uuid;

#[tokio::test]
async fn m2_to_m3_execution_foundation_adopts_rows_and_enforces_scope() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_execution_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated M3 schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_through_m2(&pool).await;

    let temporary_root = tempfile::tempdir().expect("create temporary M3 data root");
    let config = development_config(&database_url, &temporary_root);
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M2 execution rows");

    let workspace_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repository_workspaces (
            id, repository_id, conversation_id, branch_name, base_commit, state
        )
        VALUES ($1, $2, $3, $4, $5, 'retained')
        "#,
    )
    .bind(workspace_id)
    .bind(seed.repository_id)
    .bind(seed.thread_id)
    .bind(format!("codex/run-m3-{workspace_id}"))
    .bind("0123456789012345678901234567890123456789")
    .execute(&pool)
    .await
    .expect("insert retained M2 Repository Workspace");

    let provider_session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO provider_sessions (
            id, conversation_id, execution_profile_id, provider_thread_id, state
        )
        VALUES ($1, $2, $3, $4, 'idle')
        "#,
    )
    .bind(provider_session_id)
    .bind(seed.thread_id)
    .bind(seed.execution_profile_id)
    .bind(format!("provider-thread-{provider_session_id}"))
    .execute(&pool)
    .await
    .expect("insert retained M2 Provider Session");

    let historical_run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, provider_session_id, repository_id,
            repository_workspace_id, state, prompt, authorization_snapshot,
            finished_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, 'completed',
            'preserved M2 run', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(historical_run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(seed.repository_id)
    .bind(workspace_id)
    .execute(&pool)
    .await
    .expect("insert retained M2 Agent Run");

    apply_m3_foundation(&pool)
        .await
        .expect("upgrade M2 execution rows to M3");

    let adopted = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String, i16, i64, i64)>(
        r#"
        SELECT
            workspace.project_id,
            session.project_id,
            run.project_id,
            repository.preparation_state,
            profile.account_state,
            profile.max_active_turns,
            connection.version,
            worker.version
        FROM repository_workspaces workspace
        JOIN provider_sessions session ON session.id = $2
        JOIN agent_runs run ON run.id = $3
        JOIN repositories repository ON repository.id = run.repository_id
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN provider_connections connection ON connection.id = profile.provider_connection_id
        JOIN execution_workers worker ON worker.id = run.execution_worker_id
        WHERE workspace.id = $1
        "#,
    )
    .bind(workspace_id)
    .bind(provider_session_id)
    .bind(historical_run_id)
    .fetch_one(&pool)
    .await
    .expect("read adopted M3 execution rows");
    assert_eq!(adopted.0, seed.project_id);
    assert_eq!(adopted.1, seed.project_id);
    assert_eq!(adopted.2, seed.project_id);
    assert_eq!(adopted.3, "pending");
    assert_eq!(adopted.4, "unknown");
    assert_eq!(adopted.5, 1);
    assert_eq!(adopted.6, 1);
    assert_eq!(adopted.7, 1);

    let capabilities = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT capabilities FROM provider_connections WHERE id = $1",
    )
    .bind(seed.provider_connection_id)
    .fetch_one(&pool)
    .await
    .expect("read canonical Provider capabilities");
    assert_eq!(capabilities["supports_streaming"], true);
    assert_eq!(capabilities["supports_remote_execution"], false);
    assert!(capabilities.get("streaming").is_none());

    // The development seed adopts existing M3 resources; it must not silently
    // undo an operator's disabled or failed state.
    sqlx::query(
        "UPDATE provider_connections SET enabled = false, version = version + 1 WHERE id = $1",
    )
    .bind(seed.provider_connection_id)
    .execute(&pool)
    .await
    .expect("disable Provider Connection");
    sqlx::query(
        "UPDATE execution_profiles SET status = 'disabled', account_state = 'logged_out', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("disable Execution Profile");
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.worker_actor_id)
    .execute(&pool)
    .await
    .expect("disable Worker membership");
    sqlx::query(
        "UPDATE execution_workers SET enabled = false, version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_worker_id)
    .execute(&pool)
    .await
    .expect("disable Execution Worker");
    sqlx::query(
        r#"
        UPDATE repositories
        SET enabled = false,
            trusted = false,
            preparation_state = 'failed',
            preparation_failure_category = 'identity_mismatch',
            version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("retain failed Repository registration");

    let repeated_seed = dev_seed(&pool, &config)
        .await
        .expect("idempotently adopt M3 seed resources");
    assert_eq!(repeated_seed.execution_worker_id, seed.execution_worker_id);
    assert_eq!(
        repeated_seed.execution_profile_id,
        seed.execution_profile_id
    );
    assert_eq!(repeated_seed.repository_id, seed.repository_id);
    let preserved = sqlx::query_as::<_, (bool, String, bool, bool, String)>(
        r#"
        SELECT
            connection.enabled,
            profile.account_state,
            worker.enabled,
            repository.enabled,
            repository.preparation_state
        FROM provider_connections connection
        JOIN execution_profiles profile ON profile.provider_connection_id = connection.id
        JOIN execution_workers worker ON worker.execution_profile_id = profile.id
        JOIN repositories repository ON repository.id = $4
        WHERE connection.id = $1 AND profile.id = $2 AND worker.id = $3
        "#,
    )
    .bind(seed.provider_connection_id)
    .bind(seed.execution_profile_id)
    .bind(seed.execution_worker_id)
    .bind(seed.repository_id)
    .fetch_one(&pool)
    .await
    .expect("read non-destructively adopted seed state");
    assert_eq!(
        preserved,
        (
            false,
            "logged_out".to_owned(),
            false,
            false,
            "failed".to_owned()
        )
    );

    let disabled_run = insert_run(&pool, &seed, seed.thread_id, "queued").await;
    assert!(
        disabled_run.is_err(),
        "disabled execution resources must reject new Agent Runs"
    );

    sqlx::query(
        "UPDATE provider_connections SET enabled = true, version = version + 1 WHERE id = $1",
    )
    .bind(seed.provider_connection_id)
    .execute(&pool)
    .await
    .expect("enable Provider Connection for concurrency test");
    sqlx::query(
        "UPDATE execution_profiles SET status = 'available', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("enable Execution Profile for concurrency test");
    sqlx::query(
        "UPDATE project_memberships SET enabled = true WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.worker_actor_id)
    .execute(&pool)
    .await
    .expect("enable Worker membership for concurrency test");
    sqlx::query("UPDATE execution_workers SET enabled = true, version = version + 1 WHERE id = $1")
        .bind(seed.execution_worker_id)
        .execute(&pool)
        .await
        .expect("enable Execution Worker for concurrency test");
    sqlx::query(
        "UPDATE repositories SET enabled = true, trusted = true, version = version + 1 WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("enable Repository for concurrency test");

    let unverified_run = insert_run(&pool, &seed, seed.thread_id, "queued").await;
    assert!(
        unverified_run.is_err(),
        "logged-out Profiles and failed Repository preparation must reject admission"
    );

    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'authenticated', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("authenticate Execution Profile for admission tests");
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
    .execute(&pool)
    .await
    .expect("mark Repository preparation ready");

    let wrong_owner_run = insert_run_as(
        &pool,
        &seed,
        seed.thread_id,
        seed.collaborator_actor_id,
        seed.execution_profile_id,
        "queued",
    )
    .await;
    assert!(
        wrong_owner_run.is_err(),
        "a personal Execution Profile must reject a different requesting Actor"
    );

    let alternate_profile_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id,
            name, codex_home_path, status, account_state
        )
        VALUES (
            $1, $2, $3, $4, 'M3 alternate profile', $5,
            'available', 'authenticated'
        )
        "#,
    )
    .bind(alternate_profile_id)
    .bind(seed.project_id)
    .bind(seed.provider_connection_id)
    .bind(seed.human_actor_id)
    .bind(
        temporary_root
            .path()
            .join("codex-profiles")
            .join("alternate")
            .to_string_lossy()
            .as_ref(),
    )
    .execute(&pool)
    .await
    .expect("insert alternate same-Project Profile");
    let substituted_profile_run = insert_run_as(
        &pool,
        &seed,
        seed.thread_id,
        seed.human_actor_id,
        alternate_profile_id,
        "queued",
    )
    .await;
    assert!(
        substituted_profile_run.is_err(),
        "M3 Agent Runs cannot substitute a Profile not assigned to their Worker"
    );

    let transition_thread = insert_thread(
        &pool,
        seed.project_id,
        seed.channel_id,
        seed.repository_id,
        "M3 transition revalidation",
    )
    .await;
    let transition_run_id = insert_run(&pool, &seed, transition_thread, "queued")
        .await
        .expect("admit queued run while dependencies are available");
    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'logged_out', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("log out Profile before starting transition");
    let start_while_logged_out =
        sqlx::query("UPDATE agent_runs SET state = 'starting' WHERE id = $1")
            .bind(transition_run_id)
            .execute(&pool)
            .await;
    assert!(
        start_while_logged_out.is_err(),
        "entering starting must revalidate Profile availability"
    );
    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'authenticated', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("restore Profile authentication");
    sqlx::query("UPDATE agent_runs SET state = 'starting' WHERE id = $1")
        .bind(transition_run_id)
        .execute(&pool)
        .await
        .expect("enter starting after successful revalidation");
    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'logged_out', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("log out Profile before working transition");
    let work_while_logged_out =
        sqlx::query("UPDATE agent_runs SET state = 'working' WHERE id = $1")
            .bind(transition_run_id)
            .execute(&pool)
            .await;
    assert!(
        work_while_logged_out.is_err(),
        "entering working must revalidate Profile availability"
    );
    sqlx::query("UPDATE agent_runs SET state = 'canceled', finished_at = now() WHERE id = $1")
        .bind(transition_run_id)
        .execute(&pool)
        .await
        .expect("terminalization must remain available after Profile logout");
    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'authenticated', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("restore Profile for concurrency tests");

    let second_thread = insert_thread(
        &pool,
        seed.project_id,
        seed.channel_id,
        seed.repository_id,
        "M3 queued Thread",
    )
    .await;
    let third_thread = insert_thread(
        &pool,
        seed.project_id,
        seed.channel_id,
        seed.repository_id,
        "M3 conflicting Thread",
    )
    .await;
    let working_run_id = insert_run(&pool, &seed, seed.thread_id, "working")
        .await
        .expect("insert one active provider turn");
    let queued_run_id = insert_run(&pool, &seed, second_thread, "queued")
        .await
        .expect("allow a queued run for the busy Profile");
    let competing_turn = insert_run(&pool, &seed, third_thread, "working").await;
    assert!(
        competing_turn.is_err(),
        "one Profile cannot own two active provider turns"
    );

    let archive_with_active_run = sqlx::query(
        r#"
        UPDATE repositories
        SET enabled = false,
            archived_at = now(),
            archived_by_actor_id = $2,
            version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(seed.repository_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await;
    assert!(
        archive_with_active_run.is_err(),
        "Repository archive must reject non-terminal Agent Runs"
    );

    sqlx::query("UPDATE agent_runs SET state = 'completed', finished_at = now() WHERE id = $1")
        .bind(working_run_id)
        .execute(&pool)
        .await
        .expect("complete active M3 run");
    sqlx::query("UPDATE agent_runs SET state = 'canceled', finished_at = now() WHERE id = $1")
        .bind(queued_run_id)
        .execute(&pool)
        .await
        .expect("cancel queued M3 run");
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
    .bind(seed.repository_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("archive Repository after all runs terminate");
    let rewrite_archive_attribution = sqlx::query(
        r#"
        UPDATE repositories
        SET archived_at = archived_at + interval '1 second',
            archived_by_actor_id = $2,
            version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(seed.repository_id)
    .bind(seed.collaborator_actor_id)
    .execute(&pool)
    .await;
    assert!(
        rewrite_archive_attribution.is_err(),
        "Repository archive timestamp and Actor attribution are immutable"
    );

    sqlx::query("UPDATE agent_runs SET provider_turn_id = 'turn-one' WHERE id = $1")
        .bind(historical_run_id)
        .execute(&pool)
        .await
        .expect("bind a retained provider turn exactly once");
    let replace_turn =
        sqlx::query("UPDATE agent_runs SET provider_turn_id = 'turn-two' WHERE id = $1")
            .bind(historical_run_id)
            .execute(&pool)
            .await;
    assert!(
        replace_turn.is_err(),
        "provider turn binding must be immutable"
    );

    let foreign_project_id =
        insert_foreign_project(&pool, seed.organization_id, seed.human_actor_id).await;
    let cross_project_profile = sqlx::query(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id,
            name, codex_home_path
        )
        VALUES ($1, $2, $3, $4, 'cross Project', '/tmp/cross-project-codex')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(foreign_project_id)
    .bind(seed.provider_connection_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await;
    assert!(
        cross_project_profile.is_err(),
        "Execution Profiles cannot adopt a foreign Provider Connection"
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated M3 schema");
}

#[tokio::test]
async fn m3_upgrade_rejects_ambiguous_legacy_profile_ownership() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_invalid_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated invalid M3 schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_through_m2(&pool).await;
    let temporary_root = tempfile::tempdir().expect("create invalid-upgrade data root");
    let config = development_config(&database_url, &temporary_root);
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed legacy execution rows");
    let foreign_project_id =
        insert_foreign_project(&pool, seed.organization_id, seed.human_actor_id).await;

    sqlx::query(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id,
            name, codex_home_path
        )
        VALUES ($1, $2, $3, $4, 'ambiguous legacy', '/tmp/ambiguous-legacy-codex')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(foreign_project_id)
    .bind(seed.provider_connection_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("insert M2-compatible cross-Project Profile");

    let upgrade = apply_m3_foundation(&pool).await;
    assert!(
        upgrade.is_err(),
        "M3 must reject ambiguous legacy ownership instead of rewriting it"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'execution_profiles' AND column_name = 'account_state'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect rolled-back M3 schema"),
        0,
        "failed migration must remain atomic"
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated invalid M3 schema");
}

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

async fn insert_run(
    pool: &PgPool,
    seed: &agent_workspace_server::seed::DevSeed,
    conversation_id: Uuid,
    state: &str,
) -> Result<Uuid, sqlx::Error> {
    insert_run_as(
        pool,
        seed,
        conversation_id,
        seed.human_actor_id,
        seed.execution_profile_id,
        state,
    )
    .await
}

async fn insert_run_as(
    pool: &PgPool,
    seed: &agent_workspace_server::seed::DevSeed,
    conversation_id: Uuid,
    requesting_actor_id: Uuid,
    execution_profile_id: Uuid,
    state: &str,
) -> Result<Uuid, sqlx::Error> {
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'M3 constraint test', '{}'::jsonb)
        "#,
    )
    .bind(run_id)
    .bind(conversation_id)
    .bind(seed.execution_worker_id)
    .bind(requesting_actor_id)
    .bind(execution_profile_id)
    .bind(seed.repository_id)
    .bind(state)
    .execute(pool)
    .await?;
    Ok(run_id)
}

async fn insert_thread(
    pool: &PgPool,
    project_id: Uuid,
    channel_id: Uuid,
    repository_id: Uuid,
    name: &str,
) -> Uuid {
    let thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, repository_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, $5)
        "#,
    )
    .bind(thread_id)
    .bind(project_id)
    .bind(channel_id)
    .bind(repository_id)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert M3 test Thread");
    thread_id
}

async fn insert_foreign_project(
    pool: &PgPool,
    organization_id: Uuid,
    owner_actor_id: Uuid,
) -> Uuid {
    let project_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'M3X', 'Foreign M3 Project')",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(pool)
    .await
    .expect("insert foreign Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', 'Foreign owner', 'foreign-owner')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(owner_actor_id)
    .execute(pool)
    .await
    .expect("insert foreign Project membership");
    project_id
}

async fn apply_m3_foundation(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    for migration in [
        include_str!("../../migrations/0009_m3_execution_foundation.sql"),
        include_str!("../../migrations/0010_m3_execution_admission_hardening.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(())
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

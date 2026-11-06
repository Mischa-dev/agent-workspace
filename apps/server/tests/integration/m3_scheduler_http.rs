use std::{collections::BTreeMap, fs, path::Path, process::Command, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    git_runtime::expected_repository_mirror_path,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    seed::dev_seed,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tokio::time::{Duration, Instant, sleep};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:3100";
const LOGIN_NAME: &str = "m3-scheduler-owner";
const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn queued_runs_are_idempotent_and_create_no_workspace_before_admission() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_scheduler_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create scheduler schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run scheduler migrations");

    let temporary = tempfile::tempdir().expect("temporary scheduler root");
    let data_root = temporary.path().join("data");
    let mut config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
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
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_DEMO_EXECUTOR".to_owned(),
            "true".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOST_CONCURRENCY_LIMIT".to_owned(),
            "1".to_owned(),
        ),
    ]))
    .expect("scheduler configuration");
    // Scheduler behavior is covered independently from host-capacity admission.
    // Keep this temporary fixture deterministic as CI disk usage changes.
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed scheduler fixture");
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(seed.execution_profile_id)
        .execute(&pool)
        .await
        .expect("authenticate scheduler Profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare scheduler Repository");
    let mirror = expected_repository_mirror_path(&data_root, seed.repository_id)
        .expect("canonical scheduler mirror");
    create_test_mirror(temporary.path(), &mirror);
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M3 Scheduler Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap scheduler owner");

    let second_thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, repository_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, 'Queued scheduler proof')
        "#,
    )
    .bind(second_thread_id)
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("insert second scheduler Thread");

    let supervisors = SupervisorRegistry::new(&config).expect("scheduler supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors: supervisors.clone(),
    };
    let application = router(state.clone());
    let (cookie, csrf) = login(&application).await;

    let first = start_run(
        &application,
        &cookie,
        &csrf,
        seed.thread_id,
        "first-scheduler-run",
        "hold the only host slot",
    )
    .await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(first.2["state"], "queued");
    assert!(first.2.get("worktreePath").is_none());
    assert!(first.2["repositoryWorkspaceId"].is_null());
    let first_run_id = uuid_field(&first.2, "runId");

    let replay = start_run(
        &application,
        &cookie,
        &csrf,
        seed.thread_id,
        "first-scheduler-run",
        "hold the only host slot",
    )
    .await;
    assert_eq!(replay.0, StatusCode::ACCEPTED);
    assert_eq!(uuid_field(&replay.2, "runId"), first_run_id);
    let mismatch = start_run(
        &application,
        &cookie,
        &csrf,
        seed.thread_id,
        "first-scheduler-run",
        "a conflicting retry payload",
    )
    .await;
    assert_eq!(mismatch.0, StatusCode::CONFLICT);
    assert_eq!(mismatch.2["error"]["code"], "conflict");

    wait_for_state(&pool, first_run_id, "blocked").await;
    let second = start_run(
        &application,
        &cookie,
        &csrf,
        second_thread_id,
        "second-scheduler-run",
        "remain queued without touching Git",
    )
    .await;
    assert_eq!(second.0, StatusCode::ACCEPTED);
    let second_run_id = uuid_field(&second.2, "runId");
    sleep(Duration::from_millis(250)).await;
    let (second_state, second_workspace) = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT state, repository_workspace_id FROM agent_runs WHERE id = $1",
    )
    .bind(second_run_id)
    .fetch_one(&pool)
    .await
    .expect("read queued scheduler run");
    assert_eq!(second_state, "queued");
    assert!(second_workspace.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM git_worktrees")
            .fetch_one(&pool)
            .await
            .expect("count admitted worktrees"),
        1
    );

    let alternate_repository_id = Uuid::now_v7();
    let alternate_mirror = expected_repository_mirror_path(&data_root, alternate_repository_id)
        .expect("alternate canonical mirror path");
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, enabled, trusted, preparation_state,
            prepared_at
        )
        VALUES (
            $1, $2, 9000000099, 'Mischa-dev', 'alternate-scheduler-repository',
            'https://github.com/Mischa-dev/alternate-scheduler-repository.git',
            'main', $3, true, true, 'ready', now()
        )
        "#,
    )
    .bind(alternate_repository_id)
    .bind(seed.project_id)
    .bind(alternate_mirror.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .expect("insert alternate ready Repository");
    let choice_thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES ($1, $2, 'thread', $3, 'Repository choice proof')
        "#,
    )
    .bind(choice_thread_id)
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .execute(&pool)
    .await
    .expect("insert unbound repository-choice Thread");
    let choice_required = start_run(
        &application,
        &cookie,
        &csrf,
        choice_thread_id,
        "repository-choice-required",
        "require deterministic repository selection",
    )
    .await;
    assert_eq!(choice_required.0, StatusCode::CONFLICT);
    assert_eq!(
        choice_required.2["error"]["code"],
        "repository_choice_required"
    );
    assert_eq!(
        choice_required.2["error"]["details"]["repositoryIds"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let explicit = start_run_with_repository(
        &application,
        &cookie,
        &csrf,
        choice_thread_id,
        "repository-choice-explicit",
        "use the explicit authorized repository",
        seed.repository_id,
    )
    .await;
    assert_eq!(explicit.0, StatusCode::ACCEPTED);
    assert_eq!(explicit.2["repositoryId"], seed.repository_id.to_string());
    assert!(explicit.2["repositoryWorkspaceId"].is_null());
    let explicit_run_id = uuid_field(&explicit.2, "runId");

    let linked_thread_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, parent_channel_id, name) VALUES ($1, $2, 'thread', $3, 'Linked Work Item repository proof')",
    )
    .bind(linked_thread_id)
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .execute(&pool)
    .await
    .expect("insert Work Item-linked Thread");
    let linked_work_item_id = Uuid::now_v7();
    let linked_work_item_number = sqlx::query_scalar::<_, i64>(
        "UPDATE project_counters SET next_work_item_number = next_work_item_number + 1 WHERE project_id = $1 RETURNING next_work_item_number - 1",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("allocate linked Work Item number");
    let (default_type_id, default_status_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT work_type.id, status.id
        FROM work_item_types work_type
        CROSS JOIN work_item_statuses status
        WHERE work_type.project_id = $1 AND work_type.is_default AND work_type.enabled
          AND status.project_id = $1 AND status.is_default AND status.enabled
        "#,
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("read default Work Item workflow");
    sqlx::query(
        r#"
        INSERT INTO work_items (
            id, project_id, number, title, type_id, status_id,
            created_by_actor_id, board_rank
        ) VALUES ($1, $2, $3, 'Repository precedence proof', $4, $5, $6, 1024)
        "#,
    )
    .bind(linked_work_item_id)
    .bind(seed.project_id)
    .bind(linked_work_item_number)
    .bind(default_type_id)
    .bind(default_status_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("insert repository precedence Work Item");
    sqlx::query(
        "INSERT INTO work_item_conversation_links (project_id, work_item_id, conversation_id, created_by_actor_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(seed.project_id)
    .bind(linked_work_item_id)
    .bind(linked_thread_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("link repository precedence Thread");
    sqlx::query(
        "INSERT INTO work_item_repository_links (project_id, work_item_id, repository_id, role, created_by_actor_id) VALUES ($1, $2, $3, 'primary', $4)",
    )
    .bind(seed.project_id)
    .bind(linked_work_item_id)
    .bind(seed.repository_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("link primary repository fixture");
    let implicit = start_run(
        &application,
        &cookie,
        &csrf,
        linked_thread_id,
        "repository-choice-work-item-primary",
        "use the linked Work Item primary repository",
    )
    .await;
    assert_eq!(implicit.0, StatusCode::ACCEPTED, "{}", implicit.1);
    assert_eq!(implicit.2["repositoryId"], seed.repository_id.to_string());
    let implicit_run_id = uuid_field(&implicit.2, "runId");
    sqlx::query(
        "UPDATE agent_runs SET state = 'interrupted', finished_at = now(), updated_at = now() WHERE id = $1 AND state = 'queued'",
    )
    .bind(implicit_run_id)
    .execute(&pool)
    .await
    .expect("retire implicit repository selection fixture");

    sqlx::query(
        r#"
        UPDATE agent_runs
        SET scheduler_claim_id = $2,
            scheduler_claimed_at = now() - interval '2 minutes',
            scheduler_lease_expires_at = now() - interval '1 minute'
        WHERE id = $1 AND state = 'queued'
        "#,
    )
    .bind(second_run_id)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .expect("expire a scheduler lease fixture");

    let interrupted = interrupt(&application, &cookie, &csrf, first_run_id).await;
    assert_eq!(interrupted.0, StatusCode::OK);
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("drain scheduler after releasing host capacity");
    wait_for_state(&pool, second_run_id, "blocked").await;
    let (workspace_id, branch, base_commit, worktree_path) =
        sqlx::query_as::<_, (Uuid, String, String, String)>(
            r#"
        SELECT workspace.id, workspace.branch_name, workspace.base_commit, worktree.path
        FROM agent_runs run
        JOIN repository_workspaces workspace ON workspace.id = run.repository_workspace_id
        JOIN git_worktrees worktree ON worktree.repository_workspace_id = workspace.id
        WHERE run.id = $1
        "#,
        )
        .bind(second_run_id)
        .fetch_one(&pool)
        .await
        .expect("read admitted scheduler workspace");
    assert!(branch.starts_with("codex/run-aw-"));
    assert_eq!(base_commit.len(), 40);
    assert_eq!(
        Path::new(&worktree_path)
            .canonicalize()
            .expect("canonical admitted worktree"),
        data_root
            .join("worktrees")
            .join(seed.repository_id.to_string())
            .join(workspace_id.to_string())
            .canonicalize()
            .expect("canonical expected worktree")
    );

    let question_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM agent_approvals WHERE agent_run_id = $1 AND status = 'pending'",
    )
    .bind(second_run_id)
    .fetch_one(&pool)
    .await
    .expect("read demo blocker for question response proof");
    sqlx::query(
        r#"
        UPDATE agent_approvals
           SET kind = 'question',
               summary = 'What should the proof answer be?',
               options = '[]'::jsonb
         WHERE id = $1
        "#,
    )
    .bind(question_id)
    .execute(&pool)
    .await
    .expect("convert deterministic demo blocker to a question");
    let question = respond_to_approval(
        &application,
        &cookie,
        &csrf,
        question_id,
        "question-answer-proof",
        json!({"answer": "Aurora"}),
    )
    .await;
    assert_eq!(question.0, StatusCode::OK);
    assert_eq!(question.2["kind"], "question");
    assert_eq!(question.2["response"]["answer"], "Aurora");
    let repeated_question = respond_to_approval(
        &application,
        &cookie,
        &csrf,
        question_id,
        "question-answer-proof",
        json!({"answer": "Aurora"}),
    )
    .await;
    assert_eq!(repeated_question.0, StatusCode::OK);
    wait_for_state(&pool, second_run_id, "completed").await;
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("promote explicit repository run after completion");
    wait_for_state(&pool, explicit_run_id, "blocked").await;
    assert_eq!(
        interrupt(&application, &cookie, &csrf, explicit_run_id)
            .await
            .0,
        StatusCode::OK
    );
    let second_worktree = Path::new(&worktree_path);
    git(second_worktree, ["add", "KNOWN_FILE.txt"]);
    git(
        second_worktree,
        [
            "-c",
            "user.name=Agent Workspace",
            "-c",
            "user.email=agent-workspace@example.invalid",
            "commit",
            "-m",
            "make retry fixture clean",
        ],
    );

    let retry = retry_run(
        &application,
        &cookie,
        &csrf,
        second_run_id,
        "retry-second-run",
    )
    .await;
    assert_eq!(retry.0, StatusCode::ACCEPTED);
    let retry_run_id = uuid_field(&retry.2, "runId");
    assert_ne!(retry_run_id, second_run_id);
    let retry_replay = retry_run(
        &application,
        &cookie,
        &csrf,
        second_run_id,
        "retry-second-run",
    )
    .await;
    assert_eq!(retry_replay.0, StatusCode::ACCEPTED);
    assert_eq!(uuid_field(&retry_replay.2, "runId"), retry_run_id);
    wait_for_state(&pool, retry_run_id, "blocked").await;
    let (predecessor, retry_workspace_id, preserved_dirty) =
        sqlx::query_as::<_, (Option<Uuid>, Uuid, bool)>(
            r#"
            SELECT retry.predecessor_run_id, retry.repository_workspace_id, old_tree.dirty
              FROM agent_runs retry
              JOIN agent_runs old ON old.id = retry.predecessor_run_id
              JOIN git_worktrees old_tree
                ON old_tree.repository_workspace_id = old.repository_workspace_id
             WHERE retry.id = $1
            "#,
        )
        .bind(retry_run_id)
        .fetch_one(&pool)
        .await
        .expect("read explicit retry ownership");
    assert_eq!(predecessor, Some(second_run_id));
    assert_eq!(retry_workspace_id, workspace_id);
    assert!(
        !preserved_dirty,
        "only a clean owned worktree may be reused"
    );
    assert_eq!(
        interrupt(&application, &cookie, &csrf, retry_run_id)
            .await
            .0,
        StatusCode::OK
    );

    // Retention cleanup and a retry racing for the same retained worktree:
    // the cleanup holds the repository's retention lock and has removed the
    // workspace (uncommitted) when the retry inspects it. The retry waits for
    // the lock, sees the workspace is gone, and fails instead of reviving it.
    let repository_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT repository_id FROM repository_workspaces WHERE id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&pool)
    .await
    .expect("read retained workspace repository");
    let mut cleanup = pool.begin().await.expect("begin cleanup");
    agent_workspace_server::retention::lock_repository_retention(&mut cleanup, repository_id)
        .await
        .expect("cleanup takes the retention lock");
    sqlx::query("UPDATE repository_workspaces SET state = 'cleaned' WHERE id = $1")
        .bind(workspace_id)
        .execute(&mut *cleanup)
        .await
        .expect("cleanup marks the workspace cleaned");
    sqlx::query("UPDATE git_worktrees SET state = 'removed' WHERE repository_workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *cleanup)
        .await
        .expect("cleanup marks the worktree removed");
    let raced = retry_run(
        &application,
        &cookie,
        &csrf,
        retry_run_id,
        "retry-during-cleanup",
    )
    .await;
    assert_eq!(raced.0, StatusCode::ACCEPTED);
    let raced_id = uuid_field(&raced.2, "runId");
    let mut waited = 0;
    while sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted)",
    )
    .fetch_one(&pool)
    .await
    .expect("read lock waits")
    {
        waited += 1;
        assert!(
            waited < 200,
            "the retry never waited for the retention lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    cleanup.commit().await.expect("cleanup commits");
    wait_for_state(&pool, raced_id, "failed").await;
    let (raced_workspace, workspace_state, worktree_state) =
        sqlx::query_as::<_, (Option<Uuid>, String, String)>(
            r#"
            SELECT run.repository_workspace_id, workspace.state, worktree.state
              FROM agent_runs run, repository_workspaces workspace
              JOIN git_worktrees worktree ON worktree.repository_workspace_id = workspace.id
             WHERE run.id = $1 AND workspace.id = $2
            "#,
        )
        .bind(raced_id)
        .bind(workspace_id)
        .fetch_one(&pool)
        .await
        .expect("read raced retry");
    assert!(
        raced_workspace.is_none(),
        "the retry did not claim a cleaned workspace"
    );
    assert_eq!(
        (workspace_state.as_str(), worktree_state.as_str()),
        ("cleaned", "removed")
    );
    // Put the fixture back (the files were never removed) for the next case.
    sqlx::query("UPDATE repository_workspaces SET state = 'retained' WHERE id = $1")
        .bind(workspace_id)
        .execute(&pool)
        .await
        .expect("restore the workspace");
    sqlx::query("UPDATE git_worktrees SET state = 'ready' WHERE repository_workspace_id = $1")
        .bind(workspace_id)
        .execute(&pool)
        .await
        .expect("restore the worktree");

    fs::write(second_worktree.join("UNCOMMITTED.txt"), "preserve me\n")
        .expect("dirty the retained retry worktree");
    let unsafe_retry = retry_run(
        &application,
        &cookie,
        &csrf,
        retry_run_id,
        "unsafe-dirty-retry",
    )
    .await;
    assert_eq!(unsafe_retry.0, StatusCode::ACCEPTED);
    let unsafe_retry_id = uuid_field(&unsafe_retry.2, "runId");
    wait_for_state(&pool, unsafe_retry_id, "failed").await;
    let (unsafe_workspace, unsafe_reason, dirty_preserved) =
        sqlx::query_as::<_, (Option<Uuid>, Option<String>, bool)>(
            r#"
            SELECT failed.repository_workspace_id, failed.failure_reason, worktree.dirty
              FROM agent_runs failed
              JOIN agent_runs predecessor ON predecessor.id = failed.predecessor_run_id
              JOIN git_worktrees worktree
                ON worktree.repository_workspace_id = predecessor.repository_workspace_id
             WHERE failed.id = $1
            "#,
        )
        .bind(unsafe_retry_id)
        .fetch_one(&pool)
        .await
        .expect("read unsafe retry preservation result");
    assert!(unsafe_workspace.is_none());
    assert!(unsafe_reason.is_some_and(|reason| reason.contains("dirty, missing")));
    assert!(dirty_preserved);
    assert!(second_worktree.join("UNCOMMITTED.txt").is_file());
    let failure_event_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM agent_run_events WHERE agent_run_id = $1 AND kind = 'run_failed'",
    )
    .bind(unsafe_retry_id)
    .fetch_one(&pool)
    .await
    .expect("read durable unsafe retry failure event");
    let acknowledgement = acknowledge_failure(
        &application,
        &cookie,
        &csrf,
        unsafe_retry_id,
        failure_event_id,
    )
    .await;
    assert_eq!(acknowledgement.0, StatusCode::OK);
    assert_eq!(
        acknowledge_failure(
            &application,
            &cookie,
            &csrf,
            unsafe_retry_id,
            failure_event_id,
        )
        .await
        .0,
        StatusCode::OK
    );
    let acknowledgement_actors = sqlx::query_scalar::<_, Vec<Uuid>>(
        "SELECT array_agg(actor_id ORDER BY actor_id) FROM agent_run_acknowledgements WHERE agent_run_id = $1",
    )
    .bind(unsafe_retry_id)
    .fetch_one(&pool)
    .await
    .expect("read per-Actor failure acknowledgement");
    assert_eq!(acknowledgement_actors, vec![seed.human_actor_id]);
    let reloaded_timeline = response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/conversations/{second_thread_id}/timeline"))
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .expect("build reloaded timeline request"),
            )
            .await
            .expect("reloaded timeline response"),
    )
    .await;
    assert_eq!(reloaded_timeline.0, StatusCode::OK);
    assert_eq!(
        reloaded_timeline.2["latestTerminalRun"]["id"],
        unsafe_retry_id.to_string()
    );
    assert_eq!(
        reloaded_timeline.2["latestTerminalRun"]["failureAcknowledged"], true,
        "the current Actor's acknowledgement must survive timeline refetch"
    );

    let pre_workspace_retry = retry_run(
        &application,
        &cookie,
        &csrf,
        unsafe_retry_id,
        "retry-pre-workspace-failure",
    )
    .await;
    assert_eq!(pre_workspace_retry.0, StatusCode::ACCEPTED);
    let pre_workspace_retry_id = uuid_field(&pre_workspace_retry.2, "runId");
    wait_for_state(&pool, pre_workspace_retry_id, "blocked").await;
    let allocated_workspace = sqlx::query_scalar::<_, Uuid>(
        "SELECT repository_workspace_id FROM agent_runs WHERE id = $1",
    )
    .bind(pre_workspace_retry_id)
    .fetch_one(&pool)
    .await
    .expect("a retry whose predecessor has no workspace receives a fresh workspace");
    assert_ne!(allocated_workspace, workspace_id);
    assert!(
        second_worktree.join("UNCOMMITTED.txt").is_file(),
        "the earlier dirty worktree remains preserved"
    );
    assert_eq!(
        interrupt(&application, &cookie, &csrf, pre_workspace_retry_id)
            .await
            .0,
        StatusCode::OK
    );

    wait_for_claim_release(&pool, second_run_id).await;
    supervisors.shutdown_all().await;
    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop scheduler schema");
}

/// A scheduler with one host slot, for the AWD-74 admission tests: the
/// first test's setup, kept in one place.
struct AdmissionFixture {
    pool: PgPool,
    administrative_pool: PgPool,
    schema: String,
    state: AppState,
    supervisors: SupervisorRegistry,
    application: axum::Router,
    cookie: String,
    csrf: String,
    seed: agent_workspace_server::seed::DevSeed,
    second_thread_id: Uuid,
    _temporary: tempfile::TempDir,
}

async fn admission_fixture(name: &str) -> Option<AdmissionFixture> {
    let (database_url, administrative_pool) = integration_pool().await?;
    let schema = format!("m3_{name}_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create admission schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run admission migrations");
    let temporary = tempfile::tempdir().expect("temporary admission root");
    let data_root = temporary.path().join("data");
    let mut config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
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
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_DEMO_EXECUTOR".to_owned(),
            "true".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOST_CONCURRENCY_LIMIT".to_owned(),
            "1".to_owned(),
        ),
    ]))
    .expect("admission configuration");
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed admission fixture");
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(seed.execution_profile_id)
        .execute(&pool)
        .await
        .expect("authenticate admission Profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare admission Repository");
    let mirror = expected_repository_mirror_path(&data_root, seed.repository_id)
        .expect("canonical admission mirror");
    create_test_mirror(temporary.path(), &mirror);
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M3 Scheduler Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap admission owner");
    let second_thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, repository_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, 'Queued admission proof')
        "#,
    )
    .bind(second_thread_id)
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("insert second admission Thread");
    let supervisors = SupervisorRegistry::new(&config).expect("admission supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors: supervisors.clone(),
    };
    let application = router(state.clone());
    let (cookie, csrf) = login(&application).await;
    Some(AdmissionFixture {
        pool,
        administrative_pool,
        schema,
        state,
        supervisors,
        application,
        cookie,
        csrf,
        seed,
        second_thread_id,
        _temporary: temporary,
    })
}

impl AdmissionFixture {
    /// A second run, started by the owner in the browser, waiting in the
    /// queue while the first holds the only host slot; then the slot is
    /// freed with the run still held (a live claim), so nothing takes it
    /// before the test changes who may start it. Returns the waiting run.
    async fn run_waiting_for_a_slot(&self) -> Uuid {
        let first = start_run(
            &self.application,
            &self.cookie,
            &self.csrf,
            self.seed.thread_id,
            "admission-first-run",
            "hold the only host slot",
        )
        .await;
        assert_eq!(first.0, StatusCode::ACCEPTED, "{}", first.1);
        let first_run_id = uuid_field(&first.2, "runId");
        wait_for_state(&self.pool, first_run_id, "blocked").await;
        let second = start_run(
            &self.application,
            &self.cookie,
            &self.csrf,
            self.second_thread_id,
            "admission-second-run",
            "wait for a slot",
        )
        .await;
        assert_eq!(second.0, StatusCode::ACCEPTED, "{}", second.1);
        let second_run_id = uuid_field(&second.2, "runId");
        sleep(Duration::from_millis(250)).await;
        wait_for_state(&self.pool, second_run_id, "queued").await;
        // A live claim: all three claim columns set, the lease after the
        // claim (agent_runs_scheduler_claim_shape_check).
        sqlx::query(
            r#"
            UPDATE agent_runs
            SET scheduler_claim_id = $2,
                scheduler_claimed_at = now(),
                scheduler_lease_expires_at = now() + interval '1 hour'
            WHERE id = $1 AND state = 'queued'
            "#,
        )
        .bind(second_run_id)
        .bind(Uuid::now_v7())
        .execute(&self.pool)
        .await
        .expect("hold the queued run");
        let interrupted =
            interrupt(&self.application, &self.cookie, &self.csrf, first_run_id).await;
        assert_eq!(interrupted.0, StatusCode::OK, "{}", interrupted.1);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let active = sqlx::query_scalar::<_, bool>(
                "SELECT state IN ('starting', 'working', 'blocked', 'recovering') FROM agent_runs WHERE id = $1",
            )
            .bind(first_run_id)
            .fetch_one(&self.pool)
            .await
            .expect("read first run");
            if !active {
                break;
            }
            assert!(Instant::now() < deadline, "the first run kept its slot");
            sleep(Duration::from_millis(25)).await;
        }
        second_run_id
    }

    /// Lets the held run's claim lapse the way the first test does (claimed
    /// two minutes ago, lease over one minute ago, so the lease still
    /// follows the claim), drains the queue, and returns the run's state and
    /// workspace.
    async fn release_and_drain(&self, run_id: Uuid) -> (String, Option<Uuid>) {
        self.lapse_claim(run_id).await;
        agent_workspace_server::scheduler::drain_now(&self.state)
            .await
            .expect("drain the queue");
        sqlx::query_as::<_, (String, Option<Uuid>)>(
            "SELECT state, repository_workspace_id FROM agent_runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await
        .expect("read the queued run")
    }

    /// Lets the held run's claim lapse: claimed two minutes ago, lease over a
    /// minute ago, so the lease still follows the claim. Only the claim
    /// columns change, which the admission trigger doesn't watch.
    async fn lapse_claim(&self, run_id: Uuid) {
        sqlx::query(
            r#"
            UPDATE agent_runs
            SET scheduler_claimed_at = now() - interval '2 minutes',
                scheduler_lease_expires_at = now() - interval '1 minute'
            WHERE id = $1 AND state = 'queued'
            "#,
        )
        .bind(run_id)
        .execute(&self.pool)
        .await
        .expect("let the held claim lapse");
    }

    async fn close(self) {
        self.supervisors.shutdown_all().await;
        self.pool.close().await;
        agent_workspace_server::test_support::drop_schema(&self.administrative_pool, &self.schema)
            .await
            .expect("drop admission schema");
    }
}

/// AWD-74: a run waiting in the admission queue starts only if whoever
/// started it may still start runs there when a slot frees up. Someone
/// whose role lost "start runs" while it waited has it cancelled, with no
/// workspace made for it.
#[tokio::test]
async fn a_queued_run_is_cancelled_when_its_requester_loses_start_runs() {
    let Some(fixture) = admission_fixture("admission_role").await else {
        return;
    };
    let run_id = fixture.run_waiting_for_a_slot().await;
    let no_runs = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO workspace_roles (id, organization_id, key, base_role, name, permissions)
        VALUES (gen_random_uuid(), $1, 'no-runs', 'member', 'No runs',
                ARRAY['projects.view', 'tasks.view', 'channels.post'])
        RETURNING id
        "#,
    )
    .bind(fixture.seed.organization_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("role without start runs");
    sqlx::query(
        "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(fixture.seed.project_id)
    .bind(fixture.seed.human_actor_id)
    .bind(no_runs)
    .execute(&fixture.pool)
    .await
    .expect("narrow the requester's role");
    let (state, workspace) = fixture.release_and_drain(run_id).await;
    assert_eq!(state, "canceled");
    assert!(workspace.is_none());
    fixture.close().await;
}

/// AWD-74: claiming a queued run never waits on its thread while holding
/// the run. Removing a message holds the thread and then redacts the run's
/// prompt; starting a run holds the run and then (the admission trigger)
/// takes the thread. With the thread held, a claim pass skips the run and
/// returns; once the thread is free, the next pass claims it.
#[tokio::test]
async fn a_claim_skips_a_run_whose_thread_is_locked() {
    let Some(fixture) = admission_fixture("admission_lock_order").await else {
        return;
    };
    let run_id = fixture.run_waiting_for_a_slot().await;
    fixture.lapse_claim(run_id).await;

    // Stands in for a message removal holding the run's thread.
    let mut holder = fixture
        .pool
        .begin()
        .await
        .expect("begin holding the thread");
    sqlx::query("SELECT id FROM conversations WHERE id = $1 FOR UPDATE")
        .bind(fixture.second_thread_id)
        .fetch_one(&mut *holder)
        .await
        .expect("hold the thread");
    tokio::time::timeout(
        Duration::from_secs(5),
        agent_workspace_server::scheduler::drain_now(&fixture.state),
    )
    .await
    .expect("the claim waited on the locked thread")
    .expect("drain the queue");
    let (state, claim) = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT state, scheduler_claim_id FROM agent_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("read the waiting run");
    assert_eq!(state, "queued");
    assert!(
        claim.is_none(),
        "the run was claimed with its thread locked"
    );

    // The thread free again: the next pass claims and starts it.
    holder.rollback().await.expect("free the thread");
    agent_workspace_server::scheduler::drain_now(&fixture.state)
        .await
        .expect("drain the queue");
    let state = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("read the started run");
    assert!(
        !matches!(state.as_str(), "queued" | "canceled"),
        "the run didn't start once its thread was free: {state}"
    );
    fixture.close().await;
}

/// AWD-74: a run a person's app started (directly, or for a launch the app
/// queued, which records the same origin on the run) starts only within
/// the app's agent limit as it is when a slot frees up. The person's role
/// still allows it; their apps' limit no longer does, so it is cancelled.
#[tokio::test]
async fn a_queued_app_run_is_cancelled_when_the_app_loses_start_runs() {
    let Some(fixture) = admission_fixture("admission_app").await else {
        return;
    };
    let run_id = fixture.run_waiting_for_a_slot().await;
    // Started through the person's app (what start_run_in records inside an
    // app's call, or for a launch the app queued).
    sqlx::query("UPDATE agent_runs SET made_through_app = true WHERE id = $1")
        .bind(run_id)
        .execute(&fixture.pool)
        .await
        .expect("mark the run as the app's");
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['tasks.view', 'tasks.edit'] WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(fixture.seed.organization_id)
    .bind(fixture.seed.human_actor_id)
    .execute(&fixture.pool)
    .await
    .expect("the person's apps no longer start runs");
    let (state, workspace) = fixture.release_and_drain(run_id).await;
    assert_eq!(state, "canceled");
    assert!(workspace.is_none());
    fixture.close().await;
}

async fn start_run(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    thread_id: Uuid,
    idempotency_key: &str,
    prompt: &str,
) -> (StatusCode, String, Value) {
    start_run_body(
        application,
        cookie,
        csrf,
        thread_id,
        idempotency_key,
        json!({"prompt": prompt}),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn start_run_with_repository(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    thread_id: Uuid,
    idempotency_key: &str,
    prompt: &str,
    repository_id: Uuid,
) -> (StatusCode, String, Value) {
    start_run_body(
        application,
        cookie,
        csrf,
        thread_id,
        idempotency_key,
        json!({"prompt": prompt, "repositoryId": repository_id}),
    )
    .await
}

pub(super) async fn start_run_body(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    thread_id: Uuid,
    idempotency_key: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/threads/{thread_id}/agent-runs"))
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header("idempotency-key", idempotency_key)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&body).expect("serialize scheduler start"),
                    ))
                    .expect("build scheduler start"),
            )
            .await
            .expect("scheduler start response"),
    )
    .await
}

pub(super) async fn interrupt(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    run_id: Uuid,
) -> (StatusCode, String, Value) {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/agent-runs/{run_id}/interrupt"))
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header("idempotency-key", format!("interrupt-{run_id}"))
                    .body(Body::empty())
                    .expect("build scheduler interrupt"),
            )
            .await
            .expect("scheduler interrupt response"),
    )
    .await
}

async fn respond_to_approval(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    approval_id: Uuid,
    idempotency_key: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/agent-approvals/{approval_id}/respond"))
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header("idempotency-key", idempotency_key)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&body).expect("serialize blocker response"),
                    ))
                    .expect("build blocker response"),
            )
            .await
            .expect("blocker response"),
    )
    .await
}

async fn retry_run(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    run_id: Uuid,
    idempotency_key: &str,
) -> (StatusCode, String, Value) {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/agent-runs/{run_id}/retry"))
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header("idempotency-key", idempotency_key)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .expect("build retry request"),
            )
            .await
            .expect("retry response"),
    )
    .await
}

async fn acknowledge_failure(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    run_id: Uuid,
    event_id: Uuid,
) -> (StatusCode, String, Value) {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/agent-runs/{run_id}/acknowledge-failure"))
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header(
                        "idempotency-key",
                        format!("acknowledge-{run_id}-{event_id}"),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"eventId": event_id}))
                            .expect("serialize failure acknowledgement"),
                    ))
                    .expect("build failure acknowledgement"),
            )
            .await
            .expect("failure acknowledgement response"),
    )
    .await
}

pub(super) async fn wait_for_state(pool: &PgPool, run_id: Uuid, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .expect("read scheduler run state");
        if state == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "run did not reach {expected}; last state was {state}"
        );
        sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_claim_release(pool: &PgPool, run_id: Uuid) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let released = sqlx::query_scalar::<_, bool>(
            "SELECT scheduler_claim_id IS NULL FROM agent_runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("read scheduler claim");
        if released {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "scheduler claim was not released"
        );
        sleep(Duration::from_millis(25)).await;
    }
}

pub(super) fn uuid_field(value: &Value, field: &str) -> Uuid {
    value[field]
        .as_str()
        .expect("UUID response field")
        .parse()
        .expect("valid UUID response field")
}

pub(super) fn create_test_mirror(runtime_root: &Path, mirror_path: &Path) {
    let source = runtime_root.join("source");
    fs::create_dir(&source).expect("create scheduler source");
    git(&source, ["init", "-b", "main"]);
    fs::write(source.join("KNOWN_FILE.txt"), "known\n").expect("write scheduler source");
    git(&source, ["add", "KNOWN_FILE.txt"]);
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
    fs::create_dir_all(mirror_path.parent().expect("scheduler mirror parent"))
        .expect("create scheduler mirror parent");
    let output = Command::new("/usr/bin/git")
        .args([
            "clone",
            "--mirror",
            "--no-local",
            source.to_string_lossy().as_ref(),
            mirror_path.to_string_lossy().as_ref(),
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("clone scheduler mirror");
    assert!(output.status.success());
    let output = Command::new("/usr/bin/git")
        .args([
            "--git-dir",
            mirror_path.to_string_lossy().as_ref(),
            "remote",
            "set-url",
            "origin",
            "https://github.com/Mischa-dev/agent-workspace-m0-spike.git",
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("set scheduler mirror origin");
    assert!(output.status.success());
}

fn git<const N: usize>(repository: &Path, arguments: [&str; N]) {
    let output = Command::new("/usr/bin/git")
        .current_dir(repository)
        .args(arguments)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("run scheduler Git fixture");
    assert!(output.status.success());
}

async fn login(application: &axum::Router) -> (String, String) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "loginName": LOGIN_NAME,
                        "password": PASSWORD
                    }))
                    .expect("serialize scheduler login"),
                ))
                .expect("build scheduler login"),
        )
        .await
        .expect("scheduler login response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let cookie = cookies.join("; ");
    let csrf = cookies
        .iter()
        .find_map(|value| value.strip_prefix("agent_workspace_csrf="))
        .expect("scheduler CSRF cookie")
        .to_owned();
    (cookie, csrf)
}

pub(super) async fn response_json(
    response: axum::response::Response,
) -> (StatusCode, String, Value) {
    let status = response.status();
    let text = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("read scheduler response")
            .to_vec(),
    )
    .expect("scheduler response UTF-8");
    let value = serde_json::from_str(&text).expect("scheduler response JSON");
    (status, text, value)
}

pub(super) async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect scheduler PostgreSQL");
    Some((database_url, pool))
}

pub(super) async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(12)
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
        .expect("connect scheduler schema pool")
}

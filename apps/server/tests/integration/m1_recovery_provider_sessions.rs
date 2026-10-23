use std::{
    collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, process::Command,
    sync::Arc,
};

use agent_workspace_server::{
    AppState, config::AppConfig, rate_limit::RateLimiters, realtime::RealtimeHub, recovery,
    seed::dev_seed,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
async fn startup_recovery_idles_the_provider_session_after_interrupting_its_run() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m1_recovery_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated recovery schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let temporary_root = tempfile::tempdir().expect("temporary recovery root");
    let fake_codex = temporary_root.path().join("fake-codex");
    fs::write(
        &fake_codex,
        r#"#!/usr/bin/env bash
set -euo pipefail
while IFS= read -r line; do
  if [[ "$line" != *'"id"'* ]]; then
    continue
  fi
  request_id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  if [[ "$line" == *'"method":"initialize"'* ]]; then
    printf '{"id":%s,"result":{"server":"fake"}}\n' "$request_id"
  elif [[ "$line" == *'"method":"thread/read"'* ]]; then
    printf '{"id":%s,"result":{"thread":{"turns":[{"id":"turn-recovered","status":"completed","items":[{"type":"agentMessage","text":"Recovered proof"}]}]}}}\n' "$request_id"
  else
    printf '{"id":%s,"result":{}}\n' "$request_id"
  fi
done
"#,
    )
    .expect("write recovery fake Codex binary");
    fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700))
        .expect("make recovery fake Codex executable");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            temporary_root.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            temporary_root
                .path()
                .join("codex")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            fake_codex.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS".to_owned(),
            "2".to_owned(),
        ),
    ]))
    .expect("development config");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed recovery fixture");
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(seed.execution_profile_id)
        .execute(&pool)
        .await
        .expect("authenticate recovery Profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare recovery Repository");
    let provider_session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO provider_sessions (
            id, conversation_id, execution_profile_id, provider_thread_id, state
        )
        VALUES ($1, $2, $3, $4, 'active')
        "#,
    )
    .bind(provider_session_id)
    .bind(seed.thread_id)
    .bind(seed.execution_profile_id)
    .bind(format!("recovery-thread-{provider_session_id}"))
    .execute(&pool)
    .await
    .expect("insert active provider Session");
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, provider_session_id, repository_id, state,
            prompt, authorization_snapshot, started_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, 'working',
            'recovery provider-session test', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("insert non-terminal Agent Run");

    let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
        .expect("test supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
        supervisors,
    };
    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("reconcile Agent Run and provider Session"),
        1
    );

    let run_state = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read recovered Agent Run");
    let provider_session_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM provider_sessions WHERE id = $1")
            .bind(provider_session_id)
            .fetch_one(&pool)
            .await
            .expect("read reconciled provider Session");
    assert_eq!(run_state, "interrupted");
    assert_eq!(provider_session_state, "idle");

    sqlx::query("UPDATE provider_sessions SET state = 'active' WHERE id = $1")
        .bind(provider_session_id)
        .execute(&pool)
        .await
        .expect("recreate the stale terminal-run provider Session");
    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("repair the already-terminal provider Session"),
        0,
        "provider Session repair must not invent another recovered Agent Run"
    );
    let repaired_provider_session_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM provider_sessions WHERE id = $1")
            .bind(provider_session_id)
            .fetch_one(&pool)
            .await
            .expect("read repaired provider Session");
    assert_eq!(repaired_provider_session_state, "idle");

    let reentered_run_id = Uuid::now_v7();
    sqlx::query("UPDATE provider_sessions SET state = 'active' WHERE id = $1")
        .bind(provider_session_id)
        .execute(&pool)
        .await
        .expect("reactivate provider Session for repeat-crash fixture");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, provider_session_id, repository_id, state,
            prompt, authorization_snapshot, started_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, 'recovering',
            'repeat-crash recovery test', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(reentered_run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("insert Agent Run left recovering by an earlier startup");

    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("re-enter recovery after a second crash"),
        1
    );
    let (reentered_run_state, reentered_session_state, recorded_previous_state) =
        sqlx::query_as::<_, (String, String, String)>(
            r#"
            SELECT ar.state, ps.state, are.payload->>'previous_state'
            FROM agent_runs ar
            JOIN provider_sessions ps ON ps.id = ar.provider_session_id
            JOIN agent_run_events are
              ON are.agent_run_id = ar.id
             AND are.kind = 'recovery'
            WHERE ar.id = $1
            "#,
        )
        .bind(reentered_run_id)
        .fetch_one(&pool)
        .await
        .expect("read repeat-crash recovery result");
    assert_eq!(reentered_run_state, "interrupted");
    assert_eq!(reentered_session_state, "idle");
    assert_eq!(recorded_previous_state, "recovering");

    let mirror_path =
        sqlx::query_scalar::<_, String>("SELECT mirror_path FROM repositories WHERE id = $1")
            .bind(seed.repository_id)
            .fetch_one(&pool)
            .await
            .expect("read recovery mirror path");
    let mirror_path = Path::new(&mirror_path);
    let base_commit = create_test_mirror(temporary_root.path(), mirror_path);
    let recovered_workspace_id = Uuid::now_v7();
    let recovered_worktree_id = Uuid::now_v7();
    let recovered_worktree_parent = temporary_root.path().join("data/worktrees");
    fs::create_dir_all(&recovered_worktree_parent).expect("create recovered worktree parent");
    let recovered_worktree = recovered_worktree_parent
        .canonicalize()
        .expect("canonical recovered worktree parent")
        .join("recovered-completion");
    agent_workspace_server::git_runtime::create_worktree(
        mirror_path,
        &recovered_worktree,
        "codex/run-recovered-completion",
        &base_commit,
    )
    .expect("create exact recovery worktree");
    sqlx::query(
        r#"
        INSERT INTO repository_workspaces (
            id, repository_id, conversation_id, branch_name, base_commit
        )
        VALUES ($1, $2, $3, 'codex/run-recovered-completion', $4)
        "#,
    )
    .bind(recovered_workspace_id)
    .bind(seed.repository_id)
    .bind(seed.thread_id)
    .bind(&base_commit)
    .execute(&pool)
    .await
    .expect("insert recovered Repository Workspace");
    sqlx::query(
        r#"
        INSERT INTO git_worktrees (
            id, repository_workspace_id, path, state
        )
        VALUES ($1, $2, $3, 'ready')
        "#,
    )
    .bind(recovered_worktree_id)
    .bind(recovered_workspace_id)
    .bind(recovered_worktree.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .expect("insert recovered Git worktree");
    sqlx::query("UPDATE provider_sessions SET state = 'active' WHERE id = $1")
        .bind(provider_session_id)
        .execute(&pool)
        .await
        .expect("activate exact completed-turn provider Session");
    let completed_recovery_run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, provider_session_id, provider_turn_id,
            repository_id, repository_workspace_id, state, prompt,
            authorization_snapshot, started_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'turn-recovered', $7, $8, 'working',
            'exact completed recovery test', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(completed_recovery_run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(seed.repository_id)
    .bind(recovered_workspace_id)
    .execute(&pool)
    .await
    .expect("insert exact completed-turn recovery run");

    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("reconcile exact completed provider turn"),
        1
    );
    let (completed_state, completed_session_state, message_text, recovery_result) =
        sqlx::query_as::<_, (String, String, String, String)>(
            r#"
            SELECT run.state, session.state, message.plain_text,
                   event.payload->>'result'
              FROM agent_runs run
              JOIN provider_sessions session ON session.id = run.provider_session_id
              JOIN messages message ON message.agent_run_id = run.id
              JOIN agent_run_events event
                ON event.agent_run_id = run.id
               AND event.kind = 'recovery'
             WHERE run.id = $1
            "#,
        )
        .bind(completed_recovery_run_id)
        .fetch_one(&pool)
        .await
        .expect("read exact completed-turn recovery result");
    assert_eq!(completed_state, "completed");
    assert_eq!(completed_session_state, "idle");
    assert_eq!(message_text, "Recovered proof");
    assert_eq!(recovery_result, "completed");
    assert!(recovered_worktree.is_dir());
    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("repeat exact completed-turn reconciliation"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE agent_run_id = $1",)
            .bind(completed_recovery_run_id)
            .fetch_one(&pool)
            .await
            .expect("count converged recovered Worker Message"),
        1
    );

    sqlx::query("UPDATE repository_workspaces SET state = 'active' WHERE id = $1")
        .bind(recovered_workspace_id)
        .execute(&pool)
        .await
        .expect("reactivate workspace for mismatched-turn recovery");
    sqlx::query("UPDATE provider_sessions SET state = 'active' WHERE id = $1")
        .bind(provider_session_id)
        .execute(&pool)
        .await
        .expect("reactivate provider Session for mismatched-turn recovery");
    let mismatched_recovery_run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, provider_session_id, provider_turn_id,
            repository_id, repository_workspace_id, state, prompt,
            authorization_snapshot, started_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'turn-different', $7, $8, 'working',
            'mismatched completed recovery test', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(mismatched_recovery_run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(seed.repository_id)
    .bind(recovered_workspace_id)
    .execute(&pool)
    .await
    .expect("insert mismatched completed-turn recovery run");
    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("reconcile mismatched completed provider turn"),
        1
    );
    let (mismatched_state, mismatched_reason, provider_resumed, message_count) =
        sqlx::query_as::<_, (String, Option<String>, bool, i64)>(
            r#"
            SELECT run.state, run.failure_reason,
                   (event.payload->>'provider_resumed')::boolean,
                   (SELECT count(*) FROM messages WHERE agent_run_id = run.id)
              FROM agent_runs run
              JOIN agent_run_events event
                ON event.agent_run_id = run.id
               AND event.kind = 'recovery'
             WHERE run.id = $1
            "#,
        )
        .bind(mismatched_recovery_run_id)
        .fetch_one(&pool)
        .await
        .expect("read mismatched completed-turn recovery result");
    assert_eq!(mismatched_state, "interrupted");
    assert!(mismatched_reason.is_some_and(|reason| reason.contains("did not match")));
    assert!(provider_resumed);
    assert_eq!(message_count, 0, "an older completion cannot be inherited");
    assert!(recovered_worktree.is_dir());

    let queued_run_id = Uuid::now_v7();
    let scheduler_claim_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, scheduler_claim_id, scheduler_claimed_at,
            scheduler_lease_expires_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'queued', 'queued restart test',
            '{}'::jsonb, $7, now(), now() + interval '30 seconds'
        )
        "#,
    )
    .bind(queued_run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .bind(scheduler_claim_id)
    .execute(&pool)
    .await
    .expect("insert queued run with an abandoned scheduler lease");
    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("release queued scheduler lease without recovering the run"),
        0
    );
    let (queued_state, queued_claim) = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT state, scheduler_claim_id FROM agent_runs WHERE id = $1",
    )
    .bind(queued_run_id)
    .fetch_one(&pool)
    .await
    .expect("read queued restart result");
    assert_eq!(queued_state, "queued");
    assert!(queued_claim.is_none());

    drop(state);
    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated recovery schema");
}

fn create_test_mirror(runtime_root: &Path, mirror_path: &Path) -> String {
    let source = runtime_root.join("recovery-source");
    fs::create_dir_all(&source).expect("create recovery source repository");
    git(["init", "-b", "main", source.to_string_lossy().as_ref()]);
    fs::write(source.join("KNOWN_FILE.txt"), "known\n").expect("write recovery fixture");
    git([
        "-C",
        source.to_string_lossy().as_ref(),
        "add",
        "KNOWN_FILE.txt",
    ]);
    git([
        "-C",
        source.to_string_lossy().as_ref(),
        "-c",
        "user.name=Agent Workspace Test",
        "-c",
        "user.email=agent-workspace@example.invalid",
        "commit",
        "-m",
        "recovery baseline",
    ]);
    fs::create_dir_all(mirror_path.parent().expect("recovery mirror parent"))
        .expect("create recovery mirror parent");
    git([
        "clone",
        "--mirror",
        source.to_string_lossy().as_ref(),
        mirror_path.to_string_lossy().as_ref(),
    ]);
    git_output([
        "--git-dir",
        mirror_path.to_string_lossy().as_ref(),
        "rev-parse",
        "refs/heads/main",
    ])
}

fn git<const N: usize>(args: [&str; N]) {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .output()
        .expect("run recovery Git command");
    assert!(
        output.status.success(),
        "recovery Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output<const N: usize>(args: [&str; N]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .output()
        .expect("run recovery Git output command");
    assert!(
        output.status.success(),
        "recovery Git output command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("recovery Git output is UTF-8")
        .trim()
        .to_owned()
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
        .max_connections(4)
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
        .expect("connect to isolated recovery schema")
}

use std::{collections::BTreeMap, fs, path::Path, process::Command, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
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
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn provider_start_failure_is_a_durable_failed_timeline_entry() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m2_start_failure_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated start-failure schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let temporary_root = tempfile::tempdir().expect("temporary runtime root");
    let runtime_root = temporary_root
        .path()
        .canonicalize()
        .expect("canonical runtime root");
    let data_root = runtime_root.join("data");
    let codex_home_root = data_root.join("codex");
    let missing_codex = runtime_root.join("missing-codex");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            codex_home_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            missing_codex.to_string_lossy().into_owned(),
        ),
    ]))
    .expect("development configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M2 failure fixture");
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(seed.execution_profile_id)
        .execute(&pool)
        .await
        .expect("authenticate start-failure Profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare start-failure Repository");
    let mirror_path = data_root
        .join("repositories")
        .join("m0-spike")
        .join("mirror.git");
    create_test_mirror(&runtime_root, &mirror_path);

    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "m2-failure-owner".to_owned(),
            display_name: "M2 Failure Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        "correct horse battery staple".to_owned(),
    )
    .await
    .expect("bootstrap seeded owner");

    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
    });
    let (cookie, csrf) = login(&application).await;
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/threads/{}/agent-runs", seed.thread_id))
                .header("origin", "http://127.0.0.1:3100")
                .header("cookie", &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"prompt": "prove durable start failure"}))
                        .expect("serialize start request"),
                ))
                .expect("build start request"),
        )
        .await
        .expect("provider start failure response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let _ = response_json(response).await;

    let (run_id, state, failure_reason, workspace_state, worktree_state, worktree_path) =
        sqlx::query_as::<_, (Uuid, String, Option<String>, String, String, String)>(
            r#"
            SELECT
                run.id,
                run.state,
                run.failure_reason,
                workspace.state,
                worktree.state,
                worktree.path
            FROM agent_runs run
            JOIN repository_workspaces workspace
              ON workspace.id = run.repository_workspace_id
            JOIN git_worktrees worktree
              ON worktree.repository_workspace_id = workspace.id
            WHERE run.conversation_id = $1
            ORDER BY run.created_at DESC, run.id DESC
            LIMIT 1
            "#,
        )
        .bind(seed.thread_id)
        .fetch_one(&pool)
        .await
        .expect("read failed start state");
    assert_eq!(state, "failed");
    assert_eq!(
        failure_reason.as_deref(),
        Some("Codex App Server could not start. The worktree was preserved.")
    );
    assert_eq!(workspace_state, "retained");
    assert_eq!(worktree_state, "ready");
    assert!(
        Path::new(&worktree_path).is_dir(),
        "the worktree is preserved"
    );

    let (event_count, failure_sequence) = sqlx::query_as::<_, (i64, Option<i64>)>(
        r#"
        SELECT
            count(*),
            max(entry.sequence)
        FROM agent_run_events event
        JOIN conversation_entries entry
          ON entry.agent_run_event_id = event.id
         AND entry.conversation_id = event.conversation_id
        WHERE event.agent_run_id = $1
          AND event.kind = 'run_failed'
        "#,
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("read durable failure entry");
    assert_eq!(event_count, 1);
    assert_eq!(failure_sequence, Some(2), "prompt then run_failed");
    assert!(
        sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM outbox_events
                WHERE event_type = 'conversation.agent_run.updated'
                  AND conversation_id = $1
                  AND resource_id = $2
            )
            "#,
        )
        .bind(seed.thread_id)
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read failed-run outbox invalidation")
    );

    let sidebar = application
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{}/conversation-sidebar?selectedChannelId={}",
                    seed.project_id, seed.channel_id
                ))
                .header("cookie", cookie)
                .body(Body::empty())
                .expect("build sidebar request"),
        )
        .await
        .expect("failed sidebar response");
    assert_eq!(sidebar.status(), StatusCode::OK);
    let sidebar = response_json(sidebar).await;
    assert_eq!(
        sidebar["channels"][0]["threads"][0]["id"],
        seed.thread_id.to_string()
    );
    assert_eq!(
        sidebar["channels"][0]["threads"][0]["attention"]["label"],
        "Run failed"
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&administrative_pool)
        .await
        .expect("drop isolated start-failure schema");
}

fn create_test_mirror(runtime_root: &Path, mirror_path: &Path) {
    let source = runtime_root.join("source");
    fs::create_dir_all(&source).expect("create source repository directory");
    run_git(["init", "-b", "main", source.to_string_lossy().as_ref()]);
    fs::write(source.join("KNOWN_FILE.txt"), "known\n").expect("write known fixture file");
    run_git([
        "-C",
        source.to_string_lossy().as_ref(),
        "add",
        "KNOWN_FILE.txt",
    ]);
    run_git([
        "-C",
        source.to_string_lossy().as_ref(),
        "-c",
        "user.name=Agent Workspace Test",
        "-c",
        "user.email=agent-workspace@example.invalid",
        "commit",
        "-m",
        "initial",
    ]);
    fs::create_dir_all(mirror_path.parent().expect("mirror parent")).expect("create mirror parent");
    run_git([
        "clone",
        "--mirror",
        source.to_string_lossy().as_ref(),
        mirror_path.to_string_lossy().as_ref(),
    ]);
}

fn run_git<const N: usize>(arguments: [&str; N]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run Git fixture command");
    assert!(
        output.status.success(),
        "Git fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn login(application: &axum::Router) -> (String, String) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "loginName": "m2-failure-owner",
                        "password": "correct horse battery staple"
                    }))
                    .expect("serialize login request"),
                ))
                .expect("build login request"),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("cookie header")
                .split(';')
                .next()
                .expect("cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    let _ = response_json(response).await;
    (cookies.join("; "), csrf)
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("read response body"),
    )
    .expect("parse response JSON")
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
        .expect("connect to isolated start-failure schema")
}

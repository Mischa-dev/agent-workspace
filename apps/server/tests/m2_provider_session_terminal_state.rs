use std::{collections::BTreeMap, fs, path::Path, process::Command, sync::Arc, time::Duration};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    seed::{DevSeed, dev_seed},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tempfile::TempDir;
use tokio::time::{Instant, sleep};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:3100";
const LOGIN_NAME: &str = "provider-session-owner";
const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn authenticated_interrupt_idles_only_an_unowned_provider_session() {
    let Some(harness) = TestHarness::new("m2_provider_interrupt", false).await else {
        return;
    };
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
    .bind(harness.seed.thread_id)
    .bind(harness.seed.execution_profile_id)
    .bind(format!("interrupt-thread-{provider_session_id}"))
    .execute(&harness.pool)
    .await
    .expect("insert active provider Session");

    let interrupted_run_id = insert_working_run(&harness, provider_session_id).await;
    let response = interrupt(&harness, interrupted_run_id).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["id"], interrupted_run_id.to_string());
    assert_eq!(response["state"], "interrupted");
    assert_eq!(
        response["providerSessionId"],
        provider_session_id.to_string()
    );

    let (run_state, session_state, finished, interruption_entries, outbox_events) =
        sqlx::query_as::<_, (String, String, bool, i64, i64)>(
            r#"
            SELECT
                run.state,
                session.state,
                run.finished_at IS NOT NULL,
                (
                    SELECT count(*)
                    FROM agent_run_events event
                    JOIN conversation_entries entry
                      ON entry.agent_run_event_id = event.id
                     AND entry.conversation_id = run.conversation_id
                    WHERE event.agent_run_id = run.id
                      AND event.kind = 'run_interrupted'
                ),
                (
                    SELECT count(*)
                    FROM outbox_events event
                    WHERE event.event_type = 'conversation.agent_run.updated'
                      AND event.conversation_id = run.conversation_id
                      AND event.resource_id = run.id
                )
            FROM agent_runs run
            JOIN provider_sessions session ON session.id = run.provider_session_id
            WHERE run.id = $1
            "#,
        )
        .bind(interrupted_run_id)
        .fetch_one(&harness.pool)
        .await
        .expect("read committed interrupt state");
    assert_eq!(run_state, "interrupted");
    assert_eq!(session_state, "idle");
    assert!(finished);
    assert_eq!(interruption_entries, 1);
    assert_eq!(outbox_events, 1);

    let successor_run_id = insert_working_run(&harness, provider_session_id).await;
    sqlx::query("UPDATE provider_sessions SET state = 'active' WHERE id = $1")
        .bind(provider_session_id)
        .execute(&harness.pool)
        .await
        .expect("activate Provider Session for the legitimate successor run");

    let repeated = interrupt(&harness, interrupted_run_id).await;
    assert_eq!(repeated.status(), StatusCode::OK);
    let _ = response_json(repeated).await;
    let (session_state, successor_state) = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT session.state, successor.state
        FROM provider_sessions session
        JOIN agent_runs successor ON successor.provider_session_id = session.id
        WHERE session.id = $1 AND successor.id = $2
        "#,
    )
    .bind(provider_session_id)
    .bind(successor_run_id)
    .fetch_one(&harness.pool)
    .await
    .expect("read session ownership after repeated interrupt");
    assert_eq!(session_state, "active");
    assert_eq!(successor_state, "working");

    let successor_interrupt = interrupt(&harness, successor_run_id).await;
    assert_eq!(successor_interrupt.status(), StatusCode::OK);
    let _ = response_json(successor_interrupt).await;
    let final_session_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM provider_sessions WHERE id = $1")
            .bind(provider_session_id)
            .fetch_one(&harness.pool)
            .await
            .expect("read Provider Session after successor interrupt");
    assert_eq!(final_session_state, "idle");

    harness.cleanup().await;
}

#[tokio::test]
async fn provider_turn_failure_idles_its_linked_session_without_live_credentials() {
    let Some(harness) = TestHarness::new("m2_provider_failure", true).await else {
        return;
    };
    create_test_mirror(
        harness.temporary_root.path(),
        &harness
            .config
            .data_root
            .join("repositories")
            .join("m0-spike")
            .join("mirror.git"),
    );

    let response = harness
        .application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/threads/{}/agent-runs",
                    harness.seed.thread_id
                ))
                .header("origin", ORIGIN)
                .header("cookie", &harness.cookie)
                .header("x-csrf-token", &harness.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "prompt": "exercise a deterministic provider turn failure"
                    }))
                    .expect("serialize start request"),
                ))
                .expect("build start request"),
        )
        .await
        .expect("start fake-provider run");
    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    let run_id = response["runId"]
        .as_str()
        .expect("run ID in start response")
        .parse::<Uuid>()
        .expect("valid run ID");

    let deadline = Instant::now() + Duration::from_secs(10);
    let (run_state, session_state, failure_reason, provider_session_id) = loop {
        let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<Uuid>)>(
            r#"
            SELECT run.state, session.state, run.failure_reason, run.provider_session_id
            FROM agent_runs run
            LEFT JOIN provider_sessions session ON session.id = run.provider_session_id
            WHERE run.id = $1
            "#,
        )
        .bind(run_id)
        .fetch_one(&harness.pool)
        .await
        .expect("read provider failure state");
        if row.0 == "failed" {
            break (
                row.0,
                row.1.expect("failed linked run retains Provider Session"),
                row.2.expect("failed run records a bounded reason"),
                row.3
                    .expect("failed linked run retains Provider Session ID"),
            );
        }
        assert!(
            Instant::now() < deadline,
            "fake provider run did not fail before the test deadline (last state: {})",
            row.0
        );
        sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(run_state, "failed");
    assert_eq!(session_state, "idle");
    assert_eq!(
        failure_reason,
        "The hosted Codex turn failed. The worktree was preserved."
    );

    let (failure_entries, outbox_events) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT
            (
                SELECT count(*)
                FROM agent_run_events event
                JOIN conversation_entries entry
                  ON entry.agent_run_event_id = event.id
                WHERE event.agent_run_id = $1
                  AND event.kind = 'run_failed'
            ),
            (
                SELECT count(*)
                FROM outbox_events event
                WHERE event.event_type = 'conversation.agent_run.updated'
                  AND event.resource_id = $1
            )
        "#,
    )
    .bind(run_id)
    .fetch_one(&harness.pool)
    .await
    .expect("read committed provider failure effects");
    assert_eq!(failure_entries, 1);
    assert!(
        outbox_events >= 2,
        "start and failure both invalidate the run"
    );
    assert_ne!(provider_session_id, Uuid::nil());

    harness.cleanup().await;
}

struct TestHarness {
    administrative_pool: PgPool,
    pool: PgPool,
    schema: String,
    temporary_root: TempDir,
    config: AppConfig,
    seed: DevSeed,
    application: axum::Router,
    cookie: String,
    csrf: String,
}

impl TestHarness {
    async fn new(prefix: &str, fake_provider: bool) -> Option<Self> {
        let (database_url, administrative_pool) = integration_pool().await?;
        let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&administrative_pool)
            .await
            .expect("create isolated Provider Session schema");
        let pool = schema_pool(&database_url, &schema).await;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let temporary_root = tempfile::tempdir().expect("temporary Provider Session root");
        let runtime_root = temporary_root
            .path()
            .canonicalize()
            .expect("canonical test runtime root");
        let data_root = runtime_root.join("data");
        let codex_home_root = runtime_root.join("codex");
        let fake_codex = runtime_root.join("fake-codex");
        if fake_provider {
            write_fake_codex(&fake_codex);
        }
        let mut values = BTreeMap::from([
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
                "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
                runtime_root.to_string_lossy().into_owned(),
            ),
        ]);
        if fake_provider {
            values.insert(
                "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
                fake_codex.to_string_lossy().into_owned(),
            );
        }
        let config = AppConfig::from_values(&values).expect("development configuration");
        let seed = dev_seed(&pool, &config)
            .await
            .expect("seed Provider Session fixture");
        bootstrap_admin(
            &pool,
            BootstrapCommand {
                login_name: LOGIN_NAME.to_owned(),
                display_name: "Provider Session Owner".to_owned(),
                organization_name: None,
                organization_slug: None,
                adopt_actor_id: Some(seed.human_actor_id),
                organization_id: Some(seed.organization_id),
            },
            PASSWORD.to_owned(),
        )
        .await
        .expect("bootstrap seeded Provider Session owner");

        let application = router(AppState {
            config: Arc::new(config.clone()),
            pool: pool.clone(),
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(16),
        });
        let (cookie, csrf) = login(&application).await;
        Some(Self {
            administrative_pool,
            pool,
            schema,
            temporary_root,
            config,
            seed,
            application,
            cookie,
            csrf,
        })
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.administrative_pool)
            .await
            .expect("drop isolated Provider Session schema");
    }
}

async fn insert_working_run(harness: &TestHarness, provider_session_id: Uuid) -> Uuid {
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
            'Provider Session terminal-state regression', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(run_id)
    .bind(harness.seed.thread_id)
    .bind(harness.seed.execution_worker_id)
    .bind(harness.seed.human_actor_id)
    .bind(harness.seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(harness.seed.repository_id)
    .execute(&harness.pool)
    .await
    .expect("insert working Agent Run linked to Provider Session");
    run_id
}

async fn interrupt(harness: &TestHarness, run_id: Uuid) -> axum::response::Response {
    harness
        .application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/agent-runs/{run_id}/interrupt"))
                .header("origin", ORIGIN)
                .header("cookie", &harness.cookie)
                .header("x-csrf-token", &harness.csrf)
                .body(Body::empty())
                .expect("build interrupt request"),
        )
        .await
        .expect("interrupt response")
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

fn write_fake_codex(path: &Path) {
    fs::write(
        path,
        r#"#!/bin/sh
IFS= read -r request || exit 1
printf '%s\n' '{"id":1,"result":{"serverInfo":{"name":"fake-codex"}}}'
IFS= read -r initialized || exit 1
IFS= read -r request || exit 1
printf '%s\n' '{"id":2,"result":{"thread":{"id":"fake-provider-thread"}}}'
IFS= read -r request || exit 1
printf '%s\n' '{"id":3,"result":{"turn":{"id":"fake-provider-turn"}}}'
printf '%s\n' '{"method":"error","params":{"message":"deterministic fixture failure"}}'
while IFS= read -r ignored; do :; done
"#,
    )
    .expect("write fake Codex executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("make fake Codex executable");
    }
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
        .expect("connect to isolated Provider Session schema")
}

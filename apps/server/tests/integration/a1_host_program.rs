//! A1.17: the built-in host as its own program. The server runs with the
//! built-in host off; `agent-workspace-host` runs beside it as a separate
//! process against the same database, enrols with a one-time code and
//! carries out runs over HTTP. A server restart mid-run leaves the run
//! going; a host restart recovers it, once.

use std::{collections::BTreeMap, path::Path, process::Stdio};

use agent_workspace_server::{
    bootstrap::{BootstrapCommand, bootstrap_admin},
    cluster::LeaseName,
    config::AppConfig,
    git_runtime::expected_repository_mirror_path,
    hosts::builtin::install::{self, Prepared},
    seed::dev_seed,
};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use tokio::{
    process::{Child, Command},
    time::{Duration, Instant, sleep},
};
use uuid::Uuid;

use crate::{
    cluster_support::{Instance, cluster_on},
    m3_scheduler_http::{create_test_mirror, uuid_field},
    support::{app::PASSWORD, db::db},
};

const LOGIN: &str = "host-program-owner";

/// What the server, the host and the seed all agree on.
fn shared(root: &Path) -> Vec<(&'static str, String)> {
    let data = root.join("data");
    vec![
        ("AGENT_WORKSPACE_DATA_ROOT", data.display().to_string()),
        (
            "AGENT_WORKSPACE_PROVIDER_HOME_ROOT",
            data.join("codex/profiles").display().to_string(),
        ),
        ("AGENT_WORKSPACE_WEB_ROOT", root.display().to_string()),
        ("AGENT_WORKSPACE_DEMO_EXECUTOR", "true".to_owned()),
        (
            "AGENT_WORKSPACE_HARNESS_HOST_CONCURRENCY_LIMIT",
            "2".to_owned(),
        ),
        ("AGENT_WORKSPACE_LEASE_SECONDS", "2".to_owned()),
    ]
}

/// The program, started as its own process.
fn spawn_host(root: &Path, database_url: &str, server: &str, code_file: &Path) -> Child {
    // From a nextest archive (CI builds once and runs elsewhere) the build's
    // path is gone; nextest names where the binary landed.
    let program = std::env::var_os("NEXTEST_BIN_EXE_agent-workspace-host")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_agent_workspace_host"))
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_agent-workspace-host").into());
    let mut command = Command::new(program);
    for (key, value) in shared(root) {
        command.env(key, value);
    }
    command
        .env("AGENT_WORKSPACE_DATABASE_URL", database_url)
        .env("AGENT_WORKSPACE_HOST_SERVER_URL", server)
        // The demo executor is development-only; CI runs the tests with
        // AGENT_WORKSPACE_ENV=test, which the program would inherit.
        .env("AGENT_WORKSPACE_ENV", "development")
        .env("AGENT_WORKSPACE_HOST_CODE_PATH", code_file)
        .env("AGENT_WORKSPACE_HEALTH_ADDR", "127.0.0.1:0")
        .env("AGENT_WORKSPACE_LOG_FILTER", "warn")
        .current_dir(root)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("start agent-workspace-host")
}

async fn until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check().await {
        assert!(Instant::now() < deadline, "{what} never happened");
        sleep(Duration::from_millis(50)).await;
    }
}

async fn start_run(
    server: &Instance,
    browser: &crate::support::app::Browser,
    thread: Uuid,
    key: &str,
) -> Uuid {
    let (status, started) = server
        .send(browser.request(
            "POST",
            &format!("/api/v1/threads/{thread}/agent-runs"),
            Some(key),
            json!({ "prompt": "through the host program" }),
        ))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    uuid_field(&started, "runId")
}

/// The demo harness's open request, made a question, answered in words.
async fn answer(
    server: &Instance,
    browser: &crate::support::app::Browser,
    pool: &PgPool,
    run: Uuid,
) {
    let request = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM requests WHERE run_id = $1 AND status = 'open'",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .expect("the demo's request");
    sqlx::query(
        "UPDATE requests SET kind = 'question', title = 'Which way?', options = '[]'::jsonb, \
         allow_text = true WHERE id = $1",
    )
    .bind(request)
    .execute(pool)
    .await
    .expect("a question");
    let (status, answered) = server
        .send(browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some(&format!("answer-{request}")),
            json!({ "requestId": request, "text": "North" }),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
}

/// Waits for `run` to reach `expected`; if it ends otherwise, says why.
async fn reach(pool: &PgPool, run: Uuid, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (state, reason) = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT state, failure_reason FROM agent_runs WHERE id = $1",
        )
        .bind(run)
        .fetch_one(pool)
        .await
        .expect("run");
        if state == expected {
            return;
        }
        let ended = matches!(
            state.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        );
        if ended || Instant::now() > deadline {
            let steps = sqlx::query_scalar::<_, String>(
                "SELECT concat_ws(' ', payload->>'step', summary) FROM run_activities \
                 WHERE run_id = $1 ORDER BY seq DESC LIMIT 8",
            )
            .bind(run)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            panic!("run is {state}, not {expected}: {reason:?}; latest steps: {steps:?}");
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// How many times the run was recovered.
async fn recoveries(pool: &PgPool, run: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM run_activities WHERE run_id = $1 AND payload->>'step' = 'recovery'",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .expect("recoveries")
}

#[tokio::test]
async fn the_host_runs_as_its_own_program_through_restarts_of_either() {
    let Some(db) = db("a1_host_program").await else {
        return;
    };
    let root = tempfile::tempdir().expect("root");
    let data = root.path().join("data");
    let mut values = shared(root.path())
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect::<BTreeMap<_, _>>();
    values.insert("AGENT_WORKSPACE_DATABASE_URL".to_owned(), db.url.clone());
    let config = AppConfig::from_values(&values).expect("configuration");
    let seed = dev_seed(&db.pool, &config).await.expect("seed");
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(seed.execution_profile_id)
        .execute(&db.pool)
        .await
        .expect("profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&db.pool)
    .await
    .expect("repository");
    create_test_mirror(
        root.path(),
        &expected_repository_mirror_path(&data, seed.repository_id).expect("mirror path"),
    );
    bootstrap_admin(
        &db.pool,
        BootstrapCommand {
            login_name: LOGIN.to_owned(),
            display_name: "Host Program Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("owner");
    let pool = db.pool.clone();
    let url = db.url.clone();
    let mut cluster = cluster_on(db, seed.organization_id);

    // The server, with the built-in host switched off.
    let mut server_values = shared(root.path());
    server_values.push(("AGENT_WORKSPACE_BUILTIN_HOST", "off".to_owned()));
    let server_settings = server_values
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    cluster.start(&server_settings).await;
    let base = cluster.instance(0).base().to_owned();
    let address = base.trim_start_matches("http://").to_owned();

    // The host program enrols with the code the server wrote as it started
    // (a server starting again leaves it be).
    let code_file = root.path().join("code");
    assert_eq!(
        install::prepare(&pool, &code_file).await.expect("code"),
        Prepared::Written(code_file.clone())
    );
    assert_eq!(
        install::prepare(&pool, &code_file).await.expect("code"),
        Prepared::Waiting(code_file.clone())
    );
    let code = std::fs::read_to_string(&code_file).expect("the written code");
    let mut host = spawn_host(root.path(), &url, &base, &code_file);
    let token_file = data.join("builtin-host/token");
    until("the host's enrolment", async || token_file.exists()).await;
    assert!(!code_file.exists(), "the used code is removed");
    assert_eq!(
        install::prepare(&pool, &code_file).await.expect("enrolled"),
        Prepared::Enrolled
    );
    assert!(!code_file.exists(), "no new code once a host enrolled");

    // A run started on the server is carried out by the program: its start
    // fetched and answered over HTTP with the install's token.
    let browser = cluster.instance(0).login(LOGIN).await;
    let first = start_run(cluster.instance(0), &browser, seed.thread_id, "first").await;
    reach(&pool, first, "blocked").await;
    assert!(
        !cluster.instance(0).cluster().holds(&LeaseName::BuiltinHost),
        "the server doesn't hold the host"
    );
    let acknowledged = sqlx::query_scalar::<_, bool>(
        "SELECT acked_at IS NOT NULL FROM host_commands WHERE run_id = $1 AND kind = 'start'",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .expect("start command");
    assert!(acknowledged, "the program acknowledged the start");
    let seen = sqlx::query_scalar::<_, bool>(
        "SELECT bool_or(last_seen_at IS NOT NULL) FROM builtin_host_installs WHERE revoked_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("install");
    assert!(seen, "the program called with the install's token");

    // The server restarts mid-run, on the same address; the run goes on
    // and finishes once answered.
    cluster.instance_mut(0).stop().await;
    cluster.start_at(&server_settings, &address).await;
    let server = cluster.instance(1);
    answer(server, &browser, &pool, first).await;
    reach(&pool, first, "completed").await;

    // The host dies mid-run; started again, it recovers the run once.
    let second = start_run(server, &browser, seed.thread_id, "second").await;
    reach(&pool, second, "blocked").await;
    host.kill().await.expect("stop the host");
    let _ = host.wait().await;
    // A spent code left in place (a read-only mount can't lose it) is not
    // offered again: the host starts on the token it kept.
    std::fs::write(&code_file, &code).expect("leave the spent code");
    let mut host = spawn_host(root.path(), &url, &base, &code_file);
    until("the run's recovery", async || {
        recoveries(&pool, second).await >= 1
    })
    .await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(recoveries(&pool, second).await, 1, "recovered once");
    let (state, reason) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, failure_reason FROM agent_runs WHERE id = $1",
    )
    .bind(second)
    .fetch_one(&pool)
    .await
    .expect("second run");
    assert!(
        matches!(state.as_str(), "blocked" | "working" | "completed")
            || (state == "interrupted" && reason.is_some()),
        "recovered honestly: {state} {reason:?}"
    );

    host.kill().await.expect("stop the host");
    let _ = host.wait().await;
    cluster.finish().await;
}

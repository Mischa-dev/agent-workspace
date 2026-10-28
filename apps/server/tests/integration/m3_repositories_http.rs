use std::{collections::BTreeMap, sync::Arc};

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
use tower::ServiceExt;
use uuid::Uuid;

const LOGIN_NAME: &str = "m3-repository-owner";
const PASSWORD: &str = "correct horse battery staple";

#[cfg(unix)]
#[tokio::test]
async fn repository_registration_preparation_archive_and_policy_are_safe_and_idempotent() {
    use std::{fs, process::Command};

    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_repositories_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M3 repository schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M3 repository migrations");

    let temporary = tempfile::tempdir().expect("temporary repository runtime root");
    let data_root = temporary.path().join("data");
    let mut config = AppConfig::from_values(&BTreeMap::from([
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
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("repository test configuration");
    // Repository preparation semantics are covered independently from host capacity.
    // Keep this temporary mirror fixture deterministic as CI disk usage changes.
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed repository fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M3 Repository Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap repository owner");

    let local_source = temporary.path().join("source");
    fs::create_dir(&local_source).expect("create local source");
    git(&local_source, ["init", "-b", "main"]);
    fs::write(local_source.join("known.txt"), "known\n").expect("write local source file");
    git(&local_source, ["add", "known.txt"]);
    git(
        &local_source,
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
    let local_repository_id = Uuid::now_v7();
    let local_mirror = expected_repository_mirror_path(&data_root, local_repository_id)
        .expect("local canonical mirror path");
    fs::create_dir_all(local_mirror.parent().expect("local mirror parent"))
        .expect("create local mirror parent");
    let imported = Command::new("/usr/bin/git")
        .args([
            "clone",
            "--mirror",
            local_source.to_string_lossy().as_ref(),
            local_mirror.to_string_lossy().as_ref(),
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("create imported local mirror fixture");
    assert!(imported.status.success());
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, legacy_mirror_path,
            enabled, trusted, preparation_state
        )
        VALUES ($1, $2, 9000000001, 'Fixture', 'local-repository', $3,
                'main', $4, $5, true, true, 'pending')
        "#,
    )
    .bind(local_repository_id)
    .bind(seed.project_id)
    .bind(local_source.to_string_lossy().as_ref())
    .bind(local_mirror.to_string_lossy().as_ref())
    .bind(local_source.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .expect("insert internal local repository fixture");

    let supervisors = SupervisorRegistry::new(&config).expect("repository supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors: supervisors.clone(),
    });
    let base = format!("/api/v1/projects/{}/repositories", seed.project_id);

    let unauthenticated = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(&base)
                .body(Body::empty())
                .expect("build unauthenticated repository request"),
        )
        .await
        .expect("unauthenticated repository response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let cookie = login(&application).await;
    let csrf = csrf_from_cookie(&cookie);
    let registration = json!({
        "githubRepositoryId": 9000000002_i64,
        "owner": "Mischa-dev",
        "name": "m3-registration-fixture",
        "cloneUrl": "https://github.com/Mischa-dev/m3-registration-fixture.git",
        "defaultBranch": "main",
        "trustAcknowledged": true
    });
    let missing_csrf = post_json(
        &application,
        &cookie,
        None,
        Some("register-key"),
        &base,
        registration.clone(),
    )
    .await;
    assert_eq!(missing_csrf.0, StatusCode::FORBIDDEN);

    let (created_status, created_text, created) = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("register-key"),
        &base,
        registration.clone(),
    )
    .await;
    assert_eq!(created_status, StatusCode::CREATED);
    assert_eq!(created["preparationState"], "pending");
    assert_eq!(created["trusted"], true);
    assert!(created.get("mirrorPath").is_none());
    assert!(!created_text.contains(data_root.to_string_lossy().as_ref()));

    let replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("register-key"),
        &base,
        registration.clone(),
    )
    .await;
    assert_eq!(replay.0, StatusCode::CREATED);
    assert_eq!(replay.2["id"], created["id"]);

    let duplicate = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("duplicate-key"),
        &base,
        registration,
    )
    .await;
    assert_eq!(duplicate.0, StatusCode::CONFLICT);

    let unsafe_registration = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("unsafe-key"),
        &base,
        json!({
            "githubRepositoryId": 9000000003_i64,
            "owner": "Mischa-dev",
            "name": "unsafe",
            "cloneUrl": "https://token@github.com/Mischa-dev/unsafe.git",
            "defaultBranch": "-upload-pack",
            "trustAcknowledged": true
        }),
    )
    .await;
    assert_eq!(unsafe_registration.0, StatusCode::BAD_REQUEST);

    let prepared = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("prepare-key"),
        &format!("{base}/{local_repository_id}/prepare"),
        json!({}),
    )
    .await;
    assert_eq!(prepared.0, StatusCode::OK);
    assert_eq!(prepared.2["preparationState"], "ready");
    assert!(local_mirror.is_dir());

    let tamper = Command::new("/usr/bin/git")
        .args([
            "--git-dir",
            local_mirror.to_string_lossy().as_ref(),
            "remote",
            "set-url",
            "origin",
            "https://github.com/wrong/repository.git",
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/dev/null")
        .output()
        .expect("tamper fixture origin");
    assert!(tamper.status.success());
    let failed_preparation = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("prepare-mismatch-key"),
        &format!("{base}/{local_repository_id}/prepare"),
        json!({}),
    )
    .await;
    assert_eq!(failed_preparation.0, StatusCode::OK);
    assert_eq!(failed_preparation.2["preparationState"], "failed");
    assert_eq!(
        failed_preparation.2["preparationFailureCategory"],
        "git_prepare_failed"
    );

    let foreign_project_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'FOREIGN', 'Foreign Project')",
    )
    .bind(foreign_project_id)
    .bind(seed.organization_id)
    .execute(&pool)
    .await
    .expect("insert foreign Project without membership");
    let foreign = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{foreign_project_id}/repositories"),
    )
    .await;
    assert_eq!(foreign.0, StatusCode::NOT_FOUND);

    sqlx::query(
        "UPDATE project_memberships SET role = 'member' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("make fixture Actor a non-manager");
    let forbidden = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("forbidden-key"),
        &format!("{base}/{local_repository_id}/prepare"),
        json!({}),
    )
    .await;
    assert_eq!(forbidden.0, StatusCode::FORBIDDEN);
    sqlx::query(
        "UPDATE project_memberships SET role = 'owner' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("restore fixture owner role");

    let created_id = created["id"].as_str().expect("created repository id");
    let created_version = created["version"]
        .as_i64()
        .expect("created repository version");

    // A repository with a run that hasn't finished stays, and the answer
    // says why.
    let created_uuid = Uuid::parse_str(created_id).expect("created repository UUID");
    let run_thread = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
        VALUES ($1, $2, 'thread', $3, $4, 'Archive guard')
        "#,
    )
    .bind(run_thread)
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .bind(created_uuid)
    .execute(&pool)
    .await
    .expect("insert run thread");
    let run = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, started_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'failed', 'Archive guard', '{}'::jsonb, now())
        "#,
    )
    .bind(run)
    .bind(run_thread)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(created_uuid)
    .execute(&pool)
    .await
    .expect("insert run");
    // Admission checks a live account on insert; a run already waiting on
    // an approval only needs the row.
    sqlx::query("UPDATE agent_runs SET state = 'blocked' WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .expect("block run");
    let refused = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("archive-active-key"),
        &format!("{base}/{created_id}/archive"),
        json!({"expectedVersion": created_version}),
    )
    .await;
    assert_eq!(refused.0, StatusCode::CONFLICT, "{}", refused.1);
    assert_eq!(refused.2["error"]["code"], "repository_has_active_runs");
    sqlx::query("UPDATE agent_runs SET state = 'failed', finished_at = now() WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .expect("finish run");

    let archived = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("archive-key"),
        &format!("{base}/{created_id}/archive"),
        json!({"expectedVersion": created_version}),
    )
    .await;
    assert_eq!(archived.0, StatusCode::OK);
    assert_eq!(archived.2["enabled"], false);
    assert!(archived.2["archivedAt"].is_string());

    let archived_replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("archive-key"),
        &format!("{base}/{created_id}/archive"),
        json!({"expectedVersion": created_version}),
    )
    .await;
    assert_eq!(archived_replay.0, StatusCode::OK);
    assert_eq!(archived_replay.2["version"], archived.2["version"]);

    let (_, list_text, list) = get_json(&application, &cookie, &base).await;
    assert!(
        list["repositories"]
            .as_array()
            .is_some_and(|repositories| repositories.len() >= 3)
    );
    assert!(!list_text.contains(data_root.to_string_lossy().as_ref()));
    assert!(!list_text.contains("mirrorPath"));

    supervisors.shutdown_all().await;
    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated M3 repository schema");

    fn git<const N: usize>(repository: &std::path::Path, arguments: [&str; N]) {
        let output = Command::new("/usr/bin/git")
            .current_dir(repository)
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/dev/null")
            .output()
            .expect("run repository fixture Git command");
        assert!(output.status.success());
    }
}

async fn login(application: &axum::Router) -> String {
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
                        "loginName": LOGIN_NAME,
                        "password": PASSWORD
                    }))
                    .expect("serialize repository login request"),
                ))
                .expect("build repository login request"),
        )
        .await
        .expect("repository login response");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("repository cookie header")
                .split(';')
                .next()
                .expect("repository cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn csrf_from_cookie(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|pair| pair.strip_prefix("agent_workspace_csrf="))
        .expect("repository CSRF cookie")
        .to_owned()
}

async fn get_json(
    application: &axum::Router,
    cookie: &str,
    uri: &str,
) -> (StatusCode, String, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .expect("build repository GET request"),
        )
        .await
        .expect("repository GET response");
    response_json(response).await
}

async fn post_json(
    application: &axum::Router,
    cookie: &str,
    csrf: Option<&str>,
    idempotency_key: Option<&str>,
    uri: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(csrf) = csrf {
        request = request
            .header("origin", "http://127.0.0.1:3100")
            .header("x-csrf-token", csrf);
    }
    if let Some(idempotency_key) = idempotency_key {
        request = request.header("idempotency-key", idempotency_key);
    }
    let response = application
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize repository request"),
                ))
                .expect("build repository POST request"),
        )
        .await
        .expect("repository POST response");
    response_json(response).await
}

async fn response_json(response: axum::response::Response) -> (StatusCode, String, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read repository response");
    let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 repository response");
    let value = serde_json::from_str(&text).expect("JSON repository response");
    (status, text, value)
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
        .expect("connect schema-scoped M3 repository pool")
}

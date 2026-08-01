use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::Command,
    sync::Arc,
};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
    config::AppConfig,
    git_runtime::{create_worktree, expected_repository_mirror_path, managed_worktree_path},
    github::webhook_router,
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
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const OWNER_LOGIN: &str = "m5-github-owner";
const VIEWER_LOGIN: &str = "m5-github-viewer";
const PASSWORD: &str = "correct horse battery staple";
const APP_ID: i64 = 4_442_593;
const INSTALLATION_ID: i64 = 150_232_852;
const WEBHOOK_SECRET: &str = "m5-test-webhook-secret-with-at-least-32-bytes";

#[tokio::test]
async fn github_status_and_webhook_receipt_are_scoped_signed_and_idempotent() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m5_github_foundation_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M5 GitHub schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M5 GitHub migrations");

    let temporary = tempfile::tempdir().expect("temporary M5 runtime root");
    let data_root = temporary.path().join("data");
    let private_key = temporary.path().join("github-app.pem");
    let webhook_secret = temporary.path().join("github-webhook-secret");
    write_owner_only(&private_key, b"test-private-key-placeholder");
    write_owner_only(&webhook_secret, WEBHOOK_SECRET.as_bytes());
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
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().join("web").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_GITHUB_APP_ID".to_owned(),
            APP_ID.to_string(),
        ),
        (
            "AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH".to_owned(),
            private_key.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH".to_owned(),
            webhook_secret.to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M5 GitHub test configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M5 GitHub fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OWNER_LOGIN.to_owned(),
            display_name: "M5 GitHub Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M5 owner");
    bootstrap_development_member(
        &pool,
        BootstrapCommand {
            login_name: VIEWER_LOGIN.to_owned(),
            display_name: "M5 GitHub Viewer".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.collaborator_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M5 viewer");
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.collaborator_actor_id)
    .execute(&pool)
    .await
    .expect("set viewer role");

    let integration_actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'integration', 'active')")
        .bind(integration_actor_id)
        .execute(&pool)
        .await
        .expect("insert GitHub Integration Actor");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) VALUES ($1, $2, $3, 'member', 'GitHub', 'github_integration')",
    )
    .bind(Uuid::now_v7())
    .bind(seed.project_id)
    .bind(integration_actor_id)
    .execute(&pool)
    .await
    .expect("insert GitHub Project membership");
    let installation_row_id = Uuid::now_v7();
    let events = vec![
        "push",
        "pull_request",
        "pull_request_review",
        "check_suite",
        "check_run",
        "repository",
    ];
    sqlx::query(
        r#"
        INSERT INTO github_installations (
            id, project_id, integration_actor_id, app_id, installation_id,
            account_id, account_login, target_type, repository_selection,
            metadata_permission, contents_permission, pull_requests_permission,
            checks_permission, subscribed_events, state, enabled,
            last_success_at
        ) VALUES (
            $1, $2, $3, $4, $5, 777, 'Mischa-dev', 'Organization', 'selected',
            'read', 'write', 'write', 'read', $6, 'available', true, now()
        )
        "#,
    )
    .bind(installation_row_id)
    .bind(seed.project_id)
    .bind(integration_actor_id)
    .bind(APP_ID)
    .bind(INSTALLATION_ID)
    .bind(&events)
    .execute(&pool)
    .await
    .expect("insert GitHub installation");
    let github_repository_id =
        sqlx::query_scalar::<_, i64>("SELECT github_repository_id FROM repositories WHERE id = $1")
            .bind(seed.repository_id)
            .fetch_one(&pool)
            .await
            .expect("seed GitHub Repository ID");
    sqlx::query(
        "INSERT INTO github_installation_repositories (project_id, github_installation_id, repository_id, github_repository_id, access_state, last_verified_at) VALUES ($1, $2, $3, $4, 'selected', now())",
    )
    .bind(seed.project_id)
    .bind(installation_row_id)
    .bind(seed.repository_id)
    .bind(github_repository_id)
    .execute(&pool)
    .await
    .expect("link GitHub installation Repository");

    let supervisors = SupervisorRegistry::new(&config).expect("M5 supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors: supervisors.clone(),
    };
    let application = router(state.clone());
    let webhooks = webhook_router(state);
    let webhook_health = webhooks
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .expect("build webhook-listener health request"),
        )
        .await
        .expect("webhook-listener health response");
    assert_eq!(webhook_health.status(), StatusCode::NOT_FOUND);
    let webhook_get = webhooks
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/integrations/github/webhook")
                .body(Body::empty())
                .expect("build webhook-listener GET request"),
        )
        .await
        .expect("webhook-listener GET response");
    assert_eq!(webhook_get.status(), StatusCode::METHOD_NOT_ALLOWED);
    let owner_cookie = login(&application, OWNER_LOGIN).await;
    let viewer_cookie = login(&application, VIEWER_LOGIN).await;
    let status_uri = format!("/api/v1/projects/{}/integrations/github", seed.project_id);
    let owner_status = get_json(&application, &owner_cookie, &status_uri).await;
    assert_eq!(owner_status.0, StatusCode::OK);
    assert_eq!(owner_status.1["configured"], true);
    assert_eq!(owner_status.1["apiVersion"], "2026-03-10");
    assert_eq!(owner_status.1["installation"]["state"], "available");
    assert_eq!(
        owner_status.1["installation"]["repositories"]
            .as_array()
            .expect("repository projection")
            .len(),
        1
    );
    let owner_text = owner_status.1.to_string();
    assert!(!owner_text.contains(private_key.to_string_lossy().as_ref()));
    assert!(!owner_text.contains(WEBHOOK_SECRET));
    let viewer_status = get_json(&application, &viewer_cookie, &status_uri).await;
    assert_eq!(viewer_status.0, StatusCode::FORBIDDEN);

    let body = serde_json::to_vec(&json!({
        "installation": { "id": INSTALLATION_ID },
        "repository": {
            "id": github_repository_id,
            "full_name": "Mischa-dev/agent-workspace-m0-spike"
        },
        "after": "0123456789012345678901234567890123456789",
        "before": "1111111111111111111111111111111111111111",
        "ignored_secret": "this-arbitrary-field-must-not-persist"
    }))
    .expect("serialize push webhook");
    let first = send_webhook(&webhooks, "delivery-m5-1", "push", &body, WEBHOOK_SECRET).await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(first.1["duplicate"], false);
    let duplicate = send_webhook(&webhooks, "delivery-m5-1", "push", &body, WEBHOOK_SECRET).await;
    assert_eq!(duplicate.0, StatusCode::OK);
    assert_eq!(duplicate.1["duplicate"], true);

    let changed_body = serde_json::to_vec(&json!({
        "installation": { "id": INSTALLATION_ID },
        "repository": {
            "id": github_repository_id,
            "full_name": "Mischa-dev/agent-workspace-m0-spike"
        },
        "after": "2222222222222222222222222222222222222222"
    }))
    .expect("serialize changed webhook");
    let mismatch = send_webhook(
        &webhooks,
        "delivery-m5-1",
        "push",
        &changed_body,
        WEBHOOK_SECRET,
    )
    .await;
    assert_eq!(mismatch.0, StatusCode::CONFLICT);

    let bad_signature = send_webhook(
        &webhooks,
        "delivery-m5-2",
        "push",
        &body,
        "a-different-high-entropy-webhook-secret",
    )
    .await;
    assert_eq!(bad_signature.0, StatusCode::UNAUTHORIZED);
    let unknown_event =
        send_webhook(&webhooks, "delivery-m5-3", "issues", &body, WEBHOOK_SECRET).await;
    assert_eq!(unknown_event.0, StatusCode::UNPROCESSABLE_ENTITY);

    let delivery_count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM github_webhook_deliveries")
            .fetch_one(&pool)
            .await
            .expect("delivery count");
    let job_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM github_jobs")
        .fetch_one(&pool)
        .await
        .expect("job count");
    let job_payload = sqlx::query_scalar::<_, String>(
        "SELECT payload::text FROM github_jobs WHERE webhook_delivery_id IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("bounded job payload");
    assert_eq!(delivery_count, 1);
    assert_eq!(job_count, 1);
    assert!(job_payload.contains("0123456789012345678901234567890123456789"));
    assert!(!job_payload.contains("this-arbitrary-field-must-not-persist"));

    let source_repository = temporary.path().join("publication-source");
    fs::create_dir(&source_repository).expect("create publication source");
    git(&source_repository, &["init", "--initial-branch=main"]);
    fs::write(source_repository.join("known.txt"), "base\n")
        .expect("write publication base fixture");
    git(&source_repository, &["add", "known.txt"]);
    git(
        &source_repository,
        &[
            "-c",
            "user.name=Agent Workspace Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "base",
        ],
    );
    let mirror = expected_repository_mirror_path(&data_root, seed.repository_id)
        .expect("managed publication mirror path");
    git(
        temporary.path(),
        &[
            "clone",
            "--mirror",
            source_repository.to_str().expect("UTF-8 source path"),
            mirror.to_str().expect("UTF-8 mirror path"),
        ],
    );
    let base_commit = git_output(&mirror, &["rev-parse", "refs/heads/main"], true);
    let workspace_id = Uuid::now_v7();
    let run_id = Uuid::now_v7();
    let branch = format!(
        "codex/run-m5-publish-{}",
        &run_id.simple().to_string()[..12]
    );
    let worktree = managed_worktree_path(&data_root, seed.repository_id, workspace_id)
        .expect("managed publication worktree path");
    create_worktree(&mirror, &worktree, &branch, &base_commit)
        .expect("create managed publication worktree");
    fs::write(worktree.join("known.txt"), "published\n").expect("write publication change");
    git(&worktree, &["add", "known.txt"]);
    git(
        &worktree,
        &[
            "-c",
            "user.name=Agent Workspace Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "publication candidate",
        ],
    );
    let intended_commit = git_output(&worktree, &["rev-parse", "HEAD"], false);
    sqlx::query(
        "UPDATE repositories SET mirror_path = $2, preparation_state = 'ready', prepared_at = now(), preparation_failure_category = NULL WHERE id = $1",
    )
    .bind(seed.repository_id)
    .bind(mirror.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .expect("point test Repository at managed mirror");
    sqlx::query(
        r#"
        INSERT INTO repository_workspaces (
            id, project_id, repository_id, conversation_id, branch_name,
            base_commit, state
        ) VALUES ($1, $2, $3, $4, $5, $6, 'retained')
        "#,
    )
    .bind(workspace_id)
    .bind(seed.project_id)
    .bind(seed.repository_id)
    .bind(seed.thread_id)
    .bind(&branch)
    .bind(&base_commit)
    .execute(&pool)
    .await
    .expect("insert publication Repository Workspace");
    sqlx::query(
        "INSERT INTO git_worktrees (id, repository_workspace_id, path, state) VALUES ($1, $2, $3, 'ready')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(worktree.to_string_lossy().as_ref())
    .execute(&pool)
    .await
    .expect("insert publication Git worktree");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, project_id, conversation_id, execution_worker_id,
            requesting_actor_id, execution_profile_id, repository_id,
            repository_workspace_id, state, prompt, authorization_snapshot,
            started_at, finished_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, 'completed',
            'M5 explicit publication fixture', '{}'::jsonb, now(), now()
        )
        "#,
    )
    .bind(run_id)
    .bind(seed.project_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .bind(workspace_id)
    .execute(&pool)
    .await
    .expect("insert terminal publication Agent Run");
    let csrf = csrf_from_cookie(&owner_cookie);
    let publish_uri = format!(
        "/api/v1/projects/{}/agent-runs/{run_id}/publish",
        seed.project_id
    );
    let publish = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&publish_uri)
                .header("origin", "http://127.0.0.1:3100")
                .header(header::COOKIE, &owner_cookie)
                .header("x-csrf-token", &csrf)
                .header("idempotency-key", "m5-explicit-publish")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .expect("build M5 publish request"),
        )
        .await
        .expect("M5 publish response");
    let publish = response_json(publish).await;
    assert_eq!(publish.0, StatusCode::ACCEPTED);
    assert_eq!(publish.1["agentRunId"], run_id.to_string());
    assert_eq!(publish.1["repositoryWorkspaceId"], workspace_id.to_string());
    assert_eq!(publish.1["intendedCommit"], intended_commit);
    let durable_publish = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String)>(
        "SELECT agent_run_id, repository_workspace_id, requesting_actor_id, intended_commit FROM github_jobs WHERE kind = 'workspace_publish'",
    )
    .fetch_one(&pool)
    .await
    .expect("durable workspace publication job");
    assert_eq!(durable_publish.0, run_id);
    assert_eq!(durable_publish.1, workspace_id);
    assert_eq!(durable_publish.2, seed.human_actor_id);
    assert_eq!(durable_publish.3, intended_commit);

    sqlx::query(
        r#"
        UPDATE repository_workspaces
        SET published_commit = $3, published_at = now(),
            published_by_actor_id = $4, publish_version = publish_version + 1
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(seed.project_id)
    .bind(workspace_id)
    .bind(&intended_commit)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("record exact published commit for pull-request request");
    let pull_request_uri = format!(
        "/api/v1/projects/{}/agent-runs/{run_id}/pull-request",
        seed.project_id
    );
    let pull_request = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&pull_request_uri)
                .header("origin", "http://127.0.0.1:3100")
                .header(header::COOKIE, &owner_cookie)
                .header("x-csrf-token", &csrf)
                .header("idempotency-key", "m5-explicit-pull-request")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title": "M5 exact publication",
                        "body": "Created from the exact managed commit.",
                        "workItemIds": []
                    }))
                    .expect("serialize pull-request command"),
                ))
                .expect("build M5 pull-request request"),
        )
        .await
        .expect("M5 pull-request response");
    let pull_request = response_json(pull_request).await;
    assert_eq!(pull_request.0, StatusCode::ACCEPTED);
    assert_eq!(pull_request.1["agentRunId"], run_id.to_string());
    assert_eq!(
        pull_request.1["repositoryWorkspaceId"],
        workspace_id.to_string()
    );
    assert_eq!(pull_request.1["headRef"], branch);
    assert_eq!(pull_request.1["headSha"], intended_commit);
    assert_eq!(pull_request.1["baseRef"], "main");
    let durable_pull_request = sqlx::query_as::<_, (Uuid, Uuid, String, String, String)>(
        r#"
        SELECT request.agent_run_id, request.repository_workspace_id,
               request.head_ref, request.head_sha, request.base_ref
        FROM github_pull_request_requests request
        JOIN github_jobs job ON job.pull_request_request_id = request.id
        WHERE job.kind = 'pull_request_create'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("durable pull-request creation job");
    assert_eq!(durable_pull_request.0, run_id);
    assert_eq!(durable_pull_request.1, workspace_id);
    assert_eq!(durable_pull_request.2, branch);
    assert_eq!(durable_pull_request.3, intended_commit);
    assert_eq!(durable_pull_request.4, "main");

    supervisors.shutdown_all().await;
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated M5 GitHub schema");
}

fn csrf_from_cookie(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|pair| pair.strip_prefix("agent_workspace_csrf="))
        .expect("M5 CSRF cookie")
        .to_owned()
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("run Git fixture command");
    assert!(status.success(), "Git fixture command failed: {args:?}");
}

fn git_output(cwd: &Path, args: &[&str], bare: bool) -> String {
    let mut command = Command::new("/usr/bin/git");
    if bare {
        command.arg("--git-dir").arg(cwd);
    } else {
        command.current_dir(cwd);
    }
    let output = command.args(args).output().expect("run Git fixture query");
    assert!(
        output.status.success(),
        "Git fixture query failed: {args:?}"
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8 Git fixture output")
        .trim()
        .to_owned()
}

fn write_owner_only(path: &std::path::Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .expect("create owner-only M5 secret fixture");
    file.write_all(bytes).expect("write M5 secret fixture");
}

async fn login(application: &axum::Router, login_name: &str) -> String {
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
                        "loginName": login_name,
                        "password": PASSWORD
                    }))
                    .expect("serialize M5 login request"),
                ))
                .expect("build M5 login request"),
        )
        .await
        .expect("M5 login response");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("M5 cookie header")
                .split(';')
                .next()
                .expect("M5 cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("; ")
}

async fn get_json(application: &axum::Router, cookie: &str, uri: &str) -> (StatusCode, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .expect("build M5 GET request"),
        )
        .await
        .expect("M5 GET response");
    response_json(response).await
}

async fn send_webhook(
    webhooks: &axum::Router,
    delivery_id: &str,
    event: &str,
    body: &[u8],
    secret: &str,
) -> (StatusCode, Value) {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let response = webhooks
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/integrations/github/webhook")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-hub-signature-256", signature)
                .header("x-github-delivery", delivery_id)
                .header("x-github-event", event)
                .body(Body::from(body.to_vec()))
                .expect("build M5 webhook request"),
        )
        .await
        .expect("M5 webhook response");
    response_json(response).await
}

async fn response_json(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read M5 response");
    let value = serde_json::from_slice(&bytes).expect("JSON M5 response");
    (status, value)
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
        .expect("connect schema-scoped M5 GitHub pool")
}

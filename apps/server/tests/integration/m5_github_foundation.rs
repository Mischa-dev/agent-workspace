use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::Command,
    sync::Arc,
    time::Duration,
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

    let automation_uri = format!(
        "/api/v1/projects/{}/integrations/github/automation",
        seed.project_id
    );
    let automation = get_json(&application, &owner_cookie, &automation_uri).await;
    assert_eq!(automation.0, StatusCode::OK);
    assert_eq!(automation.1["pullRequestOpenEnabled"], false);
    assert_eq!(automation.1["pullRequestMergeEnabled"], true);
    assert_eq!(automation.1["version"], 1);
    let viewer_automation = get_json(&application, &viewer_cookie, &automation_uri).await;
    assert_eq!(viewer_automation.0, StatusCode::FORBIDDEN);
    let update_body = serde_json::to_vec(&json!({
        "expectedVersion": 1,
        "pullRequestOpenEnabled": true,
        "pullRequestMergeEnabled": true
    }))
    .expect("serialize automation settings update");
    let update_automation = || {
        Request::builder()
            .method("PUT")
            .uri(&automation_uri)
            .header("origin", "http://127.0.0.1:3100")
            .header(header::COOKIE, &owner_cookie)
            .header("x-csrf-token", csrf_from_cookie(&owner_cookie))
            .header("idempotency-key", "m5-automation-settings")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(update_body.clone()))
            .expect("build automation settings update")
    };
    let updated = response_json(
        application
            .clone()
            .oneshot(update_automation())
            .await
            .expect("automation settings update response"),
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK);
    assert_eq!(updated.1["pullRequestOpenEnabled"], true);
    assert_eq!(updated.1["pullRequestMergeEnabled"], true);
    assert_eq!(updated.1["version"], 2);
    let replayed = response_json(
        application
            .clone()
            .oneshot(update_automation())
            .await
            .expect("automation settings replay response"),
    )
    .await;
    assert_eq!(replayed, updated);
    let settings_audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE action = 'github.automation_settings_update'",
    )
    .fetch_one(&pool)
    .await
    .expect("automation settings audit count");
    assert_eq!(settings_audits, 1);

    let body = serde_json::to_vec(&json!({
        "installation": { "id": INSTALLATION_ID },
        "repository": {
            "id": github_repository_id,
            "full_name": format!(
                "{}/{}",
                agent_workspace_server::seed::DEMO_REPOSITORY_OWNER,
                agent_workspace_server::seed::DEMO_REPOSITORY_NAME
            )
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

    sqlx::query(
        r#"
        UPDATE github_jobs
        SET state = 'failed', attempts = 5,
            failure_category = 'job_processing_failed', next_attempt_at = now()
        WHERE webhook_delivery_id IS NOT NULL
        "#,
    )
    .execute(&pool)
    .await
    .expect("simulate failed legacy webhook job");
    sqlx::query(
        r#"
        UPDATE github_webhook_deliveries
        SET state = 'failed', failure_category = 'job_processing_failed',
            processed_at = NULL
        WHERE delivery_id = 'delivery-m5-1'
        "#,
    )
    .execute(&pool)
    .await
    .expect("simulate failed legacy delivery");
    let (retried_a, retried_b) = tokio::join!(
        send_webhook(&webhooks, "delivery-m5-1", "push", &body, WEBHOOK_SECRET),
        send_webhook(&webhooks, "delivery-m5-1", "push", &body, WEBHOOK_SECRET),
    );
    for retried in [retried_a, retried_b] {
        assert_eq!(retried.0, StatusCode::OK);
        assert_eq!(retried.1["duplicate"], true);
    }
    let retried_job = sqlx::query_as::<_, (String, i32, Value)>(
        r#"
        SELECT job.state, job.attempts, delivery.redelivery_payload
        FROM github_jobs job
        JOIN github_webhook_deliveries delivery ON delivery.id = job.webhook_delivery_id
        WHERE job.webhook_delivery_id IS NOT NULL
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("requeued webhook job");
    assert_eq!(retried_job.0, "queued");
    assert_eq!(retried_job.1, 0);
    assert_eq!(
        retried_job.2["after"],
        "0123456789012345678901234567890123456789"
    );

    let changed_body = serde_json::to_vec(&json!({
        "installation": { "id": INSTALLATION_ID },
        "repository": {
            "id": github_repository_id,
            "full_name": format!(
                "{}/{}",
                agent_workspace_server::seed::DEMO_REPOSITORY_OWNER,
                agent_workspace_server::seed::DEMO_REPOSITORY_NAME
            )
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
    drop_isolated_schema(&administrative_pool, &schema).await;
}

async fn drop_isolated_schema(administrative_pool: &PgPool, schema: &str) {
    const MAX_ATTEMPTS: u32 = 5;

    for attempt in 1..=MAX_ATTEMPTS {
        match agent_workspace_server::test_support::drop_schema(administrative_pool, schema).await {
            Ok(_) => return,
            Err(sqlx::Error::Database(error))
                if error.code().as_deref() == Some("40P01") && attempt < MAX_ATTEMPTS =>
            {
                tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
            }
            Err(error) => panic!("drop isolated M5 GitHub schema: {error}"),
        }
    }

    unreachable!("the final schema-drop attempt either returns or panics");
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

/// P0.2: a database error in a pass doesn't end the GitHub job worker.
#[tokio::test]
async fn github_jobs_keep_running_through_a_database_error() {
    use super::m3_scheduler_http::{background_state, survives_a_missing_table};

    let Some((state, admin, schema)) = background_state("github_jobs_blip").await else {
        return;
    };
    survives_a_missing_table(state.clone(), "github_jobs", |state, shutdown| {
        agent_workspace_server::github::run_jobs(state, shutdown)
    })
    .await;
    state.pool.close().await;
    agent_workspace_server::test_support::drop_schema(&admin, &schema)
        .await
        .expect("drop schema");
}

/// P0.11: a merged pull request moves its linked task through the task
/// command, as a person's move would: an activity that says what changed,
/// the assigned agent's event, a watching agent's note, the card at the
/// bottom of its new column, and `changedFields` on the webhook.
#[tokio::test]
async fn a_merged_pull_request_moves_its_task_like_a_person_would() {
    use super::{
        agent_interface::{connect_app, create_agent, initialize, tool},
        mcp_code::seeded,
        mcp_task_board::seed_pull_request,
        mcp_tasks::{add_member, space},
        message_edits::owner_id,
    };
    use agent_workspace_server::{
        github::{AutomationJob, apply_pull_request_automation},
        realtime::publish_ready_outbox_batch,
    };
    let Some((setup, seed, _temporary)) = seeded("p11_github_moves").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let project = seed.project_id;
    let aw = project.to_string();
    let workspace = space(&setup).await;
    let (mara, _) = create_agent(app, &setup.browser, &workspace, "Mara", "mara").await;
    let (wes, _) = create_agent(app, &setup.browser, &workspace, "Wes", "wes").await;
    add_member(&setup, &aw, mara, "member").await;
    add_member(&setup, &aw, wes, "member").await;
    let owner = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &owner).await.expect("owner's app");
    let mut tasks = Vec::new();
    for title in ["Ship offline mode", "Already in verification"] {
        let created = tool(
            app,
            &owner,
            &session,
            "task_create",
            json!({"projectId": aw, "title": title}),
        )
        .await;
        tasks.push(
            created["workItemId"]
                .as_str()
                .unwrap_or_else(|| panic!("task: {created}"))
                .parse::<Uuid>()
                .expect("task id"),
        );
    }
    let (task, settled) = (tasks[0], tasks[1]);
    // A status with a transition into the one verification status.
    let (from, to) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT transition.from_status_id, transition.to_status_id
        FROM work_item_status_transitions transition
        JOIN work_item_statuses target
          ON target.project_id = transition.project_id AND target.id = transition.to_status_id
        WHERE transition.project_id = $1 AND target.semantic_category = 'verification'
          AND target.enabled
        ORDER BY transition.from_status_id
        LIMIT 1
        "#,
    )
    .bind(project)
    .fetch_one(pool)
    .await
    .expect("a transition into verification");
    for (id, status, assignee) in [(task, from, Some(mara)), (settled, to, None)] {
        sqlx::query("UPDATE work_items SET status_id = $2, assignee_actor_id = $3 WHERE id = $1")
            .bind(id)
            .bind(status)
            .bind(assignee)
            .execute(pool)
            .await
            .expect("set the task up");
    }
    sqlx::query(
        "INSERT INTO work_item_watchers (project_id, work_item_id, actor_id) VALUES ($1, $2, $3)",
    )
    .bind(project)
    .bind(task)
    .bind(wes)
    .execute(pool)
    .await
    .expect("Wes watches");
    let owner_actor = owner_id(pool).await;
    sqlx::query(
        r#"
        INSERT INTO webhook_subscriptions (
            id, organization_id, project_id, owner_actor_id, name, url, event_types
        ) VALUES ($1, $2, $3, $4, 'Moves', 'https://127.0.0.1/hook', '{task.updated}')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(setup.organization_id)
    .bind(project)
    .bind(owner_actor)
    .execute(pool)
    .await
    .expect("webhook");
    let (pull_request, _) = seed_pull_request(&setup, &aw, seed.repository_id, 11).await;
    let (integration, version) = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT installation.integration_actor_id, pr.version FROM github_pull_requests pr JOIN github_installations installation ON installation.id = pr.github_installation_id WHERE pr.id = $1",
    )
    .bind(pull_request)
    .fetch_one(pool)
    .await
    .expect("integration actor");
    sqlx::query(
        "INSERT INTO github_pull_request_work_item_links (project_id, github_pull_request_id, work_item_id, source, confirmed, created_by_actor_id) VALUES ($1, $2, $3, 'explicit', true, $4)",
    )
    .bind(project)
    .bind(pull_request)
    .bind(task)
    .bind(owner_actor)
    .execute(pool)
    .await
    .expect("link");
    // Leave earlier events behind.
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let config = AppConfig::from_values(&BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url,
    )]))
    .expect("configuration");
    let state = AppState {
        supervisors: SupervisorRegistry::new(&config).expect("supervisors"),
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(64),
    };
    let pump = || async {
        let registry = agent_workspace_server::jobs::default_registry();
        let options = agent_workspace_server::jobs::WorkerOptions {
            concurrency: 1,
            poll_interval: Duration::from_millis(20),
            lease: Duration::from_secs(30),
            worker_id: "p11-test".to_owned(),
        };
        for _ in 0..10 {
            let published = publish_ready_outbox_batch(pool, &state.realtime, "p11-test")
                .await
                .expect("publish outbox");
            let ran = agent_workspace_server::jobs::run_due(&state, &registry, &options)
                .await
                .expect("run jobs");
            if published == 0 && ran == 0 {
                return;
            }
        }
    };
    pump().await;
    sqlx::query("DELETE FROM webhook_deliveries")
        .execute(pool)
        .await
        .expect("clear");

    apply_pull_request_automation(
        pool,
        AutomationJob {
            job_id: Uuid::now_v7(),
            project_id: project,
            pull_request_id: pull_request,
            integration_actor_id: integration,
        },
        version,
        "merged",
        false,
    )
    .await
    .expect("automation");

    let (status, kind, changed) = sqlx::query_as::<_, (Uuid, String, Value)>(
        r#"
        SELECT item.status_id, activity.kind, activity.payload->'changedFields'
        FROM task_rows item
        JOIN work_item_activity activity ON activity.work_item_id = item.id
        WHERE item.id = $1 AND activity.actor_id = $2
        "#,
    )
    .bind(task)
    .bind(integration)
    .fetch_one(pool)
    .await
    .expect("the move's activity");
    assert_eq!(status, to);
    assert_eq!(kind, "work_item_updated");
    assert_eq!(changed, json!(["status", "position"]));
    let ranks = sqlx::query_scalar::<_, i64>(
        "SELECT board_rank FROM work_items WHERE id = ANY($1) ORDER BY array_position($1, id)",
    )
    .bind(vec![settled, task])
    .fetch_all(pool)
    .await
    .expect("ranks");
    assert!(ranks[1] > ranks[0], "the card is at the bottom: {ranks:?}");
    let event_count = |agent: Uuid| async move {
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*) FROM external_agent_events event
            JOIN external_agent_connections connection ON connection.id = event.connection_id
            WHERE connection.actor_id = $1 AND event.kind = 'work_item.changed'
              AND event.created_at > now() - interval '1 minute'
              AND event.summary IN ('An assigned Work Item changed', 'A Work Item you watch changed')
            "#,
        )
        .bind(agent)
        .fetch_one(pool)
        .await
        .expect("events")
    };
    assert!(
        event_count(mara).await >= 1,
        "the assigned agent hears of it"
    );
    assert!(
        event_count(wes).await >= 1,
        "the watching agent hears of it"
    );
    pump().await;
    let subjects = sqlx::query_scalar::<_, Value>(
        "SELECT subject FROM webhook_deliveries WHERE event_type = 'task.updated'",
    )
    .fetch_all(pool)
    .await
    .expect("deliveries");
    assert_eq!(subjects.len(), 1, "{subjects:?}");
    // Routing lists what changed in name order.
    assert_eq!(subjects[0]["changedFields"], json!(["position", "status"]));
}

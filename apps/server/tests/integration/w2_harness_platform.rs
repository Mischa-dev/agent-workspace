//! W2: a person's provider account runs several agents at once, run settings
//! are resolved once and frozen, and accounts and the model catalog stay with
//! the people allowed to change them.

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    auth::password,
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
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool};
use tokio::time::{Duration, sleep};
use tower::ServiceExt;
use uuid::Uuid;

use crate::m3_scheduler_http::{
    create_test_mirror, integration_pool, interrupt, response_json, schema_pool, start_run_body,
    uuid_field, wait_for_state,
};

const ORIGIN: &str = "http://127.0.0.1:3100";
const OWNER_LOGIN: &str = "w2-harness-owner";
const MEMBER_LOGIN: &str = "w2-harness-member";
const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn accounts_run_concurrently_with_frozen_settings_and_stay_with_their_owner() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("w2_harness_platform_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create harness schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run harness migrations");

    let temporary = tempfile::tempdir().expect("temporary harness root");
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
            "8".to_owned(),
        ),
    ]))
    .expect("harness configuration");
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed harness fixture");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare harness Repository");
    let mirror = expected_repository_mirror_path(&data_root, seed.repository_id)
        .expect("canonical harness mirror");
    create_test_mirror(temporary.path(), &mirror);
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OWNER_LOGIN.to_owned(),
            display_name: "W2 Harness Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap harness owner");
    insert_member(&pool, seed.organization_id, seed.project_id).await;
    let threads = [
        seed.thread_id,
        insert_thread(&pool, &seed, "Second run").await,
        insert_thread(&pool, &seed, "Third run").await,
    ];

    let supervisors = SupervisorRegistry::new(&config).expect("harness supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors: supervisors.clone(),
    };
    let application = router(state.clone());
    let (owner_cookie, owner_csrf) = login(&application, OWNER_LOGIN).await;
    let (member_cookie, member_csrf) = login(&application, MEMBER_LOGIN).await;
    let owner = (owner_cookie.as_str(), owner_csrf.as_str());
    let member = (member_cookie.as_str(), member_csrf.as_str());
    let accounts = format!("/api/v1/projects/{}/provider-accounts", seed.project_id);

    // The owner adds a demo account, signs it in and uses it in the project.
    let created = send(
        &application,
        owner,
        "POST",
        &accounts,
        "create-demo",
        json!({"adapterKind": "demo"}),
    )
    .await;
    assert!(created.0.is_success(), "create account: {}", created.1);
    assert_eq!(created.2["accountState"], "authentication_required");
    let account_id = uuid_field(&created.2, "id");
    let account = format!("{accounts}/{account_id}");
    let signed_in = send(
        &application,
        owner,
        "POST",
        &format!("{account}/login/api-key"),
        "demo-sign-in",
        json!({"apiKey": "demo-key-for-tests"}),
    )
    .await;
    assert!(signed_in.0.is_success(), "sign in: {}", signed_in.1);
    assert!(
        !signed_in.1.contains("demo-key-for-tests"),
        "the key never comes back"
    );
    let used = send(
        &application,
        owner,
        "POST",
        &format!("{account}/use"),
        "use-demo",
        json!({}),
    )
    .await;
    assert!(used.0.is_success(), "use account: {}", used.1);
    assert_eq!(used.2["preferred"], true);
    let profile_id = uuid_field(&used.2, "profileId");
    let stored_key = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM provider_accounts WHERE run_defaults::text LIKE '%demo-key%' OR home_path LIKE '%demo-key%'",
    )
    .fetch_one(&pool)
    .await
    .expect("look for a stored key");
    assert_eq!(stored_key, 0);

    // Someone else in the project can't see, change or use it.
    let listed = send_get(&application, member, &accounts).await;
    assert_eq!(listed.0, StatusCode::OK);
    assert!(!listed.1.contains(&account_id.to_string()));
    for (method, path, body) in [
        (
            "PATCH",
            account.clone(),
            json!({"expectedVersion": 1, "maxConcurrentRuns": 8}),
        ),
        ("POST", format!("{account}/use"), json!({})),
        ("POST", format!("{account}/logout"), json!({})),
    ] {
        let refused = send(
            &application,
            member,
            method,
            &path,
            &format!("member-{path}"),
            body,
        )
        .await;
        assert!(
            matches!(refused.0, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{method} {path} by another member returned {}",
            refused.0
        );
    }

    // Two runs at once on this account, then a third waits for a free slot.
    let version = used.2["version"].as_i64().expect("account version");
    let defaults = send(
        &application,
        owner,
        "PATCH",
        &account,
        "demo-defaults",
        json!({
            "expectedVersion": version,
            "maxConcurrentRuns": 2,
            "runDefaults": {"model": "demo-large", "reasoning": "high"}
        }),
    )
    .await;
    assert_eq!(defaults.0, StatusCode::OK, "{}", defaults.1);
    // A retry whose first response was lost replays rather than conflicting.
    let retried = send(
        &application,
        owner,
        "PATCH",
        &account,
        "demo-defaults",
        json!({
            "expectedVersion": version,
            "maxConcurrentRuns": 2,
            "runDefaults": {"model": "demo-large", "reasoning": "high"}
        }),
    )
    .await;
    assert_eq!(retried.0, StatusCode::OK, "{}", retried.1);
    assert_eq!(retried.2["version"], defaults.2["version"]);
    let first = start(
        &application,
        owner,
        threads[0],
        "first",
        json!({"prompt": "one", "config": {"model": "demo-small"}}),
    )
    .await;
    let second = start(
        &application,
        owner,
        threads[1],
        "second",
        json!({"prompt": "two"}),
    )
    .await;
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("admit two runs");
    wait_for_state(&pool, first, "blocked").await;
    wait_for_state(&pool, second, "blocked").await;
    let third = start(
        &application,
        owner,
        threads[2],
        "third",
        json!({"prompt": "three"}),
    )
    .await;
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("leave the third run queued");
    sleep(Duration::from_millis(300)).await;
    assert_eq!(run_state(&pool, third).await, "queued");

    // Settings were resolved at launch, frozen, and reached the harness.
    let first_config = run_config(&pool, first).await;
    assert_eq!(first_config["model"], "demo-small");
    let second_config = run_config(&pool, second).await;
    assert_eq!(second_config["model"], "demo-large");
    assert_eq!(second_config["reasoning"], "high");
    assert!(
        sqlx::query("UPDATE agent_runs SET config = '{\"model\":\"demo-large\"}' WHERE id = $1")
            .bind(first)
            .execute(&pool)
            .await
            .is_err(),
        "a run's settings can't change once set"
    );
    for run in [first, second] {
        assert_eq!(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT execution_profile_id FROM agent_runs WHERE id = $1"
            )
            .bind(run)
            .fetch_one(&pool)
            .await
            .expect("run profile"),
            profile_id
        );
    }
    let received = supervisors
        .demo()
        .expect("demo harness")
        .turns(account_id)
        .into_iter()
        .map(|turn| turn.config.model)
        .collect::<Vec<_>>();
    assert!(
        received.contains(&Some("demo-small".to_owned())),
        "{received:?}"
    );
    assert!(
        received.contains(&Some("demo-large".to_owned())),
        "{received:?}"
    );

    assert_eq!(
        interrupt(&application, owner.0, owner.1, first).await.0,
        StatusCode::OK
    );
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("admit the third run");
    wait_for_state(&pool, third, "blocked").await;

    // Only workspace admins edit the catalog, and hidden models can't be picked.
    let model = format!(
        "/api/v1/projects/{}/harnesses/demo/models/demo-small",
        seed.project_id
    );
    let member_edit = send(
        &application,
        member,
        "PUT",
        &model,
        "member-hide",
        json!({"hidden": true}),
    )
    .await;
    assert_eq!(member_edit.0, StatusCode::FORBIDDEN, "{}", member_edit.1);
    let member_refresh = send(
        &application,
        member,
        "POST",
        &format!(
            "/api/v1/projects/{}/harnesses/demo/catalog/refresh",
            seed.project_id
        ),
        "member-refresh",
        json!({}),
    )
    .await;
    assert_eq!(
        member_refresh.0,
        StatusCode::FORBIDDEN,
        "{}",
        member_refresh.1
    );
    let inconsistent = send(
        &application,
        owner,
        "PUT",
        &model,
        "owner-inconsistent",
        json!({"reasoningLevels": ["low"], "defaultReasoning": "high"}),
    )
    .await;
    assert_eq!(
        inconsistent.0,
        StatusCode::BAD_REQUEST,
        "{}",
        inconsistent.1
    );
    let harnesses = send_get(
        &application,
        member,
        &format!("/api/v1/projects/{}/harnesses", seed.project_id),
    )
    .await;
    assert_eq!(harnesses.0, StatusCode::OK);
    let demo = harnesses.2["harnesses"]
        .as_array()
        .and_then(|list| list.iter().find(|harness| harness["kind"] == "demo"))
        .expect("demo harness listed");
    assert_eq!(demo["canEditCatalog"], false);
    // Choosing a default replaces the previous choice: one default at a time.
    for (chosen, key) in [
        ("demo-small", "default-small"),
        ("demo-large", "default-large"),
    ] {
        let path = format!(
            "/api/v1/projects/{}/harnesses/demo/models/{chosen}",
            seed.project_id
        );
        let saved = send(
            &application,
            owner,
            "PUT",
            &path,
            key,
            json!({"isDefault": true}),
        )
        .await;
        assert_eq!(saved.0, StatusCode::OK, "{}", saved.1);
        let defaults = saved.2["catalog"]["models"]
            .as_array()
            .expect("catalog models")
            .iter()
            .filter(|model| model["isDefault"] == true)
            .map(|model| model["id"].clone())
            .collect::<Vec<_>>();
        assert_eq!(defaults, vec![json!(chosen)]);
    }
    let hidden = send(
        &application,
        owner,
        "PUT",
        &model,
        "owner-hide",
        json!({"hidden": true}),
    )
    .await;
    assert_eq!(hidden.0, StatusCode::OK, "{}", hidden.1);
    interrupt(&application, owner.0, owner.1, second).await;
    interrupt(&application, owner.0, owner.1, third).await;
    let refused = start_body(
        &application,
        owner,
        threads[0],
        "hidden-model",
        json!({"prompt": "again", "config": {"model": "demo-small"}}),
    )
    .await;
    assert_eq!(refused.0, StatusCode::BAD_REQUEST, "{}", refused.1);

    // A second account for the same harness gets its own profile here.
    let second_account = send(
        &application,
        owner,
        "POST",
        &accounts,
        "create-second-demo",
        json!({"adapterKind": "demo"}),
    )
    .await;
    assert!(second_account.0.is_success(), "{}", second_account.1);
    let second_used = send(
        &application,
        owner,
        "POST",
        &format!("{accounts}/{}/use", uuid_field(&second_account.2, "id")),
        "use-second-demo",
        json!({}),
    )
    .await;
    assert!(second_used.0.is_success(), "use second: {}", second_used.1);
    assert_ne!(uuid_field(&second_used.2, "profileId"), profile_id);

    // Managers see a teammate's profile but not the private account behind it.
    sqlx::query("UPDATE project_memberships SET role = 'admin' WHERE handle = 'w2-member'")
        .execute(&pool)
        .await
        .expect("promote member");
    let member_account = send(
        &application,
        member,
        "POST",
        &accounts,
        "member-demo",
        json!({"adapterKind": "demo"}),
    )
    .await;
    assert!(member_account.0.is_success(), "{}", member_account.1);
    let member_account_id = uuid_field(&member_account.2, "id");
    let member_used = send(
        &application,
        member,
        "POST",
        &format!("{accounts}/{member_account_id}/use"),
        "member-use-demo",
        json!({}),
    )
    .await;
    assert!(member_used.0.is_success(), "{}", member_used.1);
    let profiles = send_get(
        &application,
        owner,
        &format!("/api/v1/projects/{}/execution-profiles", seed.project_id),
    )
    .await;
    assert_eq!(profiles.0, StatusCode::OK, "{}", profiles.1);
    assert!(!profiles.1.contains(&member_account_id.to_string()));
    assert!(
        profiles.1.contains(&account_id.to_string()),
        "owners see their own"
    );

    // Signing out sticks, even though it stops the account's harness sessions.
    let signed_out = send(
        &application,
        owner,
        "POST",
        &format!("{account}/logout"),
        "sign-out",
        json!({}),
    )
    .await;
    assert!(signed_out.0.is_success(), "sign out: {}", signed_out.1);
    // A retry (say, after a lost response) still tells the harness.
    let signed_out_again = send(
        &application,
        owner,
        "POST",
        &format!("{account}/logout"),
        "sign-out-again",
        json!({}),
    )
    .await;
    assert!(signed_out_again.0.is_success(), "{}", signed_out_again.1);
    let status = send_get(&application, owner, &account).await;
    assert_eq!(status.0, StatusCode::OK, "{}", status.1);
    assert_ne!(status.2["accountState"], "authenticated");

    supervisors.shutdown_all().await;
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop harness schema");
}

async fn insert_member(pool: &PgPool, organization_id: Uuid, project_id: Uuid) {
    let actor_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let password_phc = password::hash_password(PASSWORD.to_owned())
        .await
        .expect("hash member password");
    let mut transaction = pool.begin().await.expect("member transaction");
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor_id)
        .execute(&mut *transaction)
        .await
        .expect("insert member Actor");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, $3, 'W2 Member')",
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(MEMBER_LOGIN)
    .execute(&mut *transaction)
    .await
    .expect("insert member User");
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(user_id)
        .bind(password_phc)
        .execute(&mut *transaction)
        .await
        .expect("insert member password");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(&mut *transaction)
    .await
    .expect("insert member Organization membership");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
        VALUES ($1, $2, $3, 'member', 'W2 Member', 'w2-member')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&mut *transaction)
    .await
    .expect("insert member Project membership");
    transaction.commit().await.expect("commit member");
}

async fn insert_thread(
    pool: &PgPool,
    seed: &agent_workspace_server::seed::DevSeed,
    name: &str,
) -> Uuid {
    let thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
        VALUES ($1, $2, 'thread', $3, $4, $5)
        "#,
    )
    .bind(thread_id)
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .bind(seed.repository_id)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert harness Thread");
    thread_id
}

async fn start(
    application: &axum::Router,
    session: (&str, &str),
    thread_id: Uuid,
    key: &str,
    body: Value,
) -> Uuid {
    let response = start_body(application, session, thread_id, key, body).await;
    assert_eq!(response.0, StatusCode::ACCEPTED, "{}", response.1);
    uuid_field(&response.2, "runId")
}

async fn start_body(
    application: &axum::Router,
    (cookie, csrf): (&str, &str),
    thread_id: Uuid,
    key: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    start_run_body(application, cookie, csrf, thread_id, key, body).await
}

async fn run_state(pool: &PgPool, run_id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("read run state")
}

async fn run_config(pool: &PgPool, run_id: Uuid) -> Value {
    sqlx::query_scalar("SELECT config FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("read run config")
}

async fn send(
    application: &axum::Router,
    (cookie, csrf): (&str, &str),
    method: &str,
    path: &str,
    key: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let key = key.replace(|character: char| !character.is_ascii_alphanumeric(), "-");
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("origin", ORIGIN)
                    .header("cookie", cookie)
                    .header("x-csrf-token", csrf)
                    .header("idempotency-key", key)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&body).expect("serialize body"),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("response"),
    )
    .await
}

async fn send_get(
    application: &axum::Router,
    (cookie, _): (&str, &str),
    path: &str,
) -> (StatusCode, String, Value) {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response"),
    )
    .await
}

async fn login(application: &axum::Router, login_name: &str) -> (String, String) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"loginName": login_name, "password": PASSWORD}))
                        .expect("serialize login"),
                ))
                .expect("build login"),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK, "login {login_name}");
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|value| value.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    (cookies.join("; "), csrf)
}

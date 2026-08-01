use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
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

const LOGIN_NAME: &str = "m4-work-item-owner";
const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn work_item_foundation_is_scoped_idempotent_ordered_and_actor_attributed() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m4_work_items_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M4 Work Item schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M4 Work Item migrations");

    let temporary = tempfile::tempdir().expect("temporary M4 runtime root");
    let data_root = temporary.path().join("data");
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
            temporary.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M4 Work Item test configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M4 Work Item fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M4 Work Item Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M4 Work Item owner");

    let supervisors = SupervisorRegistry::new(&config).expect("M4 supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors: supervisors.clone(),
    });
    let base = format!("/api/v1/projects/{}/work-items", seed.project_id);

    let unauthenticated = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(&base)
                .body(Body::empty())
                .expect("build unauthenticated Work Item request"),
        )
        .await
        .expect("unauthenticated Work Item response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let cookie = login(&application).await;
    let csrf = csrf_from_cookie(&cookie);
    let workflow = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{}/work-item-workflow", seed.project_id),
    )
    .await;
    assert_eq!(workflow.0, StatusCode::OK);
    assert_eq!(
        workflow.2["types"]
            .as_array()
            .expect("workflow types array")
            .len(),
        6
    );
    assert_eq!(
        workflow.2["statuses"]
            .as_array()
            .expect("workflow statuses array")
            .len(),
        7
    );
    assert_eq!(
        workflow.2["transitions"]
            .as_array()
            .expect("workflow transitions array")
            .len(),
        17
    );
    assert_eq!(
        workflow.2["types"]
            .as_array()
            .expect("workflow types array")
            .iter()
            .filter(|item| item["isDefault"] == true)
            .count(),
        1
    );
    let status_id = |name: &str| {
        workflow.2["statuses"]
            .as_array()
            .expect("workflow statuses array")
            .iter()
            .find(|status| status["name"] == name)
            .and_then(|status| status["id"].as_str())
            .map(str::to_owned)
            .expect("named workflow status")
    };
    let ready_status_id = status_id("Ready");
    let done_status_id = status_id("Done");
    assert_eq!(
        workflow.2["statuses"]
            .as_array()
            .expect("workflow statuses array")
            .iter()
            .filter(|item| item["isDefault"] == true)
            .count(),
        1
    );

    let request = json!({
        "title": "First Work Item",
        "description": "A bounded description.",
        "priority": "high",
        "assigneeActorId": seed.human_actor_id
    });
    let missing_csrf = post_json(
        &application,
        &cookie,
        None,
        Some("m4-create-one"),
        &base,
        request.clone(),
    )
    .await;
    assert_eq!(missing_csrf.0, StatusCode::FORBIDDEN);

    let created = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-create-one"),
        &base,
        request.clone(),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    assert_eq!(created.2["key"], "AW-1");
    assert_eq!(created.2["number"], 1);
    assert_eq!(created.2["priority"], "high");
    assert_eq!(
        created.2["assignee"]["actorId"],
        seed.human_actor_id.to_string()
    );
    assert_eq!(created.2["version"], 1);
    assert!(created.2.get("createdByActorId").is_none());

    let by_key = get_json(
        &application,
        &cookie,
        &format!(
            "/api/v1/projects/{}/work-items/by-key/AW-1",
            seed.project_id
        ),
    )
    .await;
    assert_eq!(by_key.0, StatusCode::OK);
    assert_eq!(by_key.2["id"], created.2["id"]);

    let assignees = get_json(
        &application,
        &cookie,
        &format!(
            "/api/v1/projects/{}/work-item-assignees?q=M2",
            seed.project_id
        ),
    )
    .await;
    assert_eq!(assignees.0, StatusCode::OK);
    assert_eq!(
        assignees.2["actors"].as_array().expect("assignees").len(),
        1
    );
    assert!(assignees.2["actors"][0].get("email").is_none());

    let replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-create-one"),
        &base,
        request,
    )
    .await;
    assert_eq!(replay.0, StatusCode::CREATED);
    assert_eq!(replay.2["id"], created.2["id"]);

    let mismatch = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-create-one"),
        &base,
        json!({"title": "Conflicting payload"}),
    )
    .await;
    assert_eq!(mismatch.0, StatusCode::CONFLICT);

    let forbidden_transition = patch_json(
        &application,
        &cookie,
        &csrf,
        "m4-invalid-transition",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({"expectedVersion": 1, "statusId": done_status_id}),
    )
    .await;
    assert_eq!(forbidden_transition.0, StatusCode::CONFLICT);

    let updated = patch_json(
        &application,
        &cookie,
        &csrf,
        "m4-update-one",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({
            "expectedVersion": 1,
            "title": "First Work Item updated",
            "statusId": ready_status_id,
            "priority": "urgent",
            "assigneeActorId": null
        }),
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK, "{}", updated.1);
    assert_eq!(updated.2["version"], 2);
    assert_eq!(updated.2["status"]["name"], "Ready");
    assert_eq!(updated.2["priority"], "urgent");
    assert!(updated.2["assignee"].is_null());

    let update_replay = patch_json(
        &application,
        &cookie,
        &csrf,
        "m4-update-one",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({
            "expectedVersion": 1,
            "title": "First Work Item updated",
            "statusId": ready_status_id,
            "priority": "urgent",
            "assigneeActorId": null
        }),
    )
    .await;
    assert_eq!(update_replay.0, StatusCode::OK);
    assert_eq!(update_replay.2["version"], 2);

    let stale_update = patch_json(
        &application,
        &cookie,
        &csrf,
        "m4-stale-update",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({"expectedVersion": 1, "title": "Lose this edit"}),
    )
    .await;
    assert_eq!(stale_update.0, StatusCode::CONFLICT);
    assert_eq!(stale_update.2["error"]["code"], "version_conflict");
    assert_eq!(stale_update.2["error"]["details"]["currentVersion"], 2);
    assert_eq!(
        stale_update.2["error"]["details"]["current"]["title"],
        "First Work Item updated"
    );

    let worker_assignee = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-worker-assignee"),
        &base,
        json!({
            "title": "Invalid assignee",
            "assigneeActorId": seed.worker_actor_id
        }),
    )
    .await;
    assert_eq!(worker_assignee.0, StatusCode::BAD_REQUEST);

    let application_one = application.clone();
    let application_two = application.clone();
    let cookie_one = cookie.clone();
    let cookie_two = cookie.clone();
    let csrf_one = csrf.clone();
    let csrf_two = csrf.clone();
    let base_one = base.clone();
    let base_two = base.clone();
    let (second, third) = tokio::join!(
        post_json(
            &application_one,
            &cookie_one,
            Some(&csrf_one),
            Some("m4-create-two"),
            &base_one,
            json!({"title": "Second Work Item"}),
        ),
        post_json(
            &application_two,
            &cookie_two,
            Some(&csrf_two),
            Some("m4-create-three"),
            &base_two,
            json!({"title": "Third Work Item"}),
        )
    );
    assert_eq!(second.0, StatusCode::CREATED, "{}", second.1);
    assert_eq!(third.0, StatusCode::CREATED, "{}", third.1);
    let mut concurrent_numbers = [
        second.2["number"]
            .as_i64()
            .expect("second Work Item number"),
        third.2["number"].as_i64().expect("third Work Item number"),
    ];
    concurrent_numbers.sort();
    assert_eq!(concurrent_numbers, [2, 3]);

    let first_page = get_json(&application, &cookie, &format!("{base}?limit=1")).await;
    assert_eq!(first_page.0, StatusCode::OK);
    assert_eq!(
        first_page.2["items"]
            .as_array()
            .expect("first Work Item page")
            .len(),
        1
    );
    let cursor = first_page.2["nextCursor"]
        .as_str()
        .expect("next Work Item cursor");
    let second_page = get_json(
        &application,
        &cookie,
        &format!("{base}?limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(second_page.0, StatusCode::OK);
    assert_eq!(
        second_page.2["items"]
            .as_array()
            .expect("second Work Item page")
            .len(),
        1
    );
    assert_ne!(
        first_page.2["items"][0]["id"],
        second_page.2["items"][0]["id"]
    );
    let search = get_json(&application, &cookie, &format!("{base}?q=First")).await;
    assert_eq!(
        search.2["items"]
            .as_array()
            .expect("search Work Item page")
            .len(),
        1
    );
    assert_eq!(search.2["items"][0]["id"], created.2["id"]);
    for foreign_filter in ["typeId", "statusId", "assigneeActorId"] {
        let response = get_json(
            &application,
            &cookie,
            &format!("{base}?{foreign_filter}={}", Uuid::now_v7()),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST, "{}", response.1);
    }

    let detail = get_json(
        &application,
        &cookie,
        &format!(
            "{base}/{}",
            created.2["id"].as_str().expect("created Work Item ID")
        ),
    )
    .await;
    assert_eq!(detail.0, StatusCode::OK);
    assert_eq!(detail.2["key"], "AW-1");

    let archived = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-archive-one"),
        &format!(
            "{base}/{}/archive",
            created.2["id"].as_str().expect("created ID")
        ),
        json!({"expectedVersion": 2}),
    )
    .await;
    assert_eq!(archived.0, StatusCode::OK, "{}", archived.1);
    assert_eq!(archived.2["version"], 3);
    assert!(archived.2["archivedAt"].is_string());
    let archived_list = get_json(&application, &cookie, &format!("{base}?archived=true")).await;
    assert_eq!(archived_list.0, StatusCode::OK);
    assert_eq!(
        archived_list.2["items"]
            .as_array()
            .expect("archived Work Item list")
            .len(),
        1
    );
    let restored = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-restore-one"),
        &format!(
            "{base}/{}/restore",
            created.2["id"].as_str().expect("created ID")
        ),
        json!({"expectedVersion": 3}),
    )
    .await;
    assert_eq!(restored.0, StatusCode::OK, "{}", restored.1);
    assert_eq!(restored.2["version"], 4);
    assert!(restored.2["archivedAt"].is_null());
    let no_op_restore = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-restore-no-op"),
        &format!(
            "{base}/{}/restore",
            created.2["id"].as_str().expect("created ID")
        ),
        json!({"expectedVersion": 4}),
    )
    .await;
    assert_eq!(no_op_restore.0, StatusCode::OK);
    assert_eq!(no_op_restore.2["version"], 4);

    let activity_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_activity WHERE project_id = $1",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("count M4 activity");
    assert_eq!(activity_count, 6);
    let outbox_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events WHERE project_id = $1 AND resource_kind = 'work_item' AND conversation_id IS NULL",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("count M4 outbox events");
    assert_eq!(outbox_count, 6);
    let counter = sqlx::query_scalar::<_, i64>(
        "SELECT next_work_item_number FROM project_counters WHERE project_id = $1",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("read M4 Work Item counter");
    assert_eq!(counter, 4);
    let referenced_type_id = Uuid::parse_str(
        created.2["itemType"]["id"]
            .as_str()
            .expect("created Work Item type ID"),
    )
    .expect("created Work Item type UUID");
    let referenced_status_id = Uuid::parse_str(
        created.2["status"]["id"]
            .as_str()
            .expect("created Work Item status ID"),
    )
    .expect("created Work Item status UUID");
    let disable_type =
        sqlx::query("UPDATE work_item_types SET enabled = false WHERE project_id = $1 AND id = $2")
            .bind(seed.project_id)
            .bind(referenced_type_id)
            .execute(&pool)
            .await;
    assert!(disable_type.is_err());
    let disable_status = sqlx::query(
        "UPDATE work_item_statuses SET enabled = false WHERE project_id = $1 AND id = $2",
    )
    .bind(seed.project_id)
    .bind(referenced_status_id)
    .execute(&pool)
    .await;
    assert!(disable_status.is_err());
    let key_change = sqlx::query("UPDATE projects SET project_key = 'NEW' WHERE id = $1")
        .bind(seed.project_id)
        .execute(&pool)
        .await;
    assert!(key_change.is_err());

    supervisors.shutdown_all().await;
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated M4 Work Item schema");
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
                    .expect("serialize M4 login request"),
                ))
                .expect("build M4 login request"),
        )
        .await
        .expect("M4 login response");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("M4 cookie header")
                .split(';')
                .next()
                .expect("M4 cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn csrf_from_cookie(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|pair| pair.strip_prefix("agent_workspace_csrf="))
        .expect("M4 CSRF cookie")
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
                .expect("build M4 GET request"),
        )
        .await
        .expect("M4 GET response");
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
                    serde_json::to_vec(&body).expect("serialize M4 request"),
                ))
                .expect("build M4 POST request"),
        )
        .await
        .expect("M4 POST response");
    response_json(response).await
}

async fn patch_json(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    idempotency_key: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("origin", "http://127.0.0.1:3100")
                .header("x-csrf-token", csrf)
                .header("idempotency-key", idempotency_key)
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize M4 PATCH request"),
                ))
                .expect("build M4 PATCH request"),
        )
        .await
        .expect("M4 PATCH response");
    response_json(response).await
}

async fn response_json(response: axum::response::Response) -> (StatusCode, String, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read M4 response");
    let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 M4 response");
    let value = serde_json::from_str(&text).expect("JSON M4 response");
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
        .expect("connect schema-scoped M4 Work Item pool")
}

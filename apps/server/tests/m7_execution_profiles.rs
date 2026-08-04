use std::{collections::BTreeMap, sync::Arc};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
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

const ORIGIN: &str = "http://127.0.0.1:3100";
const PASSWORD: &str = "correct horse battery staple";

#[cfg(unix)]
#[tokio::test]
async fn team_runs_use_only_invoker_owned_profiles_and_missing_setup_has_no_effects() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m7_execution_profiles_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M7 execution schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M7 execution migrations");

    let temporary = tempfile::tempdir().expect("temporary M7 execution root");
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
            temporary.path().join("web").to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M7 execution configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M7 execution fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "m7-execution-owner".to_owned(),
            display_name: "M7 Execution Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M7 execution owner");
    let member_actor_id = add_human(
        &pool,
        seed.organization_id,
        seed.project_id,
        "m7-execution-member",
        "M7 Execution Member",
        "m7_execution_member",
        "member",
    )
    .await;
    let viewer_actor_id = add_human(
        &pool,
        seed.organization_id,
        seed.project_id,
        "m7-execution-viewer",
        "M7 Execution Viewer",
        "m7_execution_viewer",
        "viewer",
    )
    .await;
    sqlx::query(
        "UPDATE execution_profiles SET status = 'available', account_state = 'authenticated' WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("authenticate owner profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare M7 execution Repository");

    let supervisors = SupervisorRegistry::new(&config).expect("M7 supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors: supervisors.clone(),
    });
    let (owner_cookie, owner_csrf) = login(&application, "m7-execution-owner").await;
    let (member_cookie, member_csrf) = login(&application, "m7-execution-member").await;
    let (viewer_cookie, viewer_csrf) = login(&application, "m7-execution-viewer").await;
    let profiles_uri = format!("/api/v1/projects/{}/execution-profiles", seed.project_id);

    let (_, member_profiles) = get_json(&application, &member_cookie, &profiles_uri).await;
    assert_eq!(member_profiles["profiles"], json!([]));
    let (worker_status, member_workers) = get_json(
        &application,
        &member_cookie,
        &format!("/api/v1/projects/{}/execution-workers", seed.project_id),
    )
    .await;
    assert_eq!(worker_status, StatusCode::OK);
    assert_eq!(
        member_workers["workers"][0]["status"],
        "authentication_required"
    );
    assert!(
        member_workers["workers"][0]
            .get("defaultExecutionProfileId")
            .is_none(),
        "another human's bootstrap Profile ID is not projected"
    );

    let effects_before_setup = effect_counts(&pool).await;
    let missing_profile = post_json(
        &application,
        &member_cookie,
        &member_csrf,
        "m7-missing-profile-run",
        &format!("/api/v1/threads/{}/agent-runs", seed.thread_id),
        json!({"prompt":"do not create artifacts without my profile"}),
    )
    .await;
    assert_eq!(missing_profile.0, StatusCode::CONFLICT);
    assert_eq!(
        missing_profile.1["error"]["code"],
        "execution_profile_required"
    );
    assert_eq!(
        missing_profile.1["error"]["details"]["action"],
        "connect_personal_codex_account"
    );
    assert_eq!(effect_counts(&pool).await, effects_before_setup);

    let viewer_registration = post_json(
        &application,
        &viewer_cookie,
        &viewer_csrf,
        "m7-viewer-profile-denied",
        &profiles_uri,
        json!({}),
    )
    .await;
    assert_eq!(viewer_registration.0, StatusCode::NOT_FOUND);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM execution_profiles WHERE owner_actor_id = $1",
        )
        .bind(viewer_actor_id)
        .fetch_one(&pool)
        .await
        .expect("count Viewer profiles"),
        0
    );

    let created = post_json(
        &application,
        &member_cookie,
        &member_csrf,
        "m7-member-profile-create",
        &profiles_uri,
        json!({}),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED);
    let member_profile_id = uuid_field(&created.1, "id");
    assert_eq!(created.1["owner"]["id"], member_actor_id.to_string());
    assert_eq!(created.1["accountState"], "authentication_required");
    let serialized_created = serde_json::to_string(&created.1).expect("serialize profile");
    assert!(!serialized_created.contains("codexHomePath"));
    assert!(!serialized_created.contains(data_root.to_string_lossy().as_ref()));
    let replay = post_json(
        &application,
        &member_cookie,
        &member_csrf,
        "m7-member-profile-create",
        &profiles_uri,
        json!({}),
    )
    .await;
    assert_eq!(replay.0, StatusCode::CREATED);
    assert_eq!(replay.1["id"], member_profile_id.to_string());
    let profile_path = sqlx::query_scalar::<_, String>(
        "SELECT codex_home_path FROM execution_profiles WHERE id = $1",
    )
    .bind(member_profile_id)
    .fetch_one(&pool)
    .await
    .expect("read private profile path");
    assert!(
        std::path::Path::new(&profile_path).starts_with(
            data_root
                .canonicalize()
                .expect("canonical personal profile data root"),
        )
    );
    assert_eq!(
        std::fs::metadata(&profile_path)
            .expect("personal profile metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let (_, member_profiles) = get_json(&application, &member_cookie, &profiles_uri).await;
    assert_eq!(
        member_profiles["profiles"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        member_profiles["profiles"][0]["id"],
        member_profile_id.to_string()
    );
    let (_, owner_profiles) = get_json(&application, &owner_cookie, &profiles_uri).await;
    assert_eq!(owner_profiles["profiles"].as_array().map(Vec::len), Some(2));
    let foreign_account = get_json(
        &application,
        &owner_cookie,
        &format!("{profiles_uri}/{member_profile_id}/account"),
    )
    .await;
    assert_eq!(foreign_account.0, StatusCode::NOT_FOUND);

    let effects_before_unready = effect_counts(&pool).await;
    let unready_profile = post_json(
        &application,
        &member_cookie,
        &member_csrf,
        "m7-unready-profile-run",
        &format!("/api/v1/threads/{}/agent-runs", seed.thread_id),
        json!({"prompt":"do not run before authentication"}),
    )
    .await;
    assert_eq!(
        unready_profile.1["error"]["code"],
        "execution_profile_required"
    );
    assert_eq!(effect_counts(&pool).await, effects_before_unready);

    sqlx::query(
        "UPDATE execution_profiles SET status = 'available', account_state = 'authenticated' WHERE id = $1",
    )
    .bind(member_profile_id)
    .execute(&pool)
    .await
    .expect("authenticate member profile");
    let member_run = post_json(
        &application,
        &member_cookie,
        &member_csrf,
        "m7-member-owned-run",
        &format!("/api/v1/threads/{}/agent-runs", seed.thread_id),
        json!({"prompt":"use only my personal profile"}),
    )
    .await;
    assert_eq!(member_run.0, StatusCode::ACCEPTED);
    let member_run_id = uuid_field(&member_run.1, "runId");
    assert_eq!(
        sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
            "SELECT requesting_actor_id, execution_profile_id, execution_worker_id FROM agent_runs WHERE id = $1",
        )
        .bind(member_run_id)
        .fetch_one(&pool)
        .await
        .expect("read member-owned run"),
        (member_actor_id, member_profile_id, seed.execution_worker_id)
    );

    let owner_thread_id = insert_thread(&pool, &seed, "Owner parallel profile proof").await;
    let owner_run = post_json(
        &application,
        &owner_cookie,
        &owner_csrf,
        "m7-owner-owned-run",
        &format!("/api/v1/threads/{owner_thread_id}/agent-runs"),
        json!({"prompt":"use the owner's distinct personal profile"}),
    )
    .await;
    assert_eq!(owner_run.0, StatusCode::ACCEPTED);
    let owner_run_id = uuid_field(&owner_run.1, "runId");
    assert_eq!(
        sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT requesting_actor_id, execution_profile_id FROM agent_runs WHERE id = $1",
        )
        .bind(owner_run_id)
        .fetch_one(&pool)
        .await
        .expect("read owner-owned run"),
        (seed.human_actor_id, seed.execution_profile_id)
    );

    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(member_actor_id)
    .execute(&pool)
    .await
    .expect("disable member Organization membership before admission");
    let organization_disabled_thread_id =
        insert_thread(&pool, &seed, "Organization-disabled member proof").await;
    let effects_before_organization_disabled = effect_counts(&pool).await;
    let organization_disabled_run = post_json(
        &application,
        &member_cookie,
        &member_csrf,
        "m7-organization-disabled-member-run",
        &format!("/api/v1/threads/{organization_disabled_thread_id}/agent-runs"),
        json!({"prompt":"disabled Organization membership must fail closed"}),
    )
    .await;
    assert_eq!(organization_disabled_run.0, StatusCode::NOT_FOUND);
    assert_eq!(
        effect_counts(&pool).await,
        effects_before_organization_disabled
    );
    sqlx::query(
        "UPDATE organization_memberships SET enabled = true WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(member_actor_id)
    .execute(&pool)
    .await
    .expect("restore member Organization membership after admission proof");

    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(member_actor_id)
    .execute(&pool)
    .await
    .expect("disable member before admission");
    let disabled_thread_id = insert_thread(&pool, &seed, "Disabled member proof").await;
    let run_count_before_disabled = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .expect("run count before disabled admission");
    let disabled_run = post_json(
        &application,
        &member_cookie,
        &member_csrf,
        "m7-disabled-member-run",
        &format!("/api/v1/threads/{disabled_thread_id}/agent-runs"),
        json!({"prompt":"disabled membership must fail closed"}),
    )
    .await;
    assert_eq!(disabled_run.0, StatusCode::NOT_FOUND);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .expect("run count after disabled admission"),
        run_count_before_disabled
    );

    supervisors.shutdown_all().await;
    drop(application);
    pool.close().await;
    drop_isolated_schema(&administrative_pool, &schema).await;
}

async fn drop_isolated_schema(administrative_pool: &PgPool, schema: &str) {
    const MAX_ATTEMPTS: u32 = 5;

    for attempt in 1..=MAX_ATTEMPTS {
        match administrative_pool
            .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
            .await
        {
            Ok(_) => return,
            Err(sqlx::Error::Database(error))
                if error.code().as_deref() == Some("40P01") && attempt < MAX_ATTEMPTS =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100 * u64::from(attempt)))
                    .await;
            }
            Err(error) => panic!("drop isolated M7 execution schema: {error}"),
        }
    }

    unreachable!("the final schema-drop attempt either returns or panics");
}

async fn add_human(
    pool: &PgPool,
    organization_id: Uuid,
    project_id: Uuid,
    login_name: &str,
    display_name: &str,
    handle: &str,
    project_role: &str,
) -> Uuid {
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor_id)
        .execute(pool)
        .await
        .expect("insert team human Actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(pool)
    .await
    .expect("insert team Organization membership");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .bind(project_role)
    .bind(display_name)
    .bind(handle)
    .execute(pool)
    .await
    .expect("insert team Project membership");
    bootstrap_development_member(
        pool,
        BootstrapCommand {
            login_name: login_name.to_owned(),
            display_name: display_name.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor_id),
            organization_id: Some(organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap team member User");
    actor_id
}

async fn insert_thread(
    pool: &PgPool,
    seed: &agent_workspace_server::seed::DevSeed,
    name: &str,
) -> Uuid {
    let thread_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name) VALUES ($1, $2, 'thread', $3, $4, $5)",
    )
    .bind(thread_id)
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .bind(seed.repository_id)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert team execution Thread");
    thread_id
}

async fn effect_counts(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM agent_runs),
            (SELECT count(*) FROM repository_workspaces),
            (SELECT count(*) FROM git_worktrees),
            (SELECT count(*) FROM provider_sessions),
            (SELECT count(*) FROM agent_approvals),
            (SELECT count(*) FROM messages),
            (SELECT count(*) FROM outbox_events)
        "#,
    )
    .fetch_one(pool)
    .await
    .expect("count execution side effects")
}

async fn login(application: &axum::Router, login_name: &str) -> (String, String) {
    let response = application
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/auth/login",
            &[("origin", ORIGIN)],
            json!({"loginName":login_name,"password":PASSWORD}),
        ))
        .await
        .expect("team login response");
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

async fn get_json(application: &axum::Router, cookie: &str, uri: &str) -> (StatusCode, Value) {
    let response = application
        .clone()
        .oneshot(request("GET", uri, &[("cookie", cookie)], Value::Null))
        .await
        .expect("team GET response");
    let status = response.status();
    (status, response_json(response).await)
}

async fn post_json(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    idempotency_key: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = application
        .clone()
        .oneshot(request(
            "POST",
            uri,
            &[
                ("origin", ORIGIN),
                ("cookie", cookie),
                ("x-csrf-token", csrf),
                ("idempotency-key", idempotency_key),
            ],
            body,
        ))
        .await
        .expect("team POST response");
    let status = response.status();
    (status, response_json(response).await)
}

fn request(method: &str, uri: &str, headers: &[(&str, &str)], body: Value) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("serialize request"))
    };
    builder.body(body).expect("build team request")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("read team response"),
    )
    .expect("parse team response")
}

fn uuid_field(value: &Value, field: &str) -> Uuid {
    Uuid::parse_str(
        value[field]
            .as_str()
            .unwrap_or_else(|| panic!("missing UUID field {field}: {value}")),
    )
    .unwrap_or_else(|_| panic!("invalid UUID field {field}: {value}"))
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
        .expect("connect M7 execution PostgreSQL");
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
        .expect("connect isolated M7 execution schema")
}

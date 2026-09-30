use std::{collections::BTreeMap, sync::Arc};

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

const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn human_memberships_are_reversible_idempotent_and_revoke_credentials() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m7_memberships_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M7 membership schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M7 membership migrations");

    let temporary = tempfile::tempdir().expect("temporary M7 root");
    let config = test_config(&database_url, temporary.path());
    let seed = dev_seed(&pool, &config).await.expect("seed M7 fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "m7-owner".to_owned(),
            display_name: "M7 Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M7 owner");

    let member_actor_id = add_human(
        &pool,
        seed.organization_id,
        seed.project_id,
        HumanFixture {
            login: "m7-member",
            display_name: "M7 Member",
            handle: "m7_member",
            organization_role: "member",
            project_role: "member",
        },
    )
    .await;
    let viewer_actor_id = add_human(
        &pool,
        seed.organization_id,
        seed.project_id,
        HumanFixture {
            login: "m7-viewer",
            display_name: "M7 Viewer",
            handle: "m7_viewer",
            organization_role: "member",
            project_role: "viewer",
        },
    )
    .await;
    let admin_actor_id = add_human(
        &pool,
        seed.organization_id,
        seed.project_id,
        HumanFixture {
            login: "m7-admin",
            display_name: "M7 Admin",
            handle: "m7_admin",
            organization_role: "admin",
            project_role: "admin",
        },
    )
    .await;
    let credential_id = add_credentials(&pool, seed.project_id, member_actor_id).await;

    let application = test_app(pool.clone(), config);
    let (owner_cookie, owner_csrf) = login(&application, "m7-owner").await;
    let members_uri = format!("/api/v1/projects/{}/members", seed.project_id);
    let listed = application
        .clone()
        .oneshot(request(
            "GET",
            &members_uri,
            &[("cookie", owner_cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("list members response");
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    let members = listed["members"].as_array().expect("member array");
    assert!(
        members
            .iter()
            .any(|member| { member["actorId"].as_str() == Some(&member_actor_id.to_string()) })
    );
    assert!(!members.iter().any(|member| member["role"] == "worker"));
    let serialized = serde_json::to_string(&listed).expect("serialize member list");
    for forbidden in ["password", "sessionId", "tokenDigest", "codexHomePath"] {
        assert!(!serialized.contains(forbidden));
    }

    let update_uri = format!("{members_uri}/{member_actor_id}");
    let disable_headers = write_headers(&owner_cookie, &owner_csrf, "disable-member-exactly-once");
    let disabled = application
        .clone()
        .oneshot(request(
            "PATCH",
            &update_uri,
            &disable_headers,
            json!({"expectedVersion":1,"enabled":false}),
        ))
        .await
        .expect("disable member response");
    assert_eq!(disabled.status(), StatusCode::OK);
    let disabled = response_json(disabled).await;
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["version"], 2);

    let replay = application
        .clone()
        .oneshot(request(
            "PATCH",
            &update_uri,
            &disable_headers,
            json!({"expectedVersion":1,"enabled":false}),
        ))
        .await
        .expect("disable replay response");
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json(replay).await["version"], 2);

    assert_eq!(
        sqlx::query_as::<_, (String, i64, bool)>(
            "SELECT status, version, revoked_at IS NOT NULL FROM mcp_client_credentials WHERE id = $1",
        )
        .bind(credential_id)
        .fetch_one(&pool)
        .await
        .expect("bearer revocation"),
        ("revoked".to_owned(), 2, true)
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT bool_and(revoked_at IS NOT NULL) FROM mcp_oauth_grants WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(seed.project_id)
        .bind(member_actor_id)
        .fetch_one(&pool)
        .await
        .expect("OAuth revocation")
    );
    for (table, predicate) in [
        (
            "audit_events",
            "action = 'project_membership.updated' AND target_id = $1",
        ),
        (
            "outbox_events",
            "event_type = 'project.membership.updated' AND resource_id = $1",
        ),
    ] {
        let query = format!("SELECT count(*) FROM {table} WHERE {predicate}");
        let id = if table == "audit_events" {
            member_actor_id
        } else {
            seed.project_id
        };
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&query)
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("singular side effect"),
            1
        );
    }

    let stale = application
        .clone()
        .oneshot(request(
            "PATCH",
            &update_uri,
            &write_headers(&owner_cookie, &owner_csrf, "stale-member-update"),
            json!({"expectedVersion":1,"enabled":true}),
        ))
        .await
        .expect("stale member response");
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let enabled = application
        .clone()
        .oneshot(request(
            "PATCH",
            &update_uri,
            &write_headers(&owner_cookie, &owner_csrf, "enable-member-once"),
            json!({"expectedVersion":2,"enabled":true,"role":"viewer"}),
        ))
        .await
        .expect("enable member response");
    assert_eq!(enabled.status(), StatusCode::OK);
    let enabled = response_json(enabled).await;
    assert_eq!(enabled["enabled"], true);
    assert_eq!(enabled["role"], "viewer");
    assert_eq!(enabled["version"], 3);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM mcp_client_credentials WHERE id = $1",)
            .bind(credential_id)
            .fetch_one(&pool)
            .await
            .expect("credential stays revoked"),
        "revoked"
    );

    let self_denied = application
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("{members_uri}/{}", seed.human_actor_id),
            &write_headers(&owner_cookie, &owner_csrf, "self-disable-denied"),
            json!({"expectedVersion":1,"enabled":false}),
        ))
        .await
        .expect("self-disable response");
    assert_eq!(self_denied.status(), StatusCode::FORBIDDEN);

    let (admin_cookie, admin_csrf) = login(&application, "m7-admin").await;
    let admin_promotion = application
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("{members_uri}/{viewer_actor_id}"),
            &write_headers(&admin_cookie, &admin_csrf, "admin-promotion-denied"),
            json!({"expectedVersion":1,"role":"admin"}),
        ))
        .await
        .expect("admin promotion response");
    assert_eq!(admin_promotion.status(), StatusCode::FORBIDDEN);

    // Promoting a human to Project admin makes them an Organization admin so
    // they can actually manage members; demoting them reverses it.
    let organization_role = |actor_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT role FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
            )
            .bind(seed.organization_id)
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .expect("organization role")
        }
    };
    let promoted = application
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("{members_uri}/{viewer_actor_id}"),
            &write_headers(&owner_cookie, &owner_csrf, "owner-promotes-viewer"),
            json!({"expectedVersion":1,"role":"admin"}),
        ))
        .await
        .expect("owner promotion response");
    assert_eq!(promoted.status(), StatusCode::OK);
    assert_eq!(organization_role(viewer_actor_id).await, "admin");
    let (promoted_cookie, _) = login(&application, "m7-viewer").await;
    let candidates = application
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/projects/{}/member-candidates", seed.project_id),
            &[("cookie", promoted_cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("promoted admin manages members");
    assert_eq!(candidates.status(), StatusCode::OK);
    let demoted = application
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("{members_uri}/{viewer_actor_id}"),
            &write_headers(&owner_cookie, &owner_csrf, "owner-demotes-admin"),
            json!({"expectedVersion":2,"role":"viewer"}),
        ))
        .await
        .expect("owner demotion response");
    assert_eq!(demoted.status(), StatusCode::OK);
    assert_eq!(organization_role(viewer_actor_id).await, "member");
    assert_eq!(organization_role(seed.human_actor_id).await, "owner");

    // An Organization owner who is only a Project admin still cannot remove
    // that Project's sole enabled owner. Both owner sets are locked.
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("prepare manager role");
    sqlx::query(
        "UPDATE organization_memberships SET role = 'owner' WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(admin_actor_id)
    .execute(&pool)
    .await
    .expect("prepare second Organization owner");
    sqlx::query(
        "UPDATE project_memberships SET role = 'owner' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(admin_actor_id)
    .execute(&pool)
    .await
    .expect("prepare sole Project owner");
    let last_owner = application
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("{members_uri}/{admin_actor_id}"),
            &write_headers(&owner_cookie, &owner_csrf, "last-owner-denied"),
            json!({"expectedVersion":1,"role":"member"}),
        ))
        .await
        .expect("last owner response");
    assert_eq!(last_owner.status(), StatusCode::CONFLICT);

    drop(application);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&administrative_pool)
        .await
        .expect("drop M7 membership schema");
}

struct HumanFixture<'a> {
    login: &'a str,
    display_name: &'a str,
    handle: &'a str,
    organization_role: &'a str,
    project_role: &'a str,
}

async fn add_human(
    pool: &PgPool,
    organization_id: Uuid,
    project_id: Uuid,
    fixture: HumanFixture<'_>,
) -> Uuid {
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor_id)
        .execute(pool)
        .await
        .expect("insert human Actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, $3)",
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(fixture.organization_role)
    .execute(pool)
    .await
    .expect("insert Organization membership");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .bind(fixture.project_role)
    .bind(fixture.display_name)
    .bind(fixture.handle)
    .execute(pool)
    .await
    .expect("insert Project membership");
    bootstrap_development_member(
        pool,
        BootstrapCommand {
            login_name: fixture.login.to_owned(),
            display_name: fixture.display_name.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor_id),
            organization_id: Some(organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap member User");
    actor_id
}

async fn add_credentials(pool: &PgPool, project_id: Uuid, actor_id: Uuid) -> Uuid {
    let credential_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO mcp_client_credentials (
          id, project_id, actor_id, client_name, token_prefix, token_digest,
          scopes, expires_at
        ) VALUES ($1, $2, $3, 'M7 member client', 'aw_mcp_m7member', $4,
                  ARRAY['read','write']::text[], now() + interval '1 day')
        "#,
    )
    .bind(credential_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(vec![7_u8; 32])
    .execute(pool)
    .await
    .expect("insert bearer credential");
    let client_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mcp_oauth_clients (id, client_name, redirect_uris) VALUES ($1, 'M7 client', ARRAY['http://127.0.0.1:1455/callback']::text[])",
    )
    .bind(client_id)
    .execute(pool)
    .await
    .expect("insert OAuth client");
    sqlx::query(
        "INSERT INTO mcp_oauth_grants (id, client_id, project_id, actor_id, scopes) VALUES ($1, $2, $3, $4, ARRAY['read','write']::text[])",
    )
    .bind(Uuid::now_v7())
    .bind(client_id)
    .bind(project_id)
    .bind(actor_id)
    .execute(pool)
    .await
    .expect("insert OAuth grant");
    credential_id
}

fn test_config(database_url: &str, root: &std::path::Path) -> AppConfig {
    AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.to_owned(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            root.join("data").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            root.join("data/codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            "/usr/bin/false".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            root.join("web").to_string_lossy().into_owned(),
        ),
    ]))
    .expect("test configuration")
}

fn test_app(pool: PgPool, config: AppConfig) -> axum::Router {
    let supervisors = SupervisorRegistry::new(&config).expect("supervisor registry");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    })
}

async fn login(application: &axum::Router, login_name: &str) -> (String, String) {
    let response = application
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/auth/login",
            &[("origin", "http://127.0.0.1:3100")],
            json!({"loginName":login_name,"password":PASSWORD}),
        ))
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

fn write_headers<'a>(
    cookie: &'a str,
    csrf: &'a str,
    idempotency_key: &'a str,
) -> [(&'a str, &'a str); 4] {
    [
        ("origin", "http://127.0.0.1:3100"),
        ("cookie", cookie),
        ("x-csrf-token", csrf),
        ("idempotency-key", idempotency_key),
    ]
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
        Body::from(serde_json::to_vec(&body).expect("serialize body"))
    };
    builder.body(body).expect("build request")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("read body"),
    )
    .expect("parse JSON")
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
        .expect("connect PostgreSQL");
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
        .expect("connect schema-scoped pool")
}

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
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

/// DMs and groups are visible only to their members, and the hidden direct
/// space never shows up as an ordinary Project.
#[tokio::test]
async fn direct_conversations_are_private_to_their_members() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("direct_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Direct Organization".to_owned()),
            organization_slug: Some(format!("direct-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let app = test_app(pool.clone(), &database_url);
    let (cookie, csrf) = login(&app).await;
    let headers = |key: Option<&'static str>| {
        let mut headers = vec![
            ("origin".to_owned(), "http://127.0.0.1:3100".to_owned()),
            ("cookie".to_owned(), cookie.clone()),
            ("x-csrf-token".to_owned(), csrf.clone()),
        ];
        if let Some(key) = key {
            headers.push(("idempotency-key".to_owned(), key.to_owned()));
        }
        headers
    };

    // The first visit creates the space.
    let (status, overview) = call(&app, "GET", "/api/v1/direct", &headers(None), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let space_id = uuid(&overview, "projectId");
    assert_eq!(overview["conversations"], json!([]));
    let mara = add_agent(&pool, space_id, "Mara", "mara").await;
    let rook = add_agent(&pool, space_id, "Rook", "rook").await;

    // Opening the same DM twice returns the same conversation.
    let (status, dm) = call(
        &app,
        "POST",
        "/api/v1/direct/dms",
        &headers(None),
        json!({ "actorId": mara }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let dm_id = uuid(&dm["conversation"], "id");
    let (status, again) = call(
        &app,
        "POST",
        "/api/v1/direct/dms",
        &headers(None),
        json!({ "actorId": mara }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(uuid(&again["conversation"], "id"), dm_id);

    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/conversations/{dm_id}/messages"),
        &headers(Some("dm-hello")),
        json!({ "content": text_content("Hello Mara") }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, group) = call(
        &app,
        "POST",
        "/api/v1/direct/groups",
        &headers(Some("group-1")),
        json!({ "memberIds": [mara, rook] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = uuid(&group["conversation"], "id");
    assert_eq!(group["conversation"]["name"], "Mara, Rook");

    assert!(visible(&pool, dm_id, mara).await);
    assert!(!visible(&pool, dm_id, rook).await, "Rook is not in the DM");
    assert!(visible(&pool, group_id, rook).await);

    // After leaving the group, the owner can no longer read or post in it.
    let owner = owner_actor(&pool).await;
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/conversations/{group_id}/members/{owner}/remove"),
        &headers(None),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/conversations/{group_id}/timeline"),
        &headers(None),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/conversations/{group_id}/messages"),
        &headers(Some("group-after-leaving")),
        json!({ "content": text_content("Still here?") }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, overview) = call(&app, "GET", "/api/v1/direct", &headers(None), Value::Null).await;
    let listed = overview["conversations"]
        .as_array()
        .expect("conversations")
        .iter()
        .map(|conversation| uuid(conversation, "id"))
        .collect::<Vec<_>>();
    assert_eq!(listed, vec![dm_id]);

    let (_, projects) = call(&app, "GET", "/api/v1/projects", &headers(None), Value::Null).await;
    assert!(
        projects["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .all(|project| uuid(project, "id") != space_id),
        "the direct space is not an ordinary Project"
    );

    agent_workspace_server::test_support::drop_schema(&admin, &schema)
        .await
        .expect("drop schema");
}

/// An agent connected in one of your projects can be messaged without being
/// connected again under Messages. Agents you share no project with, and
/// switched-off agents, stay out of reach.
#[tokio::test]
async fn agents_in_your_projects_can_be_messaged() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("direct_reach_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Reach Organization".to_owned()),
            organization_slug: Some(format!("reach-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let app = test_app(pool.clone(), &database_url);
    let (cookie, csrf) = login(&app).await;
    let headers = |key: Option<&str>| {
        let mut headers = vec![
            ("origin".to_owned(), "http://127.0.0.1:3100".to_owned()),
            ("cookie".to_owned(), cookie.clone()),
            ("x-csrf-token".to_owned(), csrf.clone()),
        ];
        if let Some(key) = key {
            headers.push(("idempotency-key".to_owned(), key.to_owned()));
        }
        headers
    };
    let (_, overview) = call(&app, "GET", "/api/v1/direct", &headers(None), Value::Null).await;
    let space_id = uuid(&overview, "projectId");
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(space_id)
            .fetch_one(&pool)
            .await
            .expect("organization");
    let project = |key: &'static str| {
        let headers = headers(Some(key));
        let app = app.clone();
        async move {
            let (status, project) = call(
                &app,
                "POST",
                "/api/v1/projects",
                &headers,
                json!({ "organizationId": organization_id, "key": key, "name": key }),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{project}");
            uuid(&project, "id")
        }
    };
    let connect = |project_id: Uuid, name: &'static str, role: &'static str| {
        let headers = headers(None);
        let app = app.clone();
        async move {
            let scopes = if role == "viewer" {
                json!(["read", "events"])
            } else {
                json!(["read", "write", "events"])
            };
            let (status, created) = call(
                &app,
                "POST",
                &format!("/api/v1/projects/{project_id}/external-agents"),
                &headers,
                json!({
                    "connectionName": name,
                    "displayName": name,
                    "handle": name.to_lowercase(),
                    "role": role,
                    "scopes": scopes,
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{created}");
            uuid(&created["connection"], "actorId")
        }
    };
    let build = project("BUILD").await;
    let elsewhere = project("ELSE").await;
    let mara = connect(build, "Mara", "member").await;
    let rook = connect(build, "Rook", "member").await;
    let watcher = connect(build, "Watcher", "viewer").await;
    let off = connect(build, "Off", "member").await;
    let ghost = connect(elsewhere, "Ghost", "member").await;
    // Stale also works in Build, but its home project is archived, so its
    // connection can't reach anything any more.
    let attic = project("ATTIC").await;
    let stale = connect(attic, "Stale", "member").await;
    let owner = owner_actor(&pool).await;
    sqlx::query(
        r#"
        INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
        VALUES ($1, $2, $3, 'member', 'Stale', 'stale')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(build)
    .bind(stale)
    .execute(&pool)
    .await
    .expect("add Stale to Build");
    sqlx::query("UPDATE projects SET archived_at = now(), archived_by_actor_id = $2 WHERE id = $1")
        .bind(attic)
        .bind(owner)
        .execute(&pool)
        .await
        .expect("archive Attic");
    // The owner leaves Elsewhere, so Ghost shares no project with them.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(elsewhere)
    .bind(owner)
    .execute(&pool)
    .await
    .expect("leave Elsewhere");
    sqlx::query("UPDATE actors SET status = 'disabled' WHERE id = $1")
        .bind(off)
        .execute(&pool)
        .await
        .expect("switch Off off");

    let (status, overview) = call(&app, "GET", "/api/v1/direct", &headers(None), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let listed = overview["people"]
        .as_array()
        .expect("people")
        .iter()
        .map(|person| uuid(person, "id"))
        .collect::<Vec<_>>();
    assert!(listed.contains(&mara) && listed.contains(&rook) && listed.contains(&watcher));
    assert!(
        !listed.contains(&ghost),
        "Ghost shares no project with the owner"
    );
    assert!(
        !listed.contains(&off),
        "a switched-off agent can't be messaged"
    );
    assert!(
        !listed.contains(&stale),
        "an agent whose home project is archived can't answer"
    );

    let (status, dm) = call(
        &app,
        "POST",
        "/api/v1/direct/dms",
        &headers(None),
        json!({ "actorId": mara }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm_id = uuid(&dm["conversation"], "id");
    assert!(visible(&pool, dm_id, mara).await);
    assert!(!visible(&pool, dm_id, rook).await);

    for unreachable in [ghost, off, stale] {
        let (status, _) = call(
            &app,
            "POST",
            "/api/v1/direct/dms",
            &headers(None),
            json!({ "actorId": unreachable }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/direct/groups",
        &headers(Some("group-with-ghost")),
        json!({ "memberIds": [rook, ghost] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, group) = call(
        &app,
        "POST",
        "/api/v1/direct/groups",
        &headers(Some("group-with-rook")),
        json!({ "memberIds": [rook, watcher] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    assert!(visible(&pool, uuid(&group["conversation"], "id"), rook).await);

    // Three agents with the same handle, from three projects, can all join.
    let mut twins = Vec::new();
    for (key, name) in [("TWINA", "TwinA"), ("TWINB", "TwinB"), ("TWINC", "TwinC")] {
        let home = project(key).await;
        let twin = connect(home, name, "member").await;
        sqlx::query("UPDATE project_memberships SET handle = 'twin' WHERE project_id = $1 AND actor_id = $2")
            .bind(home)
            .bind(twin)
            .execute(&pool)
            .await
            .expect("same handle");
        twins.push(twin);
    }
    let (status, trio) = call(
        &app,
        "POST",
        "/api/v1/direct/groups",
        &headers(Some("group-of-twins")),
        json!({ "memberIds": twins }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{trio}");
    let handles = sqlx::query_scalar::<_, String>(
        "SELECT handle FROM project_memberships WHERE project_id = $1 AND actor_id = ANY($2)",
    )
    .bind(space_id)
    .bind(&twins)
    .fetch_all(&pool)
    .await
    .expect("twin handles");
    assert_eq!(handles.len(), 3);

    // Each keeps its access: the read-only agent stays read-only here.
    let roles = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT actor_id, role FROM project_memberships WHERE project_id = $1 AND actor_id = ANY($2)",
    )
    .bind(space_id)
    .bind(vec![mara, rook, watcher, ghost, off])
    .fetch_all(&pool)
    .await
    .expect("space memberships")
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    assert_eq!(
        roles,
        BTreeMap::from([
            (mara, "member".to_owned()),
            (rook, "member".to_owned()),
            (watcher, "viewer".to_owned()),
        ])
    );

    agent_workspace_server::test_support::drop_schema(&admin, &schema)
        .await
        .expect("drop schema");
}

async fn add_agent(pool: &PgPool, space_id: Uuid, name: &str, handle: &str) -> Uuid {
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'external_agent')")
        .bind(actor_id)
        .execute(pool)
        .await
        .expect("insert agent");
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (organization_id, actor_id, role)
        SELECT organization_id, $2, 'member' FROM projects WHERE id = $1
        "#,
    )
    .bind(space_id)
    .bind(actor_id)
    .execute(pool)
    .await
    .expect("insert agent organization membership");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
        VALUES ($1, $2, $3, 'member', $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(space_id)
    .bind(actor_id)
    .bind(name)
    .bind(handle)
    .execute(pool)
    .await
    .expect("insert agent membership");
    actor_id
}

async fn owner_actor(pool: &PgPool) -> Uuid {
    sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
        .fetch_one(pool)
        .await
        .expect("owner actor")
}

async fn visible(pool: &PgPool, conversation_id: Uuid, actor_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
        .bind(conversation_id)
        .bind(actor_id)
        .fetch_one(pool)
        .await
        .expect("visibility check")
}

fn text_content(text: &str) -> Value {
    json!({ "version": 1, "nodes": [{ "type": "text", "text": text }] })
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Value,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("serialize body"))
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).expect("build request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read body");
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

fn uuid(value: &Value, field: &str) -> Uuid {
    Uuid::parse_str(value[field].as_str().expect("UUID field")).expect("valid UUID")
}

async fn login(app: &axum::Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "loginName": "owner", "password": PASSWORD }))
                        .expect("login body"),
                ))
                .expect("login request"),
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
    (cookies.join("; "), csrf)
}

fn test_app(pool: PgPool, database_url: &str) -> axum::Router {
    let values = BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url.to_owned(),
    )]);
    let config = AppConfig::from_values(&values).expect("test configuration");
    let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
        .expect("test supervisor registry");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    })
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _metadata| {
            let schema = Arc::clone(&schema);
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema.as_str())
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect schema pool")
}

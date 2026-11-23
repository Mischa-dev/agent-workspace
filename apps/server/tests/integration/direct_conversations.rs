use std::{collections::BTreeMap, sync::Arc};

use crate::support::legacy::router;
use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
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

/// K4.12: who you may DM is whoever shares a workspace with you there, by
/// your role's `direct.start`; what anyone may do in a DM follows their
/// workspace roles, for agents exactly as for people. Switched-off accounts
/// stay out of reach.
#[tokio::test]
async fn agent_dm_permissions_follow_role() {
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
    let organization_id = uuid(&overview, "organizationId");
    let project = |key: &'static str| {
        let headers = headers(Some(key));
        let app = app.clone();
        async move {
            let (status, project) = call(
                &app,
                "POST",
                &format!("/api/v2/w/{organization_id}/ops/space_create"),
                &headers,
                json!({ "key": key, "name": key }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{project}");
            uuid(&project["value"], "id")
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
    let off = connect(build, "Off", "member").await;
    // Ghost works only in a project the owner left: sharing the workspace
    // is enough to be messaged.
    let ghost = connect(elsewhere, "Ghost", "member").await;
    let owner = owner_actor(&pool).await;
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
    assert!(listed.contains(&mara) && listed.contains(&rook) && listed.contains(&ghost));
    assert!(
        !listed.contains(&off),
        "a switched-off agent can't be messaged"
    );
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/direct/dms",
        &headers(None),
        json!({ "actorId": off }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, group) = call(
        &app,
        "POST",
        "/api/v1/direct/groups",
        &headers(Some("group-with-ghost")),
        json!({ "memberIds": [rook, ghost] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group_id = uuid(&group["conversation"], "id");
    assert!(visible(&pool, group_id, ghost).await);
    let space =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT project_id FROM conversations WHERE id = $1")
            .bind(group_id)
            .fetch_one(&pool)
            .await
            .expect("group");
    assert_eq!(space, None, "a group lives in no space");

    // Talker and Writer can both be messaged. Writer's workspace role is
    // Viewer, so it can't post in its DM even with a token that writes;
    // made a Member in the workspace, it can. A role in a project changes
    // nothing in a DM.
    let agent = |name: &'static str| {
        let headers = headers(None);
        let app = app.clone();
        async move {
            let (status, created) = call(
                &app,
                "POST",
                &format!("/api/v1/projects/{build}/external-agents"),
                &headers,
                json!({
                    "connectionName": name,
                    "displayName": name,
                    "handle": name.to_lowercase(),
                    "role": "member",
                    "scopes": ["read", "write", "events"],
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{created}");
            let token = created["oneTimeCredential"]["token"]
                .as_str()
                .expect("agent token")
                .to_owned();
            (uuid(&created["connection"], "actorId"), token)
        }
    };
    let (talker, talker_token) = agent("Talker").await;
    let (writer, writer_token) = agent("Writer").await;
    let workspace_role = |actor: Uuid, role: &'static str| {
        let pool = pool.clone();
        async move {
            // A workspace has no built-in viewer: a custom role reads only.
            sqlx::query(
                "INSERT INTO workspace_roles (id, organization_id, key, base_role, name) \
                 VALUES (gen_random_uuid(), $1, 'reader', 'viewer', 'Reader') \
                 ON CONFLICT (organization_id, key) DO NOTHING",
            )
            .bind(organization_id)
            .execute(&pool)
            .await
            .expect("reader role");
            sqlx::query(
                "UPDATE organization_memberships SET role = 'member', \
                     role_id = (SELECT id FROM workspace_roles \
                                WHERE organization_id = $1 AND key = 'reader' AND $3 = 'viewer') \
                 WHERE organization_id = $1 AND actor_id = $2",
            )
            .bind(organization_id)
            .bind(actor)
            .bind(role)
            .execute(&pool)
            .await
            .expect("workspace role");
        }
    };
    workspace_role(writer, "viewer").await;
    let mut dms = Vec::new();
    for (actor, key) in [(talker, "dm-talker"), (writer, "dm-writer")] {
        let (status, dm) = call(
            &app,
            "POST",
            "/api/v1/direct/dms",
            &headers(Some(key)),
            json!({ "actorId": actor }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{dm}");
        dms.push(uuid(&dm["conversation"], "id"));
    }
    let send = |token: String, conversation: Uuid, key: &'static str| {
        let app = app.clone();
        async move {
            let session = super::agent_interface::initialize(&app, &token)
                .await
                .expect("session");
            super::agent_interface::tool(
                &app,
                &token,
                &session,
                "message_send",
                json!({"conversationId": conversation, "text": "Hello", "clientRequestId": key}),
            )
            .await
        }
    };
    let said = send(talker_token.clone(), dms[0], "talker-hello").await;
    assert_eq!(said["outcome"], "ok", "{said}");
    let refused = send(writer_token.clone(), dms[1], "writer-hello").await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    // A project role doesn't lift it.
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(build)
    .bind(writer)
    .execute(&pool)
    .await
    .expect("Writer an admin in Build");
    let still = send(writer_token.clone(), dms[1], "writer-again").await;
    assert_ne!(still["outcome"], "ok", "{still}");
    workspace_role(writer, "member").await;
    let allowed = send(writer_token, dms[1], "writer-member").await;
    assert_eq!(allowed["outcome"], "ok", "{allowed}");

    agent_workspace_server::test_support::drop_schema(&admin, &schema)
        .await
        .expect("drop schema");
}

async fn add_agent(pool: &PgPool, space_id: Uuid, name: &str, handle: &str) -> Uuid {
    let actor_id = Uuid::now_v7();
    // DMs live in no space: an agent is reached through its workspace.
    let organization_id = if space_id == agent_workspace_server::spaces::DIRECT {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT organization_id FROM organization_memberships ORDER BY created_at LIMIT 1",
        )
        .fetch_one(pool)
        .await
        .expect("the workspace")
    } else {
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(space_id)
            .fetch_one(pool)
            .await
            .expect("the space's workspace")
    };
    sqlx::query(
        "INSERT INTO actors (id, kind, owner_workspace_id) VALUES ($1, 'external_agent', $2)",
    )
    .bind(actor_id)
    .bind(organization_id)
    .execute(pool)
    .await
    .expect("insert agent");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(pool)
    .await
    .expect("insert agent organization membership");
    sqlx::query(
        "INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (organization_id, actor_id) DO UPDATE \
         SET display_name = EXCLUDED.display_name, handle = EXCLUDED.handle",
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(name)
    .bind(handle)
    .execute(pool)
    .await
    .expect("agent profile");
    if space_id != agent_workspace_server::spaces::DIRECT {
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, 'member', $4, $5)
            ON CONFLICT (project_id, actor_id) DO NOTHING
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
    }
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

/// P0.0: everyone who has opened Messages is a member of the workspace's
/// hidden direct project, so the project sidebar and channel list for that
/// project must still filter every row by `conversation_visible_to`. Threads
/// under a DM or group take their visibility from it and are named from a
/// message's text, so a listing that skips the rule leaks private words.
#[tokio::test]
async fn sidebar_never_lists_other_peoples_direct_threads() {
    use super::{
        agent_interface::send,
        mcp_admin::{create_project, setup},
        message_edits::{add_person, create_channel, direct_space, say, text},
    };

    let Some(setup) = setup("sidebar_privacy").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let space = direct_space(&setup).await;
    let space_id = space.parse::<Uuid>().expect("space");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (_cleo, cleo_browser) = add_person(&setup, &[&work], "cleo").await;
    // Both open Messages, which makes each a member of the direct project.
    for browser in [&bea_browser, &cleo_browser] {
        let (status, overview) = send(
            &setup.app,
            browser.request("GET", "/api/v1/direct", None, Value::Null),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{overview}");
        assert_eq!(overview["workspaceSpaceId"], space.as_str());
    }
    let agent = add_agent(&setup.pool, space_id, "Mara", "mara").await;

    // The owner and Bea talk in a DM and start a thread from a message.
    let (status, dm) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({ "actorId": bea }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm_id = dm["conversation"]["id"].as_str().expect("dm").to_owned();
    let secret = say(
        &setup,
        &setup.browser,
        &dm_id,
        "dm-secret",
        text("Quarterly layoffs list draft"),
    )
    .await;
    let (status, dm_thread) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{dm_id}/threads"),
            Some("dm-thread"),
            json!({ "originMessageId": secret }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm_thread}");
    let dm_thread_id = dm_thread["conversation"]["id"]
        .as_str()
        .or_else(|| dm_thread["id"].as_str())
        .expect("dm thread")
        .to_owned();

    // A group of the owner, Bea and an agent, with a thread too.
    let (status, group) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some("group"),
            json!({ "memberIds": [bea, agent] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group_id = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let plan = say(
        &setup,
        &setup.browser,
        &group_id,
        "group-secret",
        text("Acquisition codename Bluebird"),
    )
    .await;
    let (status, group_thread) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{group_id}/threads"),
            Some("group-thread"),
            json!({ "originMessageId": plan }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group_thread}");
    let group_thread_id = group_thread["conversation"]["id"]
        .as_str()
        .or_else(|| group_thread["id"].as_str())
        .expect("group thread")
        .to_owned();

    // A project-wide channel thread in the work project, which everyone in
    // it may see: the filter must not hide too much.
    let general = create_channel(&setup, &work, "general").await;
    let public = say(
        &setup,
        &setup.browser,
        &general,
        "public-hello",
        text("Standup notes"),
    )
    .await;
    let (status, public_thread) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/channels/{general}/threads"),
            Some("public-thread"),
            json!({ "originMessageId": public }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{public_thread}");
    let public_thread_id = public_thread["conversation"]["id"]
        .as_str()
        .or_else(|| public_thread["id"].as_str())
        .expect("public thread")
        .to_owned();

    let private_markers = [
        dm_id.as_str(),
        dm_thread_id.as_str(),
        group_id.as_str(),
        group_thread_id.as_str(),
        "layoffs",
        "Bluebird",
    ];
    for uri in [
        format!("/api/v1/projects/{space}/conversation-sidebar?includeArchived=true"),
        format!(
            "/api/v1/projects/{space}/conversation-sidebar?includeArchived=true&selectedChannelId={dm_id}&expandedChannelId={dm_id}"
        ),
        format!(
            "/api/v1/projects/{space}/conversation-sidebar?selectedChannelId={group_id}&expandedChannelId={group_id}"
        ),
        // The direct scope is no space and lists no channels; the spaces
        // Cleo is in show none of it either.
        format!("/api/v1/projects/{work}/channels"),
        format!("/api/v1/projects/{work}/conversation-sidebar?includeArchived=true"),
    ] {
        let (status, body) = send(
            &setup.app,
            cleo_browser.request("GET", &uri, None, Value::Null),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let raw = body.to_string();
        for marker in private_markers {
            assert!(
                !raw.contains(marker),
                "{uri} showed Cleo {marker}, from a conversation she is not in: {raw}"
            );
        }
    }

    // Sanity: the rule itself hides every one of them from Cleo.
    let leaked = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM conversations conversation
        WHERE conversation.project_id = $1
          AND conversation_visible_to(conversation.id, $2)
        "#,
    )
    .bind(space_id)
    .bind(
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'cleo'")
            .fetch_one(&setup.pool)
            .await
            .expect("cleo"),
    )
    .fetch_one(&setup.pool)
    .await
    .expect("visible count");
    assert_eq!(leaked, 0, "Cleo can see nothing in the direct project");

    // A member of the work project still sees its channel and thread.
    let (status, sidebar) = send(
        &setup.app,
        cleo_browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/conversation-sidebar?selectedChannelId={general}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sidebar}");
    let raw = sidebar.to_string();
    assert!(raw.contains(&general), "{raw}");
    assert!(raw.contains(&public_thread_id), "{raw}");
    let (status, channels) = send(
        &setup.app,
        cleo_browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/channels"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{channels}");
    assert!(channels.to_string().contains(&general), "{channels}");

    // Bea, who is in both, can read the DM thread by id.
    let (status, _) = send(
        &setup.app,
        bea_browser.request(
            "GET",
            &format!("/api/v1/conversations/{dm_thread_id}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Since K4.4 `conversation_visible_to` is whole: a member of another
/// workspace no longer passes it for a project-wide channel, so the rule
/// alone and the rule paired with `actor_can_access_project` agree.
/// `conversation_readable_by` answers exactly as `conversation_visible_to`
/// does (ruling R32).
#[tokio::test]
async fn conversation_visible_to_is_whole_on_its_own() {
    use super::{
        mcp_tasks::space,
        message_edits::create_channel,
        support::{
            app::send,
            two_workspaces::add_person,
            world::{create_project, setup},
        },
    };

    let Some(setup) = setup("readable_by").await else {
        return;
    };
    let pool = &setup.pool;
    let project = create_project(&setup, "WORK", "Work").await;
    let channel = create_channel(&setup, &project, "general").await;
    let space = space(&setup).await;
    let member = add_person(&setup, &[&project, &space], "lena", "lena", "Lena").await;
    let owner = owner_actor(pool).await;
    let (status, dm) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({"actorId": member}),
        ),
    )
    .await;
    assert!(status.is_success(), "DM: {status} {dm}");
    let dm = uuid(&dm["conversation"], "id");
    let bystander = add_person(&setup, &[&project, &space], "mara", "mara", "Mara").await;
    // In the workspace, but not in the channel's space.
    let outsider = add_person(&setup, &[&space], "olga", "olga", "Olga").await;

    // A person in another workspace only.
    let stranger = Uuid::now_v7();
    let elsewhere = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(stranger)
        .execute(pool)
        .await
        .expect("stranger");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(elsewhere)
        .bind(format!("elsewhere-{}", elsewhere.simple()))
        .execute(pool)
        .await
        .expect("other workspace");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(elsewhere)
    .bind(stranger)
    .execute(pool)
    .await
    .expect("stranger membership");

    let channel = channel.parse::<Uuid>().expect("channel");
    // A members-only space: only its members read its channels.
    sqlx::query("UPDATE projects SET visibility = 'members' WHERE id = $1")
        .bind(project.parse::<Uuid>().expect("project"))
        .execute(pool)
        .await
        .expect("members-only space");
    let answers = |conversation: Uuid, viewer: Uuid| async move {
        sqlx::query_as::<_, (bool, bool, bool)>(
            r#"
            SELECT conversation_visible_to(conversation.id, $2),
                   -- A DM has no space: the routes pair a membership
                   -- check only where there is one.
                   conversation_visible_to(conversation.id, $2)
                       AND (conversation.project_id IS NULL
                            OR actor_can_access_project($2, NULL, conversation.project_id)),
                   conversation_readable_by(conversation.id, $2)
            FROM conversations conversation -- visibility: the test asks the rule itself
            WHERE conversation.id = $1
            "#,
        )
        .bind(conversation)
        .bind(viewer)
        .fetch_one(pool)
        .await
        .expect("visibility")
    };
    for (what, conversation, viewer, alone, paired) in [
        ("a stranger, the channel", channel, stranger, false, false),
        ("a member, the channel", channel, member, true, true),
        (
            "outside the channel's space",
            channel,
            outsider,
            false,
            false,
        ),
        ("the owner, the channel", channel, owner, true, true),
        ("a member outside the DM", dm, bystander, false, false),
        ("a member in the DM", dm, member, true, true),
    ] {
        let (visible, with_membership, readable) = answers(conversation, viewer).await;
        assert_eq!(visible, alone, "{what}: conversation_visible_to");
        assert_eq!(
            with_membership, paired,
            "{what}: paired with actor_can_access_project"
        );
        assert_eq!(readable, visible, "{what}: conversation_readable_by");
    }
    // An open space's channels are readable by everyone in the workspace by
    // the rule; the routes still ask for membership until K4.6.
    sqlx::query("UPDATE projects SET visibility = 'workspace' WHERE id = $1")
        .bind(project.parse::<Uuid>().expect("project"))
        .execute(pool)
        .await
        .expect("open space");
    let (visible, _, readable) = answers(channel, outsider).await;
    assert!(visible && readable, "open space, a workspace member");
    let (visible, _, _) = answers(channel, stranger).await;
    assert!(!visible, "open space, another workspace");
}

/// A kept DM (ruling R20) ends with its participants' workspace membership; a
/// personal one doesn't, by the rule alone (the routes still check
/// membership until K4.6). No role passes for a DM: neither the owner nor an
/// admin outside it reads it, kept or not.
#[tokio::test]
async fn a_kept_dm_ends_with_workspace_membership_and_no_role_reads_a_dm() {
    use super::support::{
        app::{mcp, send, tool},
        world::WorldBuilder,
    };

    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .person("ben", "admin")
        .agent("mara", "member")
        .dm("ana", "mara")
        .build("kept_dm")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let ana = &world.people["ana"];
    let ben = &world.people["ben"];
    let mara = &world.agents["mara"];
    let owner = world.owner.actor;
    let dm = world.conversations["ana+mara"].clone();
    let dm_id = dm.parse::<Uuid>().expect("DM");
    sqlx::query(
        "UPDATE organization_memberships SET role = 'admin' WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(ben.actor)
    .execute(pool)
    .await
    .expect("ben is a workspace admin");
    let word = format!("keptword{}", Uuid::now_v7().simple());
    let (status, sent) = send(
        &world.app,
        ana.browser.request(
            "POST",
            &format!("/api/v1/conversations/{dm}/messages"),
            Some("kept-dm-message"),
            json!({ "content": text_content(&word) }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    // Kept in the workspace: it is that workspace's now.
    sqlx::query("UPDATE conversations SET kept = true, workspace_id = $2 WHERE id = $1")
        .bind(dm_id)
        .bind(world.organization)
        .execute(pool)
        .await
        .expect("keep the DM");

    let timeline = format!("/api/v1/conversations/{dm}/timeline");
    let search = format!("/api/v1/search?q={word}");
    let read = |token: String, session: String| {
        let app = world.app.clone();
        let dm = dm.clone();
        async move {
            tool(
                &app,
                &token,
                &session,
                "conversation_read",
                json!({ "conversationId": dm }),
            )
            .await
        }
    };

    // While both are in the workspace, both read it.
    assert!(visible(pool, dm_id, ana.actor).await);
    assert!(visible(pool, dm_id, mara.actor).await);
    let (status, shown) = send(
        &world.app,
        ana.browser.request("GET", &timeline, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // A DM has no space, so the author goes by their workspace profile,
    // never "Former member".
    let author = shown.to_string();
    assert!(author.contains(&word), "{author}");
    assert!(!author.contains("Former member"), "{author}");
    let answer = read(mara.token.clone(), mara.session.clone()).await;
    assert!(answer.to_string().contains(&word), "{answer}");

    // Nobody outside it reads it, whatever their role.
    for (who, viewer) in [("the owner", owner), ("an admin", ben.actor)] {
        assert!(!visible(pool, dm_id, viewer).await, "{who}, kept");
    }
    for (who, browser) in [
        ("the owner", &world.owner.browser),
        ("an admin", &ben.browser),
    ] {
        let (status, _) = send(
            &world.app,
            browser.request("GET", &timeline, None, Value::Null),
        )
        .await;
        assert!(
            !status.is_success(),
            "{who} read the DM's timeline: {status}"
        );
        let (_, hits) = send(
            &world.app,
            browser.request("GET", &search, None, Value::Null),
        )
        .await;
        assert!(
            !hits.to_string().contains(&word),
            "{who} found the DM: {hits}"
        );
    }

    // Each participant's workspace membership ends: the kept DM ends with it.
    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = ANY($2)",
    )
    .bind(world.organization)
    .bind(vec![ana.actor, mara.actor])
    .execute(pool)
    .await
    .expect("membership ends");
    assert!(!visible(pool, dm_id, ana.actor).await, "ana, kept");
    assert!(!visible(pool, dm_id, mara.actor).await, "mara, kept");
    let (status, _) = send(
        &world.app,
        ana.browser.request("GET", &timeline, None, Value::Null),
    )
    .await;
    assert!(!status.is_success(), "ana read the kept DM: {status}");
    let (_, hits) = send(
        &world.app,
        ana.browser.request("GET", &search, None, Value::Null),
    )
    .await;
    assert!(
        !hits.to_string().contains(&word),
        "ana found the kept DM: {hits}"
    );
    // Mara's token may be refused outright; either way the answer holds
    // nothing of the DM.
    let response = mcp(
        &world.app,
        &mara.token,
        Some(&mara.session),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "conversation_read", "arguments": { "conversationId": dm } }
        }),
    )
    .await;
    let body = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("MCP body");
    let body = String::from_utf8_lossy(&body);
    assert!(!body.contains(&word), "mara read the kept DM: {body}");

    // A personal DM stays its participants' by the rule alone; still no role
    // passes.
    sqlx::query("UPDATE conversations SET kept = false WHERE id = $1")
        .bind(dm_id)
        .execute(pool)
        .await
        .expect("make it personal");
    assert!(visible(pool, dm_id, ana.actor).await, "ana, personal");
    assert!(visible(pool, dm_id, mara.actor).await, "mara, personal");
    for (who, viewer) in [("the owner", owner), ("an admin", ben.actor)] {
        assert!(!visible(pool, dm_id, viewer).await, "{who}, personal");
    }
    world.finish().await;
}

/// One list everywhere (R20, K4.24): a pair has one personal DM on the
/// server, started from any workspace they share, and every workspace's
/// sidebar shows it. A workspace that keeps its DMs gets its own DM for the
/// pair, listed only there and ending with membership; the personal one
/// stays listed beside it, labelled. A DM from before R20 is personal.
#[tokio::test]
async fn one_dm_per_pair_and_kept_dms_stay_in_their_workspace() {
    use super::support::{app::send, world::WorldBuilder};

    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .person("ben", "member")
        .person("cal", "member")
        .build("one_dm_per_pair")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let (ana, ben, cal) = (
        &world.people["ana"],
        &world.people["ben"],
        &world.people["cal"],
    );
    let first = world.organization;
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Harbour', $2)")
        .bind(second)
        .bind(format!("harbour-{}", second.simple()))
        .execute(pool)
        .await
        .expect("second workspace");
    for actor in [ana.actor, ben.actor] {
        sqlx::query(
            "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(second)
        .bind(actor)
        .execute(pool)
        .await
        .expect("joins the second workspace");
    }
    let open = |browser: &super::support::app::Browser, body: Value| {
        let request = browser.request("POST", "/api/v1/direct/dms", None, body);
        let app = world.app.clone();
        async move {
            let (status, body) = send(&app, request).await;
            assert!(status.is_success(), "{status} {body}");
            (
                status,
                body["conversation"]["id"].as_str().expect("id").to_owned(),
            )
        }
    };
    // The list names the workspace the person is in (K4.6); an account in
    // one workspace may leave it out.
    let listed = |browser: &super::support::app::Browser, workspace: Option<Uuid>| {
        let path = workspace.map_or_else(
            || "/api/v1/direct".to_owned(),
            |workspace| format!("/api/v1/direct?workspaceId={workspace}"),
        );
        let request = browser.request("GET", &path, None, Value::Null);
        let app = world.app.clone();
        async move {
            let (status, body) = send(&app, request).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["conversations"]
                .as_array()
                .expect("conversations")
                .iter()
                .map(|item| {
                    (
                        item["id"].as_str().expect("id").to_owned(),
                        item["label"].as_str().map(str::to_owned),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        }
    };

    // In two workspaces, the list has to be told which one (K4.6).
    let (status, unnamed) = send(
        &world.app,
        ana.browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{unnamed}");

    // From either shared workspace, the same personal DM.
    let (status, personal) = open(&ana.browser, json!({"actorId": ben.actor})).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, again) = open(
        &ana.browser,
        json!({"actorId": ben.actor, "workspaceId": second}),
    )
    .await;
    assert_eq!((status, &again), (StatusCode::OK, &personal));
    for workspace in [first, second] {
        assert_eq!(
            listed(&ben.browser, Some(workspace)).await.get(&personal),
            Some(&None),
            "ben sees the DM in every workspace"
        );
    }

    // The second workspace keeps its DMs: a DM started there is its own.
    sqlx::query(
        "INSERT INTO settings (id, organization_id, scope, key, value) VALUES ($1, $2, 'workspace', 'messages.direct_messages', '\"kept\"')",
    )
    .bind(Uuid::now_v7())
    .bind(second)
    .execute(pool)
    .await
    .expect("keep DMs");
    let (status, kept) = open(
        &ana.browser,
        json!({"actorId": ben.actor, "workspaceId": second}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(kept, personal);
    let (status, kept_again) = open(
        &ana.browser,
        json!({"actorId": ben.actor, "workspaceId": second}),
    )
    .await;
    assert_eq!((status, &kept_again), (StatusCode::OK, &kept));
    let (workspace, is_kept) = sqlx::query_as::<_, (Uuid, bool)>(
        "SELECT workspace_id, kept FROM conversations WHERE id = $1::uuid",
    )
    .bind(&kept)
    .fetch_one(pool)
    .await
    .expect("kept DM");
    assert_eq!((workspace, is_kept), (second, true));
    // Started from the first workspace, which doesn't keep DMs: the personal one.
    let (_, from_first) = open(
        &ana.browser,
        json!({"actorId": ben.actor, "workspaceId": first}),
    )
    .await;
    assert_eq!(from_first, personal);

    // Listed only there, with the personal one beside it, labelled.
    let there = listed(&ana.browser, Some(second)).await;
    assert_eq!(there.get(&kept), Some(&None), "{there:?}");
    assert_eq!(
        there.get(&personal),
        Some(&Some("Personal".to_owned())),
        "{there:?}"
    );
    let elsewhere = listed(&ana.browser, Some(first)).await;
    assert!(!elsewhere.contains_key(&kept), "{elsewhere:?}");
    assert_eq!(elsewhere.get(&personal), Some(&None), "{elsewhere:?}");

    // A DM from before R20 (it carries a workspace, not the direct scope)
    // is personal: listed in every workspace, and it is the pair's DM.
    let old = Uuid::now_v7();
    let (low, high) = if ana.actor < cal.actor {
        (ana.actor, cal.actor)
    } else {
        (cal.actor, ana.actor)
    };
    sqlx::query(
        "INSERT INTO conversations (id, project_id, workspace_id, kind, name, visibility, dm_key) VALUES ($1, NULL, $2, 'dm', 'Direct message', 'members', $3)",
    )
    .bind(old)
    .bind(first)
    .bind(format!("{low}:{high}"))
    .execute(pool)
    .await
    .expect("pre-R20 DM");
    for actor in [ana.actor, cal.actor] {
        sqlx::query(
            "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES (NULL, $1, $2, $2)",
        )
        .bind(old)
        .bind(actor)
        .execute(pool)
        .await
        .expect("pre-R20 member");
    }
    for workspace in [first, second] {
        assert!(
            listed(&ana.browser, Some(workspace))
                .await
                .contains_key(&old.to_string())
        );
    }
    let (status, found) = open(&ana.browser, json!({"actorId": cal.actor})).await;
    assert_eq!((status, found), (StatusCode::OK, old.to_string()));

    // The kept DM ends with ben's membership; the personal one doesn't.
    assert!(listed(&ben.browser, Some(second)).await.contains_key(&kept));
    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(second)
    .bind(ben.actor)
    .execute(pool)
    .await
    .expect("ben leaves");
    let kept_id = kept.parse::<Uuid>().expect("kept id");
    assert!(
        !visible(pool, kept_id, ben.actor).await,
        "ben, after leaving"
    );
    // Now in one workspace, ben needn't name it.
    let after = listed(&ben.browser, None).await;
    assert!(!after.contains_key(&kept), "{after:?}");
    assert_eq!(after.get(&personal), Some(&None), "{after:?}");
    world.finish().await;
}

/// The timings in the K4.4 migration's header: the conversation list and
/// message search, with the rule as it was and as K4.4 made it whole.
/// 50,000 messages over 200 channels and 200 DMs, one viewer. Run by hand:
/// `cargo test --test integration conversation_visibility_timings -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "timings, run by hand"]
async fn conversation_visibility_timings() {
    use super::support::world::WorldBuilder;

    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .person("ben", "member")
        .dm("ana", "ben")
        .build("visibility_timings")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let ana = world.people["ana"].actor;
    let ben = world.people["ben"].actor;
    let work = world.project("WORK").parse::<Uuid>().expect("project");
    let space = sqlx::query_scalar::<_, Uuid>(
        "SELECT project_id FROM conversations WHERE id = $1 -- visibility: the test's own DM",
    )
    .bind(world.conversations["ana+ben"].parse::<Uuid>().expect("DM"))
    .fetch_one(pool)
    .await
    .expect("space");
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name, visibility)
        SELECT gen_random_uuid(), $1, 'channel', 'timing-' || g,
               CASE WHEN g % 4 = 0 THEN 'members' ELSE 'project' END
        FROM generate_series(1, 200) g
        "#,
    )
    .bind(work)
    .execute(pool)
    .await
    .expect("channels");
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name, visibility, dm_key)
        SELECT gen_random_uuid(), $1, 'dm', 'Direct message', 'members', 'timing-' || g
        FROM generate_series(1, 200) g
        "#,
    )
    .bind(space)
    .execute(pool)
    .await
    .expect("DMs");
    // Ana is in every other DM and every other private channel.
    sqlx::query(
        r#"
        INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
        SELECT c.project_id, c.id, member, $1
        FROM conversations c -- visibility: seeding the test's rows
        CROSS JOIN LATERAL (VALUES ($1), ($2)) AS members (member)
        WHERE c.name LIKE 'timing-%' OR c.dm_key LIKE 'timing-%'
        "#,
    )
    .bind(ben)
    .bind(ana)
    .execute(pool)
    .await
    .expect("members");
    sqlx::query(
        r#"
        DELETE FROM conversation_members m
        USING conversations c -- visibility: seeding the test's rows
        WHERE c.id = m.conversation_id AND m.actor_id = $1
          AND (c.dm_key LIKE 'timing-%' OR c.name LIKE 'timing-%')
          AND get_byte(uuid_send(c.id), 15) % 2 = 0
        "#,
    )
    .bind(ana)
    .execute(pool)
    .await
    .expect("half the memberships");
    sqlx::query(
        r#"
        INSERT INTO messages (id, project_id, conversation_id, author_actor_id, content, plain_text)
        SELECT gen_random_uuid(), c.project_id, c.id, $1,
               '{"version":1,"nodes":[]}', 'status update number ' || g
        FROM (SELECT id, project_id, row_number() OVER (ORDER BY id) - 1 AS n
              FROM conversations -- visibility: seeding the test's rows
              WHERE name LIKE 'timing-%' OR dm_key LIKE 'timing-%') c
        JOIN generate_series(0, 49999) g ON g % 400 = c.n
        "#,
    )
    .bind(ben)
    .execute(pool)
    .await
    .expect("messages");
    sqlx::query("ANALYZE").execute(pool).await.expect("analyze");
    // The rule as it was before K4.4 (migration 0037), for comparison.
    sqlx::query(
        r#"
        CREATE FUNCTION conversation_visible_before(target UUID, viewer UUID)
        RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
            SELECT EXISTS (
                SELECT 1 FROM conversations conversation
                LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
                WHERE conversation.id = target
                  AND (COALESCE(parent.visibility, conversation.visibility) = 'project'
                       OR EXISTS (SELECT 1 FROM conversation_members member
                                  WHERE member.conversation_id =
                                        COALESCE(conversation.parent_channel_id, conversation.id)
                                    AND member.actor_id = viewer)))
        $$
        "#,
    )
    .execute(pool)
    .await
    .expect("old rule");

    let time = |sql: String| async move {
        let mut runs = Vec::new();
        for _ in 0..7 {
            let plan =
                sqlx::query_scalar::<_, Value>(&format!("EXPLAIN (ANALYZE, FORMAT JSON) {sql}"))
                    .bind(ana)
                    .bind(vec![work, space])
                    .fetch_one(pool)
                    .await
                    .expect("explain");
            runs.push(plan[0]["Execution Time"].as_f64().expect("time"));
        }
        runs.sort_by(f64::total_cmp);
        runs[runs.len() / 2]
    };
    for rule in ["conversation_visible_before", "conversation_visible_to"] {
        let list = time(format!(
            "SELECT c.id FROM conversations c WHERE c.project_id = ANY($2) AND {rule}(c.id, $1)"
        ))
        .await;
        let search = time(format!(
            "SELECT m.id FROM messages m WHERE m.project_id = ANY($2) \
             AND m.search_vector @@ plainto_tsquery('english', 'status update') \
             AND {rule}(m.conversation_id, $1)"
        ))
        .await;
        println!("{rule}: conversation list {list:.1} ms, message search {search:.1} ms");
    }
    world.finish().await;
}

/// Across two workspaces: a handle two accounts go by names nobody, a token
/// for one workspace doesn't find the other's kept DMs in search, and a
/// decision made from a DM pins there.
#[tokio::test]
async fn handles_tokens_and_pins_hold_across_workspaces() {
    use super::support::{
        app::{initialize, send, tool},
        world::WorldBuilder,
    };

    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .person("ben", "member")
        .person("cal", "member")
        .agent("dex", "member")
        .build("dm_across_workspaces")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let (ana, ben, cal) = (
        &world.people["ana"],
        &world.people["ben"],
        &world.people["cal"],
    );
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Harbour', $2)")
        .bind(second)
        .bind(format!("harbour-{}", second.simple()))
        .execute(pool)
        .await
        .expect("second workspace");
    for actor in [ana.actor, ben.actor] {
        sqlx::query(
            "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(second)
        .bind(actor)
        .execute(pool)
        .await
        .expect("joins the second workspace");
    }
    sqlx::query(
        "INSERT INTO settings (id, organization_id, scope, key, value) VALUES ($1, $2, 'workspace', 'messages.direct_messages', '\"kept\"')",
    )
    .bind(Uuid::now_v7())
    .bind(second)
    .execute(pool)
    .await
    .expect("keep DMs");
    let open = async |body: Value| {
        let (status, dm) = send(
            &world.app,
            ana.browser
                .request("POST", "/api/v1/direct/dms", None, body),
        )
        .await;
        assert!(status.is_success(), "{status} {dm}");
        dm["conversation"]["id"].as_str().expect("id").to_owned()
    };
    let personal = open(json!({"actorId": ben.actor})).await;
    let kept = open(json!({"actorId": ben.actor, "workspaceId": second})).await;
    assert_ne!(personal, kept);
    let say = async |dm: &str, word: &str| {
        let (status, sent) = send(
            &world.app,
            ana.browser.request(
                "POST",
                &format!("/api/v1/conversations/{dm}/messages"),
                Some(word),
                json!({ "content": text_content(word) }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{sent}");
    };
    let personal_word = format!("personalword{}", Uuid::now_v7().simple());
    let kept_word = format!("keptword{}", Uuid::now_v7().simple());
    say(&personal, &personal_word).await;
    say(&kept, &kept_word).await;
    // Ben mentions Ana in the kept DM.
    let (status, mentioned) = send(
        &world.app,
        ben.browser.request(
            "POST",
            &format!("/api/v1/conversations/{kept}/messages"),
            Some("ben-mentions-ana"),
            json!({ "content": {"version": 1, "nodes": [
                {"type": "mention", "actorId": ana.actor},
                {"type": "text", "text": " kept mention"}
            ]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mentioned}");

    // Ana's account-wide token for the first workspace.
    let (status, made) = send(
        &world.app,
        ana.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/credential_create", world.organization),
            Some("ana-account"),
            json!({"name": "account", "space": world.project("WORK"), "reach": "account"}),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let secret = made["value"]["secret"].as_str().expect("secret");
    let (status, exchanged) = send(
        &world.app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(
                serde_urlencoded::to_string([
                    ("grant_type", "client_credentials"),
                    ("client_secret", secret),
                ])
                .expect("form"),
            ))
            .expect("token request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{exchanged}");
    let token = exchanged["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    let session = initialize(&world.app, &token).await.expect("session");
    let call = async |name: &str, arguments: Value| {
        tool(&world.app, &token, &session, name, arguments).await
    };

    // Search finds the personal DM, never the second workspace's kept one.
    let found = |answer: &Value| answer["value"]["results"].as_array().map_or(0, Vec::len);
    let personal_hits = call("workspace_search", json!({"query": personal_word})).await;
    assert_eq!(found(&personal_hits), 1, "{personal_hits}");
    let kept_hits = call("workspace_search", json!({"query": kept_word})).await;
    assert_eq!(
        found(&kept_hits),
        0,
        "a token found another workspace's DM: {kept_hits}"
    );

    // Ben goes by "cal" in the second workspace, cal by "cal" in the first:
    // "@cal" names nobody for sure, so nothing is sent.
    sqlx::query(
        "INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle) \
         VALUES ($1, $2, 'Ben', 'cal') \
         ON CONFLICT (organization_id, actor_id) DO UPDATE SET handle = 'cal'",
    )
    .bind(second)
    .bind(ben.actor)
    .execute(pool)
    .await
    .expect("ben goes by cal there");
    let handle_rows = sqlx::query_scalar::<_, i64>(
        "SELECT count(DISTINCT actor_id) FROM organization_actor_profiles WHERE lower(handle) = 'cal'",
    )
    .fetch_one(pool)
    .await
    .expect("handles");
    assert_eq!(handle_rows, 2, "two accounts go by cal");
    let ambiguous = call(
        "conversation_start",
        json!({"people": ["@cal"], "text": "for cal"}),
    )
    .await;
    assert_ne!(ambiguous["outcome"], "ok", "{ambiguous}");
    assert!(
        ambiguous.to_string().contains("More than one account"),
        "{ambiguous}"
    );
    let by_id = call(
        "conversation_start",
        json!({"people": [cal.actor.to_string()], "text": "for cal"}),
    )
    .await;
    assert_eq!(by_id["outcome"], "ok", "{by_id}");

    // Listing conversations shows the personal DM, never the second
    // workspace's kept one.
    let listed = call("conversation_list", json!({"kind": "dm", "limit": 50})).await;
    let listed_ids = listed["value"]["conversations"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(listed_ids.contains(&personal), "{listed}");
    assert!(
        !listed_ids.contains(&kept),
        "a token listed another workspace's DM: {listed}"
    );

    // Nor can it open the kept DM with a call that needs no permission.
    let files = call("conversation_file_list", json!({"conversationId": kept})).await;
    assert_ne!(
        files["outcome"], "ok",
        "a token listed another workspace's DM's files: {files}"
    );

    // A token for the second workspace sends a file in the kept DM; the
    // first workspace's token can't read it, alone or by its message.
    let (status, exchanged_second) = send(
        &world.app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(
                serde_urlencoded::to_string([
                    ("grant_type", "client_credentials"),
                    ("client_secret", secret),
                    ("workspace", second.to_string().as_str()),
                ])
                .expect("form"),
            ))
            .expect("token request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{exchanged_second}");
    let second_token = exchanged_second["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    let second_session = initialize(&world.app, &second_token)
        .await
        .expect("session");
    let upload = tool(
        &world.app,
        &second_token,
        &second_session,
        "file_upload",
        json!({"conversationId": kept, "name": "kept.txt", "text": "kept file"}),
    )
    .await;
    assert_eq!(upload["outcome"], "ok", "{upload}");
    let kept_file = upload["value"]["fileId"].as_str().expect("file").to_owned();
    let posted = tool(
        &world.app,
        &second_token,
        &second_session,
        "message_send",
        json!({"conversationId": kept, "text": "a file", "clientRequestId": "kept-file",
               "attachmentIds": [kept_file]}),
    )
    .await;
    assert!(
        matches!(posted["outcome"].as_str(), Some("ok" | "applied")),
        "{posted}"
    );
    let read_file = call("file_read", json!({"fileId": kept_file})).await;
    assert_ne!(
        read_file["outcome"], "ok",
        "a token read another workspace's DM file: {read_file}"
    );
    assert!(!read_file.to_string().contains("kept file"), "{read_file}");

    // Saving a DM message leaves Later readable for an account-wide token.
    let personal_message: Uuid = sqlx::query_scalar(
        "SELECT id FROM messages WHERE conversation_id = $1 ORDER BY created_at LIMIT 1",
    )
    .bind(personal.parse::<Uuid>().expect("dm"))
    .fetch_one(pool)
    .await
    .expect("personal message");
    let saved = call("later_save", json!({"messageId": personal_message})).await;
    assert_eq!(saved["outcome"], "ok", "{saved}");
    let later = call("later_list", json!({})).await;
    assert_eq!(later["outcome"], "ok", "{later}");

    // Nor does the briefing bring the kept DM's mention.
    let briefing = call("briefing_get", json!({})).await;
    assert_eq!(briefing["outcome"], "ok", "{briefing}");
    assert!(
        !briefing.to_string().contains(&kept),
        "a token's briefing named another workspace's DM: {briefing}"
    );

    // The token is for the first workspace: the second's groups (a call
    // that needs no permission) stay out of its reach.
    let elsewhere = call("group_list", json!({"workspace": second.to_string()})).await;
    assert_ne!(elsewhere["outcome"], "ok", "{elsewhere}");
    let at_home = call(
        "group_list",
        json!({"workspace": world.organization.to_string()}),
    )
    .await;
    assert_eq!(at_home["outcome"], "ok", "{at_home}");

    // A decision made from the personal DM pins there.
    let decided = call(
        "decision_record",
        json!({"conversationId": personal, "title": "Ship on Friday", "pin": true}),
    )
    .await;
    assert_eq!(decided["outcome"], "ok", "{decided}");
    let pinned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM conversation_pins WHERE conversation_id = $1::uuid AND target_kind = 'decision')",
    )
    .bind(&personal)
    .fetch_one(pool)
    .await
    .expect("pins");
    assert!(pinned, "the decision is pinned in the DM");

    // An agent's secret made for one space never makes a wider one.
    let dex = &world.agents["dex"];
    let narrow_secret = dex.token.clone();
    let narrow_call = async |name: &str, arguments: Value| {
        let (_, answer) = send(
            &world.app,
            Request::builder()
                .method("POST")
                .uri(format!("/api/v2/w/{}/ops/{name}", world.organization))
                .header(header::AUTHORIZATION, format!("Bearer {narrow_secret}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    "idempotency-key",
                    format!("narrow-{name}-{}", Uuid::now_v7()),
                )
                .body(Body::from(arguments.to_string()))
                .expect("op request"),
        )
        .await;
        answer
    };
    let wider = narrow_call(
        "credential_create",
        json!({"name": "wider", "space": world.project("WORK"), "reach": "account"}),
    )
    .await;
    assert_ne!(
        wider["outcome"], "ok",
        "a space's secret made an account-wide one: {wider}"
    );
    let same = narrow_call(
        "credential_create",
        json!({"name": "same", "space": world.project("WORK")}),
    )
    .await;
    assert_eq!(same["outcome"], "ok", "{same}");

    world.finish().await;
}

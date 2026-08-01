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

#[tokio::test]
async fn authenticated_conversation_http_flow_is_scoped_idempotent_and_paginated() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let administrative_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("m2_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M2 HTTP schema");
    let pool = schema_pool(&database_url, &schema).await;
    for migration in [
        include_str!("../migrations/0001_foundation.sql"),
        include_str!("../migrations/0002_walking_skeleton.sql"),
        include_str!("../migrations/0003_m0_consistency.sql"),
        include_str!("../migrations/0004_m1_identity_projects.sql"),
        include_str!("../migrations/0005_m2_conversations.sql"),
        include_str!("../migrations/0006_m2_message_mentions_projection.sql"),
        include_str!("../migrations/0007_m2_archived_parent_navigation.sql"),
        include_str!("../migrations/0008_m2_conversation_lock_boundaries.sql"),
        include_str!("../migrations/0009_m3_execution_foundation.sql"),
        include_str!("../migrations/0010_m3_execution_admission_hardening.sql"),
        include_str!("../migrations/0011_m3_canonical_repository_mirrors.sql"),
        include_str!("../migrations/0012_m3_agent_run_scheduler.sql"),
        include_str!("../migrations/0013_m3_provider_event_idempotency.sql"),
        include_str!("../migrations/0014_m3_run_acknowledgements.sql"),
        include_str!("../migrations/0015_m4_work_items_foundation.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(&pool)
            .await
            .expect("apply migration");
    }
    let bootstrapped = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "m2-owner".to_owned(),
            display_name: "M2 Owner".to_owned(),
            organization_name: Some("M2 Organization".to_owned()),
            organization_slug: Some(format!("m2-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        "correct horse battery staple".to_owned(),
    )
    .await
    .expect("bootstrap M2 owner");
    let app = test_app(pool.clone(), &database_url);
    let (cookie, csrf) = login(&app).await;
    let project_response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/projects",
            &write_headers(&cookie, &csrf, Some("m2-project")),
            json!({
                "organizationId": bootstrapped.organization_id,
                "key": "m2",
                "name": "M2 Conversations"
            }),
        ))
        .await
        .expect("create Project");
    assert_eq!(project_response.status(), StatusCode::CREATED);
    let project = response_json(project_response).await;
    let project_id = uuid_field(&project, "id");

    let worker_actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'execution_worker')")
        .bind(worker_actor_id)
        .execute(&pool)
        .await
        .expect("insert Worker Actor");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'worker', 'Execution Worker', 'agent')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(worker_actor_id)
    .execute(&pool)
    .await
    .expect("insert Worker membership");

    let missing_csrf = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/projects/{project_id}/channels"),
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie.as_str()),
                ("idempotency-key", "missing-csrf"),
            ],
            json!({"name": "General"}),
        ))
        .await
        .expect("missing CSRF response");
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let create_channel_uri = format!("/api/v1/projects/{project_id}/channels");
    let create_channel = app
        .clone()
        .oneshot(request(
            "POST",
            &create_channel_uri,
            &write_headers(&cookie, &csrf, Some("create-general")),
            json!({"name": " General "}),
        ))
        .await
        .expect("create Channel");
    assert_eq!(create_channel.status(), StatusCode::CREATED);
    let channel = response_json(create_channel).await;
    let channel_id = uuid_field(&channel["conversation"], "id");
    assert_eq!(channel["conversation"]["kind"], "channel");
    assert_eq!(channel["conversation"]["name"], "General");

    let channel_replay = app
        .clone()
        .oneshot(request(
            "POST",
            &create_channel_uri,
            &write_headers(&cookie, &csrf, Some("create-general")),
            json!({"name": " General "}),
        ))
        .await
        .expect("replay Channel create");
    assert_eq!(channel_replay.status(), StatusCode::CREATED);
    assert_eq!(
        uuid_field(&response_json(channel_replay).await["conversation"], "id"),
        channel_id
    );
    let channel_mismatch = app
        .clone()
        .oneshot(request(
            "POST",
            &create_channel_uri,
            &write_headers(&cookie, &csrf, Some("create-general")),
            json!({"name": "Different"}),
        ))
        .await
        .expect("mismatched Channel retry");
    assert_eq!(channel_mismatch.status(), StatusCode::CONFLICT);

    let mention_first = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/projects/{project_id}/mentionable-actors?limit=1"),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("first mention page");
    assert_eq!(mention_first.status(), StatusCode::OK);
    let mention_first = response_json(mention_first).await;
    assert_eq!(mention_first["actors"].as_array().expect("Actors").len(), 1);
    let mention_cursor = mention_first["nextCursor"]
        .as_str()
        .expect("mention cursor");
    let mention_second = app
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/projects/{project_id}/mentionable-actors?limit=1&cursor={mention_cursor}"
            ),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("second mention page");
    assert_eq!(mention_second.status(), StatusCode::OK);
    assert_eq!(
        response_json(mention_second).await["actors"]
            .as_array()
            .expect("Actors")
            .len(),
        1
    );

    let message_uri = format!("/api/v1/channels/{channel_id}/messages");
    let first_content = json!({
        "version": 1,
        "nodes": [
            {"type": "text", "text": "Hello "},
            {"type": "mention", "actorId": worker_actor_id}
        ]
    });
    let first_message = app
        .clone()
        .oneshot(request(
            "POST",
            &message_uri,
            &write_headers(&cookie, &csrf, Some("message-one")),
            json!({"content": first_content}),
        ))
        .await
        .expect("send first Message");
    assert_eq!(first_message.status(), StatusCode::CREATED);
    let first_message = response_json(first_message).await;
    let first_message_id = uuid_field(&first_message["message"], "id");
    assert_eq!(
        first_message["message"]["author"]["id"],
        bootstrapped.actor_id.to_string()
    );
    assert_eq!(first_message["message"]["plainText"], "Hello @agent");
    assert_eq!(first_message["entry"]["sequence"], 1);
    assert_eq!(
        first_message["message"]["mentionedActors"]
            .as_array()
            .expect("mentioned Actors")
            .len(),
        1
    );

    let message_replay = app
        .clone()
        .oneshot(request(
            "POST",
            &message_uri,
            &write_headers(&cookie, &csrf, Some("message-one")),
            json!({"content": {
                "version": 1,
                "nodes": [
                    {"type": "text", "text": "Hello "},
                    {"type": "mention", "actorId": worker_actor_id}
                ]
            }}),
        ))
        .await
        .expect("replay Message");
    assert_eq!(message_replay.status(), StatusCode::CREATED);
    assert_eq!(
        uuid_field(&response_json(message_replay).await["message"], "id"),
        first_message_id
    );
    let spoofed_author = app
        .clone()
        .oneshot(request(
            "POST",
            &message_uri,
            &write_headers(&cookie, &csrf, Some("spoofed-author")),
            json!({
                "authorActorId": worker_actor_id,
                "content": {"version":1,"nodes":[{"type":"text","text":"spoof"}]}
            }),
        ))
        .await
        .expect("spoofed author response");
    assert_eq!(spoofed_author.status(), StatusCode::BAD_REQUEST);

    for (key, text) in [("message-two", "Second"), ("message-three", "Third")] {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &message_uri,
                &write_headers(&cookie, &csrf, Some(key)),
                json!({"content": {"version":1,"nodes":[{"type":"text","text":text}]}}),
            ))
            .await
            .expect("send paginated Message");
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let first_page = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/conversations/{channel_id}/timeline?limit=2"),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("read first timeline page");
    assert_eq!(first_page.status(), StatusCode::OK);
    let first_page = response_json(first_page).await;
    assert_eq!(entry_sequences(&first_page), vec![2, 3]);
    assert_eq!(first_page["highestSequence"], 3);
    let timeline_cursor = first_page["nextCursor"].as_str().expect("timeline cursor");
    let second_page = app
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/conversations/{channel_id}/timeline?limit=2&cursor={timeline_cursor}"
            ),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("read older timeline page");
    assert_eq!(second_page.status(), StatusCode::OK);
    let second_page = response_json(second_page).await;
    assert_eq!(entry_sequences(&second_page), vec![1]);
    assert!(second_page["nextCursor"].is_null());

    let mark_seen_uri = format!("/api/v1/conversations/{channel_id}/mark-seen");
    let seen = app
        .clone()
        .oneshot(request(
            "POST",
            &mark_seen_uri,
            &write_headers(&cookie, &csrf, None),
            json!({"highestSeenSequence": 3, "expectedVersion": 1}),
        ))
        .await
        .expect("mark seen");
    assert_eq!(seen.status(), StatusCode::OK);
    let seen = response_json(seen).await;
    assert_eq!(seen["highestSeenSequence"], 3);
    assert_eq!(seen["version"], 2);
    let stale_seen = app
        .clone()
        .oneshot(request(
            "POST",
            &mark_seen_uri,
            &write_headers(&cookie, &csrf, None),
            json!({"highestSeenSequence": 1, "expectedVersion": 1}),
        ))
        .await
        .expect("stale mark seen");
    assert_eq!(stale_seen.status(), StatusCode::OK);
    assert_eq!(response_json(stale_seen).await["highestSeenSequence"], 3);

    let create_thread_uri = format!("/api/v1/channels/{channel_id}/threads");
    let thread = app
        .clone()
        .oneshot(request(
            "POST",
            &create_thread_uri,
            &write_headers(&cookie, &csrf, Some("thread-one")),
            json!({"name": "Focused work", "originMessageId": first_message_id}),
        ))
        .await
        .expect("create Thread");
    assert_eq!(thread.status(), StatusCode::CREATED);
    let thread = response_json(thread).await;
    let thread_id = uuid_field(&thread["conversation"], "id");
    assert_eq!(
        thread["conversation"]["parentChannelId"],
        channel_id.to_string()
    );
    assert_eq!(
        thread["conversation"]["originMessageId"],
        first_message_id.to_string()
    );

    // A later-created empty Thread must not outrank an older Thread whose
    // newest durable Conversation entry is more recent. Message append does
    // not mutate conversations.updated_at, so this catches stale ordering.
    let later_empty_thread = app
        .clone()
        .oneshot(request(
            "POST",
            &create_thread_uri,
            &write_headers(&cookie, &csrf, Some("thread-two-empty")),
            json!({"name": "Later empty work"}),
        ))
        .await
        .expect("create later empty Thread");
    assert_eq!(later_empty_thread.status(), StatusCode::CREATED);
    let later_empty_thread_id = uuid_field(
        &response_json(later_empty_thread).await["conversation"],
        "id",
    );
    let thread_message = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/threads/{thread_id}/messages"),
            &write_headers(&cookie, &csrf, Some("thread-activity-message")),
            json!({"content":{"version":1,"nodes":[{"type":"text","text":"Latest durable activity"}]}}),
        ))
        .await
        .expect("append durable Thread activity");
    assert_eq!(thread_message.status(), StatusCode::CREATED);
    let thread_seen = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/conversations/{thread_id}/mark-seen"),
            &write_headers(&cookie, &csrf, None),
            json!({"highestSeenSequence": 1}),
        ))
        .await
        .expect("mark durable Thread activity seen");
    assert_eq!(thread_seen.status(), StatusCode::OK);

    let run_uri = format!("/api/v1/threads/{thread_id}/agent-runs");
    for (index, rejected_prompt) in ["unsafe\u{0000}prompt".to_owned(), "🙂".repeat(20_001)]
        .into_iter()
        .enumerate()
    {
        let idempotency_key = format!("invalid-run-prompt-{index}");
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &run_uri,
                &write_headers(&cookie, &csrf, Some(&idempotency_key)),
                json!({"prompt": rejected_prompt}),
            ))
            .await
            .expect("reject invalid hosted prompt");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let spoofed_run_author = app
        .clone()
        .oneshot(request(
            "POST",
            &run_uri,
            &write_headers(&cookie, &csrf, Some("spoofed-run-author")),
            json!({
                "prompt": "valid prompt",
                "authorActorId": worker_actor_id
            }),
        ))
        .await
        .expect("reject hosted prompt authorship");
    assert_eq!(spoofed_run_author.status(), StatusCode::BAD_REQUEST);
    let exact_scalar_boundary = app
        .clone()
        .oneshot(request(
            "POST",
            &run_uri,
            &write_headers(&cookie, &csrf, Some("exact-run-prompt-boundary")),
            json!({"prompt": "🙂".repeat(20_000)}),
        ))
        .await
        .expect("accept hosted prompt scalar boundary");
    assert_eq!(
        exact_scalar_boundary.status(),
        StatusCode::CONFLICT,
        "the exact Message boundary must pass validation and reach repository selection"
    );

    let sidebar = app
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/projects/{project_id}/conversation-sidebar?selectedChannelId={channel_id}"
            ),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("read Conversation sidebar");
    let sidebar_status = sidebar.status();
    let sidebar = response_json(sidebar).await;
    assert_eq!(
        sidebar_status,
        StatusCode::OK,
        "sidebar response: {sidebar}"
    );
    assert_eq!(sidebar["channels"][0]["id"], channel_id.to_string());
    assert_eq!(
        sidebar["channels"][0]["threads"][0]["id"],
        thread_id.to_string()
    );
    assert_eq!(
        sidebar["channels"][0]["threads"][1]["id"],
        later_empty_thread_id.to_string()
    );
    assert_eq!(
        sidebar["channels"][0]["threads"][0]["attention"]["label"],
        "Seen"
    );

    let archived = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/threads/{thread_id}/archive"),
            &write_headers(&cookie, &csrf, Some("archived-parent-run")),
            json!({"expectedVersion": 1}),
        ))
        .await
        .expect("archive Thread");
    assert_eq!(archived.status(), StatusCode::OK);
    let archived = response_json(archived).await;
    assert_eq!(archived["conversation"]["version"], 2);
    assert_eq!(archived["fallback"]["kind"], "channel");
    assert_eq!(
        archived["fallback"]["conversationId"],
        channel_id.to_string()
    );

    let hidden_archive = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/conversations/{thread_id}/timeline"),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("hidden archived Thread");
    assert_eq!(hidden_archive.status(), StatusCode::NOT_FOUND);
    let archived_history = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/conversations/{thread_id}/timeline?includeArchived=true"),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("archived Thread history");
    if archived_history.status() != StatusCode::OK {
        panic!(
            "archived Thread history failed: {}",
            response_json(archived_history).await
        );
    }

    let scalar_boundary_message = app
        .clone()
        .oneshot(request(
            "POST",
            &message_uri,
            &write_headers(&cookie, &csrf, Some("message-unicode-scalar-boundary")),
            json!({
                "content": {
                    "version": 1,
                    "nodes": [{"type": "text", "text": "🙂".repeat(20_000)}]
                }
            }),
        ))
        .await
        .expect("send exact Unicode scalar boundary Message");
    assert_eq!(scalar_boundary_message.status(), StatusCode::CREATED);

    let preserved_child = app
        .clone()
        .oneshot(request(
            "POST",
            &create_thread_uri,
            &write_headers(&cookie, &csrf, Some("thread-preserved-with-parent")),
            json!({"name": "Preserved child history"}),
        ))
        .await
        .expect("create child Thread before parent archive");
    assert_eq!(preserved_child.status(), StatusCode::CREATED);
    let preserved_child_id =
        uuid_field(&response_json(preserved_child).await["conversation"], "id");
    let remember_child = app
        .clone()
        .oneshot(request(
            "PUT",
            &format!("/api/v1/projects/{project_id}/navigation"),
            &write_headers(&cookie, &csrf, None),
            json!({
                "location": {
                    "kind": "thread",
                    "conversationId": preserved_child_id
                },
                "expectedVersion": project["navigation"]["version"]
            }),
        ))
        .await
        .expect("remember child Thread navigation");
    assert_eq!(remember_child.status(), StatusCode::OK);
    let archive_channel = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/channels/{channel_id}/archive"),
            &write_headers(&cookie, &csrf, None),
            json!({"expectedVersion": 1}),
        ))
        .await
        .expect("archive parent Channel");
    assert_eq!(archive_channel.status(), StatusCode::OK);

    let projects_after_parent_archive = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/projects",
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("list Projects after parent Channel archive");
    assert_eq!(projects_after_parent_archive.status(), StatusCode::OK);
    let projects_after_parent_archive = response_json(projects_after_parent_archive).await;
    let listed_project = projects_after_parent_archive["projects"]
        .as_array()
        .expect("Projects")
        .iter()
        .find(|listed| listed["id"] == project_id.to_string())
        .expect("listed Project");
    assert_eq!(
        listed_project["navigation"]["location"]["kind"],
        "project_overview"
    );
    assert!(
        listed_project["navigation"]["location"]
            .get("conversationId")
            .is_none(),
        "an active child of an archived parent cannot remain the remembered route"
    );

    let archived_sidebar = app
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/projects/{project_id}/conversation-sidebar?includeArchived=true&expandedChannelId={channel_id}"
            ),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("read archived Conversation projection");
    assert_eq!(archived_sidebar.status(), StatusCode::OK);
    let archived_sidebar = response_json(archived_sidebar).await;
    let archived_channel = archived_sidebar["channels"]
        .as_array()
        .expect("archived Channels")
        .iter()
        .find(|channel| channel["id"] == channel_id.to_string())
        .expect("archived parent Channel");
    let preserved_child = archived_channel["threads"]
        .as_array()
        .expect("archived child Threads")
        .iter()
        .find(|thread| thread["id"] == preserved_child_id.to_string())
        .expect("active child retained under archived parent");
    assert!(preserved_child["archivedAt"].is_null());
    assert!(preserved_child["parentArchivedAt"].is_string());

    let child_hidden = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/conversations/{preserved_child_id}/timeline"),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("hide child of archived parent from active timeline");
    assert_eq!(child_hidden.status(), StatusCode::NOT_FOUND);
    let child_history = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/conversations/{preserved_child_id}/timeline?includeArchived=true"),
            &[("cookie", cookie.as_str())],
            Value::Null,
        ))
        .await
        .expect("read child history through archived parent");
    assert_eq!(child_history.status(), StatusCode::OK);

    let runs_before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .expect("count runs before archived-parent mutation");
    let rejected_run = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/threads/{preserved_child_id}/agent-runs"),
            &write_headers(&cookie, &csrf, Some("archived-parent-run")),
            json!({"prompt": "must remain read-only"}),
        ))
        .await
        .expect("reject run under archived parent");
    assert_eq!(rejected_run.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .expect("count runs after archived-parent mutation"),
        runs_before,
        "archived parent rejection must have no durable run side effects"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE conversation_id = $1")
            .bind(channel_id)
            .fetch_one(&pool)
            .await
            .expect("count idempotent Messages"),
        4
    );
    let outbox_counts = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT event_type, count(*)
        FROM outbox_events
        WHERE project_id = $1
        GROUP BY event_type
        "#,
    )
    .bind(project_id)
    .fetch_all(&pool)
    .await
    .expect("count typed transactional outbox events")
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    assert_eq!(outbox_counts.get("conversation.created"), Some(&4));
    assert_eq!(outbox_counts.get("conversation.message.created"), Some(&5));
    assert_eq!(outbox_counts.get("conversation.archived"), Some(&2));
    assert!(
        outbox_counts
            .get("conversation.read_state.updated")
            .is_some_and(|count| *count >= 2),
        "each independently advanced read cursor emits a durable invalidation"
    );

    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated M2 HTTP schema");
}

async fn login(app: &axum::Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/auth/login",
            &[("origin", "http://127.0.0.1:3100")],
            json!({
                "loginName": "m2-owner",
                "password": "correct horse battery staple"
            }),
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
    let cookie = cookies.join("; ");
    let _ = response_json(response).await;
    (cookie, csrf)
}

fn entry_sequences(response: &Value) -> Vec<i64> {
    response["entries"]
        .as_array()
        .expect("timeline entries")
        .iter()
        .map(|entry| entry["sequence"].as_i64().expect("entry sequence"))
        .collect()
}

fn uuid_field(value: &Value, field: &str) -> Uuid {
    Uuid::parse_str(value[field].as_str().expect("UUID field")).expect("valid UUID")
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

fn write_headers<'a>(
    cookie: &'a str,
    csrf: &'a str,
    idempotency_key: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut headers = vec![
        ("origin", "http://127.0.0.1:3100"),
        ("cookie", cookie),
        ("x-csrf-token", csrf),
    ];
    if let Some(key) = idempotency_key {
        headers.push(("idempotency-key", key));
    }
    headers
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
        Body::from(serde_json::to_vec(&body).expect("serialize request body"))
    };
    builder.body(body).expect("build request")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("read response body"),
    )
    .expect("parse response JSON")
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(8)
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
        .expect("connect schema-scoped M2 HTTP pool")
}

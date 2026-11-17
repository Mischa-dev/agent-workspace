//! A2: what agents are told. Every read and push goes back through the log
//! and checks access per event when it is read, not when it was written; a
//! DM reaches only its participants whatever a connection subscribes to;
//! every event says why the reader hears it and carries ids, never text;
//! pushes are signed the Standard Webhooks way.

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

/// Text no event may carry.
const SECRET_WORD: &str = "pangolin";

fn mention(text: &str, actor: Uuid) -> Value {
    json!({ "version": 1, "nodes": [
        { "type": "text", "text": text },
        { "type": "mention", "actorId": actor }
    ]})
}

async fn post(world: &Workspace, conversation: &str, key: &str, content: Value) {
    let (status, sent) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(key),
            json!({ "content": content }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
}

async fn channel(world: &Workspace, name: &str) -> String {
    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{}/channels", world.project("WORK")),
            Some(&format!("channel-{name}")),
            json!({ "name": name }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    made["conversation"]["id"]
        .as_str()
        .or_else(|| made["id"].as_str())
        .expect("channel")
        .to_owned()
}

/// The agent's events after `cursor` (or where it last read).
async fn read(world: &Workspace, agent: &impl Caller, cursor: Option<&Value>) -> Value {
    agent_workspace_server::events::catch_up(world.pool())
        .await
        .expect("sequence events");
    let arguments = match cursor {
        Some(cursor) => json!({ "cursor": cursor }),
        None => json!({}),
    };
    let (_, page) = agent
        .call(
            &world.app,
            Call::Tool {
                name: "event_list",
                arguments,
            },
        )
        .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    page["value"].clone()
}

fn of_type<'a>(page: &'a Value, kind: &str) -> Vec<&'a Value> {
    page["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["type"] == kind)
        .collect()
}

#[tokio::test]
async fn events_follow_access_when_read_and_dms_stay_private() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .agent("mara", "member")
        .build("delivery_access")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let mara = &world.agents["mara"];
    let start_rook = read(&world, rook, None).await["nextCursor"].clone();
    let start_mara = read(&world, mara, None).await["nextCursor"].clone();

    // Rook asks to hear everything it can see, DMs included.
    let (_, connections) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "connection_list",
                arguments: json!({}),
            },
        )
        .await;
    let rook_connection = connections["value"]["connections"][0]["id"].clone();
    let (_, widened) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "connection_update",
                arguments: json!({
                    "connectionId": rook_connection,
                    "subscriptions": [{ "kinds": ["*"], "minReason": "visible", "includesDms": true }],
                }),
            },
        )
        .await;
    assert_eq!(widened["outcome"], "ok", "{widened}");

    // The owner mentions Mara in a DM with her.
    let (status, dm) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({ "actorId": mara.actor }),
        ),
    )
    .await;
    assert!(status.is_success(), "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("DM").to_owned();
    post(
        &world,
        &dm,
        "dm-1",
        mention(&format!("{SECRET_WORD} for "), mara.actor),
    )
    .await;

    // Mara hears it, addressed, with ids only.
    let heard = read(&world, mara, Some(&start_mara)).await;
    let mentioned = of_type(&heard, "message.mentioned");
    assert_eq!(mentioned.len(), 1, "{heard}");
    assert_eq!(mentioned[0]["reason"], "mentioned");
    assert_eq!(mentioned[0]["data"]["conversationId"], dm.as_str());
    assert_eq!(of_type(&heard, "reply.requested").len(), 1, "{heard}");
    assert!(
        !heard.to_string().contains(SECRET_WORD),
        "ids only: {heard}"
    );

    // Rook, subscribed to everything, hears nothing of a DM it isn't in.
    let rook_heard = read(&world, rook, Some(&start_rook)).await;
    assert!(!rook_heard.to_string().contains(&dm), "{rook_heard}");

    // In a channel Rook can read it hears the message as visible...
    let general = channel(&world, "general").await;
    let before = rook_heard["nextCursor"].clone();
    post(&world, &general, "ch-1", mention("hello ", mara.actor)).await;
    let rook_heard = read(&world, rook, Some(&before)).await;
    let created = of_type(&rook_heard, "message.created");
    assert_eq!(created.len(), 1, "{rook_heard}");
    assert_eq!(created[0]["reason"], "visible");

    // ...and once it leaves the project, what was written before it left
    // is no longer delivered: access is checked as the event is read.
    let before = rook_heard["nextCursor"].clone();
    post(&world, &general, "ch-2", mention("again ", rook.actor)).await;
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(world.project("WORK"))
    .bind(rook.actor)
    .execute(world.pool())
    .await
    .expect("rook leaves");
    let rook_heard = read(&world, rook, Some(&before)).await;
    assert_eq!(rook_heard["events"], json!([]), "{rook_heard}");

    // A cursor is the connection's own.
    let (_, refused) = mara
        .call(
            &world.app,
            Call::Tool {
                name: "event_list",
                arguments: json!({ "cursor": before }),
            },
        )
        .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    world.finish().await;
}

#[tokio::test]
async fn a_read_without_a_cursor_recovers_once_its_place_is_gone() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("delivery_expired")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let general = channel(&world, "general").await;
    // Rook reads to here, so its connection keeps this place.
    let start = read(&world, rook, None).await["nextCursor"].clone();
    read(&world, rook, Some(&start)).await;
    // What came after goes with retention before Rook reads it.
    post(&world, &general, "old", mention("old ", rook.actor)).await;
    agent_workspace_server::events::catch_up(world.pool())
        .await
        .expect("sequence events");
    // (as retention does, for every event so far)
    let gone = sqlx::query("DELETE FROM events")
        .execute(world.pool())
        .await
        .expect("retention")
        .rows_affected();
    assert!(gone > 0);
    post(&world, &general, "new", mention("new ", rook.actor)).await;

    // The old cursor is refused, with the way back...
    agent_workspace_server::events::catch_up(world.pool())
        .await
        .expect("sequence events");
    let (_, refused) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "event_list",
                arguments: json!({ "cursor": start }),
            },
        )
        .await;
    assert_eq!(refused["code"], "cursor_expired", "{refused}");
    // ...which works: a read without one starts from the oldest kept.
    let heard = read(&world, rook, None).await;
    assert_eq!(of_type(&heard, "message.mentioned").len(), 1, "{heard}");
    let again = read(&world, rook, None).await;
    assert_eq!(of_type(&again, "message.mentioned").len(), 1, "{again}");
    world.finish().await;
}

#[tokio::test]
async fn a_narrowed_role_hears_no_task_and_a_secret_is_shown_once() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .agent("mara", "member")
        .connected_app("laptop")
        .custom_role(
            "No tasks",
            &["projects.view", "channels.post", "directory.view"],
        )
        .build("delivery_narrowed")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let mara = &world.agents["mara"];
    // Mara holds a role without tasks.view in WORK.
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2::uuid AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["No tasks"]).expect("role id"))
    .bind(world.project("WORK"))
    .bind(mara.actor)
    .execute(world.pool())
    .await
    .expect("narrow Mara");
    // Both ask to hear everything they can see.
    let mut starts = Vec::new();
    for agent in [rook, mara] {
        let start = read(&world, agent, None).await["nextCursor"].clone();
        let (_, listed) = agent
            .call(
                &world.app,
                Call::Tool {
                    name: "connection_list",
                    arguments: json!({}),
                },
            )
            .await;
        let (_, widened) = agent
            .call(
                &world.app,
                Call::Tool {
                    name: "connection_update",
                    arguments: json!({
                        "connectionId": listed["value"]["connections"][0]["id"],
                        "subscriptions": [{ "kinds": ["*"], "minReason": "visible" }],
                    }),
                },
            )
            .await;
        assert_eq!(widened["outcome"], "ok", "{widened}");
        starts.push(start);
    }
    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/task_create", world.organization),
            Some(&Uuid::now_v7().to_string()),
            json!({ "projectId": world.project("WORK"), "title": "Ship the release" }),
        ),
    )
    .await;
    assert!(status.is_success(), "{made}");
    let rook_heard = read(&world, rook, Some(&starts[0])).await;
    assert!(
        !of_type(&rook_heard, "task.created").is_empty(),
        "{rook_heard}"
    );
    let mara_heard = read(&world, mara, Some(&starts[1])).await;
    assert!(
        of_type(&mara_heard, "task.created").is_empty(),
        "a role without tasks.view hears no task: {mara_heard}"
    );

    // An app acting for the owner hears only what its delegation limit lets
    // it read: with tasks kept from it, no task event reaches it.
    let laptop = &world.apps["laptop"];
    let (_, listed) = laptop
        .call(
            &world.app,
            Call::Tool {
                name: "connection_list",
                arguments: json!({}),
            },
        )
        .await;
    let (_, widened) = laptop
        .call(
            &world.app,
            Call::Tool {
                name: "connection_update",
                arguments: json!({
                    "connectionId": listed["value"]["connections"][0]["id"],
                    "subscriptions": [{ "kinds": ["*"], "minReason": "visible" }],
                }),
            },
        )
        .await;
    assert_eq!(widened["outcome"], "ok", "{widened}");
    let laptop_start = read(&world, laptop, None).await["nextCursor"].clone();
    let limit = |limit: Option<Vec<&'static str>>| {
        sqlx::query(
            "UPDATE organization_memberships SET agent_limit = $1 WHERE organization_id = $2 AND actor_id = $3",
        )
        .bind(limit)
        .bind(world.organization)
        .bind(laptop.person)
        .execute(world.pool())
    };
    limit(Some(vec!["projects.view", "channels.post"]))
        .await
        .expect("keep tasks from the owner's apps");
    let (_, rook_made) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "task_create",
                arguments: json!({ "projectId": "WORK", "title": "Rook's task",
                                   "clientRequestId": "rook-task" }),
            },
        )
        .await;
    assert_eq!(rook_made["outcome"], "ok", "{rook_made}");
    let limited = read(&world, laptop, Some(&laptop_start)).await;
    assert!(
        of_type(&limited, "task.created").is_empty(),
        "an app kept from tasks hears no task: {limited}"
    );
    limit(None).await.expect("lift the limit");
    let lifted = read(&world, laptop, Some(&laptop_start)).await;
    assert!(
        !of_type(&lifted, "task.created").is_empty(),
        "without the limit the same read hears it: {lifted}"
    );

    // A webhook's secret is shown once: a retry of the same call answers
    // without it, and no copy is kept for the retry.
    let create = || {
        rook.call(
            &world.app,
            Call::Tool {
                name: "connection_create",
                arguments: json!({
                    "name": "Runner",
                    "webhookUrl": "https://hooks.example.test/once",
                    "clientRequestId": "shown-once",
                }),
            },
        )
    };
    let (_, first) = create().await;
    assert_eq!(first["outcome"], "ok", "{first}");
    let secret = first["value"]["webhookSecret"]
        .as_str()
        .expect("secret shown")
        .to_owned();
    let (_, again) = create().await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(
        again["value"]["connection"]["id"],
        first["value"]["connection"]["id"]
    );
    assert!(again["value"]["webhookSecret"].is_null(), "{again}");
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM idempotency_keys WHERE strpos(response_body::text, $1) > 0",
    )
    .bind(&secret)
    .fetch_one(world.pool())
    .await
    .expect("idempotency keys");
    assert_eq!(kept, 0, "the secret is kept for a replay");
    world.finish().await;
}

#[tokio::test]
async fn pushes_are_signed_the_standard_way_and_carry_no_text() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("delivery_push")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let received = Arc::new(Mutex::new(Vec::<(axum::http::HeaderMap, String)>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let sink = Arc::clone(&received);
    tokio::spawn(async move {
        let receiver = axum::Router::new().route(
            "/hook",
            axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
                let sink = Arc::clone(&sink);
                async move {
                    sink.lock().expect("lock").push((headers, body));
                    StatusCode::NO_CONTENT
                }
            }),
        );
        axum::serve(listener, receiver).await.expect("serve");
    });

    // A connection of Rook's own, pushing in the standard format.
    let (_, made) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "connection_create",
                arguments: json!({
                    "name": "Runner",
                    "webhookUrl": "https://hooks.example.test/rook",
                    "webhookFormat": "standard",
                    "clientRequestId": "rook-runner",
                }),
            },
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let secret = made["value"]["webhookSecret"]
        .as_str()
        .expect("secret shown once")
        .to_owned();
    let connection = made["value"]["connection"]["id"]
        .as_str()
        .expect("connection")
        .parse::<Uuid>()
        .expect("UUID");
    // Pointed at the local receiver, which only a trusted origin reaches.
    sqlx::query("UPDATE agent_connections SET webhook_url = $2 WHERE id = $1")
        .bind(connection)
        .bind(format!("{origin}/hook"))
        .execute(world.pool())
        .await
        .expect("local hook");
    let general = channel(&world, "general").await;
    post(
        &world,
        &general,
        "push-1",
        mention(&format!("{SECRET_WORD} for "), rook.actor),
    )
    .await;

    let mut config = (*world.state.config).clone();
    config.webhook_trusted_origins = vec![origin.clone()];
    let state = agent_workspace_server::AppState {
        config: Arc::new(config),
        ..world.state.clone()
    };
    agent_workspace_server::events::catch_up(world.pool())
        .await
        .expect("sequence events");
    agent_workspace_server::tenancy::system(
        "events.log",
        agent_workspace_server::delivery::push::push(&state, connection),
    )
    .await
    .expect("push");

    let posts = received.lock().expect("lock").clone();
    let mentioned = posts
        .iter()
        .find(|(_, body)| body.contains("\"message.mentioned\""))
        .unwrap_or_else(|| panic!("no mention pushed: {posts:?}"));
    let header = |name: &str| {
        mentioned
            .0
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let id = header("webhook-id");
    let timestamp = header("webhook-timestamp");
    let expected =
        agent_workspace_server::delivery::sign::standard(&secret, &id, &timestamp, &mentioned.1)
            .expect("sign");
    assert!(
        header("webhook-signature")
            .split(' ')
            .any(|signature| signature == expected),
        "signed with the connection's secret"
    );
    let body: Value = serde_json::from_str(&mentioned.1).expect("JSON");
    assert_eq!(body["reason"], "mentioned", "{body}");
    assert_eq!(body["recipient"]["accountId"], rook.actor.to_string());
    assert!(
        !mentioned.1.contains(SECRET_WORD),
        "ids only: {}",
        mentioned.1
    );
    let delivered = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM deliveries WHERE connection_id = $1 AND state = 'delivered'",
    )
    .bind(connection)
    .fetch_one(world.pool())
    .await
    .expect("deliveries");
    assert!(delivered >= 1);
    world.finish().await;
}

/// A2.6: a person's webhook is a connection with an address. One their app
/// set up is pushed to while the app is connected and stops, saying why,
/// once it is disconnected; a test ping's answer is kept on it.
#[tokio::test]
async fn a_webhook_an_app_set_up_stops_when_the_app_is_disconnected() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("laptop")
        .build("delivery_app_webhook")
        .await
    else {
        return;
    };
    let laptop = &world.apps["laptop"];
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let sink = Arc::clone(&received);
    tokio::spawn(async move {
        let receiver = axum::Router::new().route(
            "/hook",
            axum::routing::post(move |body: String| {
                let sink = Arc::clone(&sink);
                async move {
                    sink.lock().expect("lock").push(body);
                    StatusCode::NO_CONTENT
                }
            }),
        );
        axum::serve(listener, receiver).await.expect("serve");
    });
    let mut config = (*world.state.config).clone();
    config.webhook_trusted_origins = vec![origin.clone()];
    let state = agent_workspace_server::AppState {
        config: Arc::new(config),
        ..world.state.clone()
    };

    let (_, made) = laptop
        .call(
            &world.app,
            Call::Tool {
                name: "connection_create",
                arguments: json!({
                    "name": "Laptop hook",
                    "webhookUrl": "https://hooks.example.test/laptop",
                    "reachesAll": false,
                    "subscriptions": [{"kinds": ["task.created"], "echoes": true}],
                    "clientRequestId": "laptop-hook",
                }),
            },
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert_eq!(
        made["value"]["connection"]["madeThroughApp"], true,
        "{made}"
    );
    let connection = made["value"]["connection"]["id"]
        .as_str()
        .expect("connection")
        .parse::<Uuid>()
        .expect("UUID");
    sqlx::query("UPDATE agent_connections SET webhook_url = $2 WHERE id = $1")
        .bind(connection)
        .bind(format!("{origin}/hook"))
        .execute(world.pool())
        .await
        .expect("local hook");
    let new_task = |title: &'static str| {
        let app = world.app.clone();
        let project = world.project("WORK").to_owned();
        async move {
            let (_, made) = laptop
                .call(
                    &app,
                    Call::Tool {
                        name: "task_create",
                        arguments: json!({"projectId": project, "title": title,
                                          "clientRequestId": title.replace(' ', "-")}),
                    },
                )
                .await;
            assert!(
                matches!(made["outcome"].as_str(), Some("ok" | "applied")),
                "{made}"
            );
        }
    };
    let push = || async {
        agent_workspace_server::events::catch_up(world.pool())
            .await
            .expect("sequence events");
        agent_workspace_server::tenancy::system(
            "events.log",
            agent_workspace_server::delivery::push::push(&state, connection),
        )
        .await
        .expect("push");
    };

    // Connected: the new task is pushed.
    new_task("Pushed while connected").await;
    push().await;
    let posts = received.lock().expect("lock").clone();
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert!(posts[0].contains("\"task.created\""), "{posts:?}");
    assert!(!posts[0].contains("Pushed while connected"), "ids only");

    // A test ping goes out on the job queue; its answer is kept.
    let (_, queued) = laptop
        .call(
            &world.app,
            Call::Tool {
                name: "connection_test",
                arguments: json!({"connectionId": connection}),
            },
        )
        .await;
    assert_eq!(queued["outcome"], "ok", "{queued}");
    let registry = agent_workspace_server::jobs::default_registry();
    let options = agent_workspace_server::jobs::WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "ping-test".to_owned(),
    };
    agent_workspace_server::jobs::run_due(&state, &registry, &options)
        .await
        .expect("run jobs");
    let (_, listed) = laptop
        .call(
            &world.app,
            Call::Tool {
                name: "connection_list",
                arguments: json!({}),
            },
        )
        .await;
    let hook = listed["value"]["connections"]
        .as_array()
        .expect("connections")
        .iter()
        .find(|item| item["id"] == connection.to_string())
        .expect("the hook")
        .clone();
    assert_eq!(hook["lastTest"]["ok"], true, "{hook}");
    assert_eq!(hook["lastTest"]["status"], 204, "{hook}");
    let before = received.lock().expect("lock").len();

    // Disconnected: nothing more is pushed, and the webhook says why.
    sqlx::query(
        "UPDATE mcp_oauth_grants SET revoked_at = now() \
         WHERE id = (SELECT via_grant_id FROM agent_connections WHERE id = $1)",
    )
    .bind(connection)
    .execute(world.pool())
    .await
    .expect("disconnect the app");
    let owner_task = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{}/work-items", world.project("WORK")),
            Some("after-disconnect"),
            json!({"title": "After the app left"}),
        ),
    )
    .await;
    assert!(owner_task.0.is_success(), "{owner_task:?}");
    push().await;
    assert_eq!(received.lock().expect("lock").len(), before);
    let (state_now, reason) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, disabled_reason FROM agent_connections WHERE id = $1",
    )
    .bind(connection)
    .fetch_one(world.pool())
    .await
    .expect("connection");
    assert_eq!(state_now, "push_disabled");
    assert!(
        reason.unwrap_or_default().contains("disconnected"),
        "it says why"
    );
    world.finish().await;
}

/// A2.11: a follow-up an agent sets for itself comes due once per time, as
/// an addressed `schedule.due` carrying ids; its note stays in its row.
#[tokio::test]
async fn follow_ups_come_due_once_and_carry_no_text() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("delivery_schedules")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let start = read(&world, rook, None).await["nextCursor"].clone();
    let (_, made) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "schedule_create",
                arguments: json!({
                    "note": format!("check the {SECRET_WORD}"),
                    "inMinutes": 30,
                    "everyMinutes": 60,
                    "clientRequestId": "rook-check",
                }),
            },
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let schedule = made["value"]["id"].as_str().expect("id").to_owned();

    // Its time comes: the queued job runs (twice, as two workers might).
    sqlx::query(
        "UPDATE background_jobs SET run_at = now() - interval '1 second' \
         WHERE kind = 'schedule.due' AND state = 'queued'",
    )
    .execute(world.pool())
    .await
    .expect("due now");
    let registry = agent_workspace_server::jobs::default_registry();
    let options = agent_workspace_server::jobs::WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "schedules-test".to_owned(),
    };
    for _ in 0..2 {
        agent_workspace_server::jobs::run_due(&world.state, &registry, &options)
            .await
            .expect("run jobs");
    }
    let heard = read(&world, rook, Some(&start)).await;
    let due = of_type(&heard, "schedule.due");
    assert_eq!(due.len(), 1, "{heard}");
    assert_eq!(due[0]["data"]["scheduleId"], schedule.as_str(), "{heard}");
    assert!(
        !heard.to_string().contains(SECRET_WORD),
        "ids only: {heard}"
    );

    // It repeats an hour after the time it was due, and can be cancelled.
    let (_, listed) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "schedule_list",
                arguments: json!({}),
            },
        )
        .await;
    assert_eq!(listed["value"]["schedules"][0]["timesDue"], 1, "{listed}");
    assert_eq!(listed["value"]["schedules"][0]["state"], "active");
    let (_, cancelled) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "schedule_update",
                arguments: json!({ "scheduleId": schedule, "cancel": true }),
            },
        )
        .await;
    assert_eq!(cancelled["outcome"], "ok", "{cancelled}");
    assert_eq!(cancelled["value"]["state"], "cancelled");
    world.finish().await;
}

/// The app on a local port, for clients that speak HTTP.
async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    base
}

/// A2.7: the contract suite, reading as an agent over HTTP, passes against
/// the server: catalogue, poll, long-poll woken by a mention, the stream
/// and resuming it, every envelope shaped like the fixture.
#[tokio::test]
async fn the_delivery_contract_holds_over_http() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("delivery_contract")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let general = channel(&world, "general").await;
    let base = serve(world.app.clone()).await;
    let report = agent_workspace_delivery_contract::check_server(
        &base,
        &rook.token,
        Some(async || {
            post(
                &world,
                &general,
                "contract-1",
                mention(&format!("{SECRET_WORD} for "), rook.actor),
            )
            .await;
            agent_workspace_server::events::catch_up(world.pool())
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }),
    )
    .await;
    assert!(report.passed(), "{report}");
    assert!(
        report
            .checks
            .iter()
            .all(|check| check.outcome == agent_workspace_delivery_contract::Outcome::Pass),
        "{report}"
    );
    world.finish().await;
}

/// A2.7: what the server pushes in each format passes the receiver's
/// checks: signatures, headers and a body shaped like the fixture.
#[tokio::test]
async fn pushes_in_every_format_meet_the_contract() {
    use agent_workspace_delivery_contract::webhook::{Format, Received, check_webhook};

    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("delivery_contract_push")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let received = Arc::new(Mutex::new(Vec::<(String, Received)>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let sink = Arc::clone(&received);
    tokio::spawn(async move {
        let receiver = axum::Router::new().route(
            "/hook/{format}",
            axum::routing::post(
                move |axum::extract::Path(format): axum::extract::Path<String>,
                      headers: axum::http::HeaderMap,
                      body: String| {
                    let sink = Arc::clone(&sink);
                    async move {
                        let headers = headers
                            .iter()
                            .map(|(name, value)| {
                                (
                                    name.as_str().to_owned(),
                                    value.to_str().unwrap_or_default().to_owned(),
                                )
                            })
                            .collect();
                        sink.lock()
                            .expect("lock")
                            .push((format, Received { headers, body }));
                        StatusCode::NO_CONTENT
                    }
                },
            ),
        );
        axum::serve(listener, receiver).await.expect("serve");
    });

    let mut connections = Vec::new();
    for format in ["standard", "typed_v1", "inbox_v1"] {
        let (_, made) = rook
            .call(
                &world.app,
                Call::Tool {
                    name: "connection_create",
                    arguments: json!({
                        "name": format!("Receiver {format}"),
                        "webhookUrl": format!("https://hooks.example.test/{format}"),
                        "webhookFormat": format,
                        "clientRequestId": format!("contract-{format}"),
                    }),
                },
            )
            .await;
        assert_eq!(made["outcome"], "ok", "{made}");
        let secret = made["value"]["webhookSecret"]
            .as_str()
            .expect("secret shown once")
            .to_owned();
        let connection = made["value"]["connection"]["id"]
            .as_str()
            .expect("connection")
            .parse::<Uuid>()
            .expect("UUID");
        sqlx::query("UPDATE agent_connections SET webhook_url = $2 WHERE id = $1")
            .bind(connection)
            .bind(format!("{origin}/hook/{format}"))
            .execute(world.pool())
            .await
            .expect("local hook");
        connections.push((format, connection, secret));
    }
    // A mention asks Rook to reply, so every format has something to carry.
    let general = channel(&world, "general").await;
    post(
        &world,
        &general,
        "contract-push",
        mention(&format!("{SECRET_WORD} for "), rook.actor),
    )
    .await;
    let mut config = (*world.state.config).clone();
    config.webhook_trusted_origins = vec![origin.clone()];
    let state = agent_workspace_server::AppState {
        config: Arc::new(config),
        ..world.state.clone()
    };
    agent_workspace_server::events::catch_up(world.pool())
        .await
        .expect("sequence events");
    for (_, connection, _) in &connections {
        agent_workspace_server::tenancy::system(
            "events.log",
            agent_workspace_server::delivery::push::push(&state, *connection),
        )
        .await
        .expect("push");
    }

    let posts = received.lock().expect("lock").clone();
    for (format, _, secret) in &connections {
        let mine = posts
            .iter()
            .filter(|(to, _)| to == format)
            .map(|(_, push)| push)
            .collect::<Vec<_>>();
        assert!(!mine.is_empty(), "nothing pushed to {format}: {posts:?}");
        for push in mine {
            let report = check_webhook(Format::parse(format).expect("format"), secret, push, None);
            assert!(report.passed(), "{format}: {}\n{report}", push.body);
            assert!(!push.body.contains(SECRET_WORD), "ids only: {}", push.body);
        }
    }
    world.finish().await;
}

//! A2.14: budgets. One budget refuses a person and an agent alike; the
//! default message rate holds two agents answering each other; the count
//! is shared by every server; a hard budget on deliveries holds pushes
//! until the next period.

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Agent, Call, Caller},
    world::{Workspace, WorldBuilder},
};

async fn op(world: &Workspace, name: &str, input: Value) -> Value {
    let (status, body) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{name}: {body}");
    body["value"].clone()
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

/// What a person's send answers: its status and code.
async fn person_says(
    world: &Workspace,
    who: &super::support::app::Browser,
    conversation: &str,
    text: &str,
) -> (StatusCode, Value) {
    send(
        &world.app,
        who.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": text}]}}),
        ),
    )
    .await
}

async fn agent_says(app: &axum::Router, agent: &Agent, conversation: &str, text: &str) -> Value {
    let (_, body) = agent
        .call(
            app,
            Call::Tool {
                name: "message_send",
                arguments: json!({"conversationId": conversation, "text": text}),
            },
        )
        .await;
    body
}

#[tokio::test]
async fn one_budget_refuses_a_person_and_an_agent_alike() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("budgets_alike")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let lena = &world.people["lena"];
    let general = channel(&world, "general").await;
    let set = op(
        &world,
        "budget_set",
        json!({"scope": "workspace", "eachAccount": true, "meter": "messages",
               "period": "hour", "hardLimit": 2}),
    )
    .await;
    assert_eq!(set["budget"]["hardLimit"], 2, "{set}");

    for text in ["one", "two"] {
        let (status, body) = person_says(&world, &lena.browser, &general, text).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let sent = agent_says(&world.app, rook, &general, text).await;
        assert_eq!(sent["outcome"], "ok", "{sent}");
    }
    let (status, refused) = person_says(&world, &lena.browser, &general, "three").await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{refused}");
    assert_eq!(refused["error"]["code"], "limit_reached", "{refused}");
    let refused = agent_says(&world.app, rook, &general, "three").await;
    assert_eq!(refused["outcome"], "limit_reached", "{refused}");
    // Each account on its own: the owner still speaks.
    let (status, body) = person_says(&world, &world.owner.browser, &general, "hello").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // The list counts the account asked about; only settings managers
    // read it or change it.
    let listed = op(&world, "budget_list", json!({"account": rook.actor})).await;
    let hourly = listed["budgets"]
        .as_array()
        .expect("budgets")
        .iter()
        .find(|budget| budget["period"] == "hour")
        .unwrap_or_else(|| panic!("the hourly budget in {listed}"));
    assert_eq!(hourly["used"], 2, "{listed}");
    let (_, not_rooks) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "budget_set",
                arguments: json!({"scope": "account", "scopeId": rook.actor,
                                  "meter": "messages", "period": "hour", "hardLimit": 100}),
            },
        )
        .await;
    assert_ne!(not_rooks["outcome"], "ok", "{not_rooks}");

    // Removed, it refuses nothing.
    let removed = op(
        &world,
        "budget_set",
        json!({"scope": "workspace", "eachAccount": true, "meter": "messages",
               "period": "hour"}),
    )
    .await;
    assert!(removed["budget"].is_null(), "{removed}");
    let sent = agent_says(&world.app, rook, &general, "four").await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    world.finish().await;
}

#[tokio::test]
async fn the_default_message_rate_holds_a_loop() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("budgets_loop")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let general = channel(&world, "general").await;
    let rate = agent_workspace_server::metering::DEFAULT_MESSAGE_RATE;
    for index in 0..rate {
        let sent = agent_says(&world.app, rook, &general, &format!("ping {index}")).await;
        assert_eq!(sent["outcome"], "ok", "message {index}: {sent}");
    }
    let held = agent_says(&world.app, rook, &general, "one too many").await;
    assert_eq!(held["outcome"], "limit_reached", "{held}");
    assert!(held.to_string().contains("last minute"), "{held}");

    let listed = op(&world, "budget_list", json!({"account": rook.actor})).await;
    let default = &listed["budgets"][0];
    assert_eq!(default["isDefault"], true, "{listed}");
    assert_eq!(default["used"], rate, "{listed}");
    // A higher rate replaces the default.
    op(
        &world,
        "budget_set",
        json!({"scope": "workspace", "eachAccount": true, "meter": "messages",
               "period": "minute", "hardLimit": rate + 10}),
    )
    .await;
    let sent = agent_says(&world.app, rook, &general, "pong").await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let listed = op(&world, "budget_list", json!({})).await;
    assert!(
        listed["budgets"]
            .as_array()
            .expect("budgets")
            .iter()
            .all(|budget| budget["isDefault"] == false),
        "{listed}"
    );
    world.finish().await;
}

#[tokio::test]
async fn the_count_is_shared_by_every_server() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("budgets_shared")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let general = channel(&world, "general").await;
    op(
        &world,
        "budget_set",
        json!({"scope": "account", "scopeId": rook.actor, "meter": "messages",
               "period": "day", "hardLimit": 2}),
    )
    .await;
    let (other, _) = world.rebuilt_with(
        "AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS",
        "http://127.0.0.1:9",
    );
    let first = agent_says(&world.app, rook, &general, "here").await;
    assert_eq!(first["outcome"], "ok", "{first}");
    let second = agent_says(&other, rook, &general, "there").await;
    assert_eq!(second["outcome"], "ok", "{second}");
    let third = agent_says(&world.app, rook, &general, "again").await;
    assert_eq!(third["outcome"], "limit_reached", "{third}");
    let fourth = agent_says(&other, rook, &general, "again").await;
    assert_eq!(fourth["outcome"], "limit_reached", "{fourth}");
    world.finish().await;
}

/// A soft limit tells whoever set it, once.
#[tokio::test]
async fn nearing_a_budget_tells_whoever_set_it_once() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("budgets_warning")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let general = channel(&world, "general").await;
    op(
        &world,
        "budget_set",
        json!({"scope": "account", "scopeId": rook.actor, "meter": "messages",
               "period": "day", "softLimit": 1, "hardLimit": 10}),
    )
    .await;
    for text in ["one", "two", "three"] {
        let sent = agent_says(&world.app, rook, &general, text).await;
        assert_eq!(sent["outcome"], "ok", "{sent}");
    }
    let warnings = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = 'budget_warning'",
    )
    .bind(world.owner.actor)
    .fetch_one(world.pool())
    .await
    .expect("warnings");
    assert_eq!(warnings, 1);
    world.finish().await;
}

#[tokio::test]
async fn a_deliveries_budget_holds_pushes_until_the_next_period() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("budgets_deliveries")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
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
    let (_, made) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "connection_create",
                arguments: json!({
                    "name": "Runner",
                    "webhookUrl": "https://hooks.example.test/rook",
                    "clientRequestId": "rook-runner",
                }),
            },
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
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
    op(
        &world,
        "budget_set",
        json!({"scope": "account", "scopeId": rook.actor, "meter": "deliveries",
               "period": "day", "hardLimit": 1}),
    )
    .await;
    let general = channel(&world, "general").await;
    for text in ["first", "second", "third"] {
        let (status, body) = send(
            &world.app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/conversations/{general}/messages"),
                Some(&Uuid::now_v7().to_string()),
                json!({"content": {"version": 1, "nodes": [
                    {"type": "text", "text": format!("{text} for ")},
                    {"type": "mention", "actorId": rook.actor}
                ]}}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let mut config = (*world.state.config).clone();
    config.webhook_trusted_origins = vec![origin.clone()];
    let state = agent_workspace_server::AppState {
        config: Arc::new(config),
        ..world.state.clone()
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
    push().await;
    assert_eq!(received.lock().expect("lock").len(), 1, "one push a day");
    // The rest wait for the next day (UTC), still to be sent.
    let (held, waiting) = sqlx::query_as::<_, (i64, bool)>(
        "SELECT count(*), bool_and(next_attempt_at >= date_trunc('day', now() AT TIME ZONE 'UTC') \
                AT TIME ZONE 'UTC' + interval '1 day') \
         FROM deliveries WHERE connection_id = $1 AND state = 'pending'",
    )
    .bind(connection)
    .fetch_one(world.pool())
    .await
    .expect("held");
    assert!(held >= 1, "pushes past the budget are held");
    assert!(waiting, "held until the next period");
    push().await;
    assert_eq!(received.lock().expect("lock").len(), 1, "still held today");

    // The next day: one more goes.
    sqlx::query(
        "UPDATE usage_counters SET period_start = period_start - 1 \
         WHERE meter = 'deliveries' AND period = 'day'",
    )
    .execute(world.pool())
    .await
    .expect("a day passes");
    sqlx::query("UPDATE deliveries SET next_attempt_at = now() WHERE connection_id = $1")
        .bind(connection)
        .execute(world.pool())
        .await
        .expect("due again");
    push().await;
    assert_eq!(
        received.lock().expect("lock").len(),
        2,
        "released next period"
    );
    world.finish().await;
}

/// No one budgets a DM they aren't in, or sees a budget on it: an admin of
/// the workspace included.
#[tokio::test]
async fn a_budget_on_a_dm_stays_with_its_members() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .dm("lena", "mara")
        .build("budgets_dm")
        .await
    else {
        return;
    };
    let dm = world.conversations["lena+mara"].clone();
    let (_, refused) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/budget_set", world.organization),
            Some(&Uuid::now_v7().to_string()),
            json!({"scope": "conversation", "scopeId": dm, "meter": "messages",
                   "period": "day", "hardLimit": 5}),
        ),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    sqlx::query(
        "INSERT INTO spend_limits (id, workspace_id, scope_kind, scope_id, meter, period, \
         hard_limit, set_by_actor_id) VALUES ($1, $2, 'conversation', $3::uuid, 'messages', \
         'day', 5, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(world.organization)
    .bind(&dm)
    .bind(world.people["lena"].actor)
    .execute(world.pool())
    .await
    .expect("a budget on the DM");
    let listed = op(&world, "budget_list", json!({})).await;
    assert!(!listed.to_string().contains(&dm), "{listed}");
    world.finish().await;
}

/// A2.14 on the built-in host: a new built-in run is refused once its
/// agent's hard budget on what runs use is spent, as a run on any other
/// host is.
#[tokio::test]
async fn a_spent_run_budget_refuses_a_built_in_run_too() {
    let Some(world) = crate::support::two_workspaces::two_workspaces("budgets_builtin").await
    else {
        return;
    };
    let (worker, account, workspace, space, conversation) =
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, Uuid)>(
            "SELECT run.execution_worker_id, worker.actor_id, project.organization_id, \
                    project.id, run.conversation_id \
             FROM agent_runs run \
             JOIN execution_workers worker ON worker.id = run.execution_worker_id \
             JOIN projects project ON project.id = worker.project_id \
             WHERE run.id = $1",
        )
        .bind(world.a.run)
        .fetch_one(&world.pool)
        .await
        .expect("the built-in run's agent");
    sqlx::query(
        "INSERT INTO spend_limits (id, workspace_id, scope_kind, scope_id, per_account, meter, \
                                   period, hard_limit, set_by_actor_id) \
         VALUES ($1, $2, 'account', $3, false, 'reported_tokens', 'day', 0, $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(account)
    .execute(&world.pool)
    .await
    .expect("a spent budget");
    // The built-in host opens runs of every workspace, as its scheduler does.
    let refused = agent_workspace_server::tenancy::system("runs.host", async {
        let mut transaction = agent_workspace_server::db::scoped_tx(&world.pool)
            .await
            .expect("transaction");
        agent_workspace_server::runs::builtin::open(
            &mut transaction,
            &agent_workspace_server::runs::builtin::NewBuiltinRun {
                id: Uuid::now_v7(),
                project_id: space,
                execution_worker_id: worker,
                prompt: "Another go",
                from_request: false,
                conversation_id: conversation,
                requested_by: account,
                sponsored_by: None,
                predecessor_run_id: None,
                settings: &json!({}),
            },
        )
        .await
    })
    .await;
    assert!(
        matches!(
            refused,
            Err(agent_workspace_server::error::AppError::LimitReached(_))
        ),
        "{refused:?}"
    );
    world.finish().await;
}

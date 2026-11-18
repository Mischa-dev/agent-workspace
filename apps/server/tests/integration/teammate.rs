//! A2.9: following and ambient mode. A follower of a page hears its update
//! (and not another page's), and nothing once it leaves the space. An
//! account in ambient mode in a channel hears every message there and is
//! invited to speak by at most one open optional request at a time, not
//! again within its cooldown after answering.

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

async fn tool(world: &Workspace, agent: &Agent, name: &'static str, arguments: Value) -> Value {
    let (_, body) = agent.call(&world.app, Call::Tool { name, arguments }).await;
    body
}

async fn subscribe(world: &Workspace, agent: &Agent, min_reason: &str) {
    let listed = tool(world, agent, "connection_list", json!({})).await;
    let connection = listed["value"]["connections"][0]["id"].clone();
    let updated = tool(
        world,
        agent,
        "connection_update",
        json!({
            "connectionId": connection,
            "subscriptions": [{ "kinds": ["*"], "minReason": min_reason }],
        }),
    )
    .await;
    assert_eq!(updated["outcome"], "ok", "{updated}");
}

/// The agent's events since `cursor` (or where it last read).
async fn read(world: &Workspace, agent: &Agent, cursor: Option<&Value>) -> Value {
    agent_workspace_server::events::catch_up(world.pool())
        .await
        .expect("sequence events");
    let arguments = cursor.map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
    let page = tool(world, agent, "event_list", arguments).await;
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

async fn page(world: &Workspace, agent: &Agent, title: &str) -> String {
    let made = tool(
        world,
        agent,
        "page_write",
        json!({"projectId": world.project("WORK"), "title": title, "body": "First."}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    made["value"]["id"].as_str().expect("page").to_owned()
}

async fn edit(world: &Workspace, agent: &Agent, page: &str, version: i64) {
    let edited = tool(
        world,
        agent,
        "page_write",
        json!({"pageId": page, "body": format!("Version {}.", version + 1), "expectedVersion": version}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
}

#[tokio::test]
async fn a_follower_hears_what_it_follows_until_it_leaves_the_space() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .agent("mara", "member")
        .build("teammate_follow")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let mara = &world.agents["mara"];
    subscribe(&world, rook, "following").await;
    let followed = page(&world, mara, "Followed").await;
    let other = page(&world, mara, "Other").await;
    let start = read(&world, rook, None).await["nextCursor"].clone();

    let set = tool(
        &world,
        rook,
        "follow_set",
        json!({"kind": "page", "id": followed}),
    )
    .await;
    assert_eq!(set["outcome"], "ok", "{set}");
    assert_eq!(set["value"]["level"], "changes");
    let listed = tool(&world, rook, "follow_list", json!({})).await;
    assert_eq!(
        listed["value"]["follows"][0]["id"],
        followed.as_str(),
        "{listed}"
    );

    edit(&world, mara, &followed, 1).await;
    edit(&world, mara, &other, 1).await;
    let heard = read(&world, rook, Some(&start)).await;
    let updates = of_type(&heard, "page.updated");
    assert_eq!(updates.len(), 1, "only the followed page: {heard}");
    assert_eq!(updates[0]["reason"], "following");
    assert!(heard.to_string().contains(&followed), "{heard}");
    assert!(!heard.to_string().contains(&other), "{heard}");

    // Once it leaves the space, following it tells it nothing.
    let before = heard["nextCursor"].clone();
    sqlx::query(
        "UPDATE project_memberships SET enabled = false \
         WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(world.project("WORK"))
    .bind(rook.actor)
    .execute(world.pool())
    .await
    .expect("rook leaves");
    edit(&world, mara, &followed, 2).await;
    let heard = read(&world, rook, Some(&before)).await;
    assert_eq!(heard["events"], json!([]), "{heard}");
    // And it can't follow what it no longer sees.
    let refused = tool(
        &world,
        rook,
        "follow_set",
        json!({"kind": "page", "id": other}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    world.finish().await;
}

async fn say(world: &Workspace, conversation: &str, text: &str) {
    let (status, sent) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": text}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
}

async fn open_requests(world: &Workspace, agent: Uuid, conversation: &str) -> Vec<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM reply_requests WHERE agent_actor_id = $1 \
         AND conversation_id = $2::uuid AND status IN ('pending', 'claimed') ORDER BY created_at",
    )
    .bind(agent)
    .bind(conversation)
    .fetch_all(world.pool())
    .await
    .expect("requests")
}

#[tokio::test]
async fn ambient_mode_invites_one_request_at_a_time_and_waits_out_its_cooldown() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_ambient")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{}/channels", world.project("WORK")),
            Some("channel-general"),
            json!({ "name": "general" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let general = made["conversation"]["id"]
        .as_str()
        .or_else(|| made["id"].as_str())
        .expect("channel")
        .to_owned();

    // Without ambient mode a message nobody addressed asks nobody.
    say(&world, &general, "Morning all.").await;
    assert!(open_requests(&world, rook.actor, &general).await.is_empty());

    let listed = op(
        &world,
        "conversation_agent_settings_list",
        json!({"conversation": general}),
    )
    .await;
    assert_eq!(
        listed["accounts"][0]["accountId"],
        rook.actor.to_string(),
        "{listed}"
    );
    assert_eq!(listed["accounts"][0]["ambient"], false);
    let set = op(
        &world,
        "conversation_agent_settings_set",
        json!({"conversation": general, "account": rook.actor, "ambient": true}),
    )
    .await;
    assert_eq!(set["ambient"], true, "{set}");
    assert_eq!(set["replyCooldownSeconds"], 300);
    let start = read(&world, rook, None).await["nextCursor"].clone();

    // Two messages: one open request, optional.
    say(&world, &general, "Anyone seen the build?").await;
    say(&world, &general, "It failed twice.").await;
    let open = open_requests(&world, rook.actor, &general).await;
    assert_eq!(open.len(), 1, "{open:?}");
    let required =
        sqlx::query_scalar::<_, bool>("SELECT required FROM reply_requests WHERE id = $1")
            .bind(open[0])
            .fetch_one(world.pool())
            .await
            .expect("request");
    assert!(!required);
    // It hears each message, though it subscribes to what is addressed.
    let heard = read(&world, rook, Some(&start)).await;
    let ambient = of_type(&heard, "message.created")
        .into_iter()
        .filter(|event| event["reason"] == "ambient")
        .count();
    assert_eq!(ambient, 2, "{heard}");

    // Answered: not invited again within the cooldown...
    let passed = tool(
        &world,
        rook,
        "reply_pass",
        json!({"requestId": open[0].to_string()}),
    )
    .await;
    assert_eq!(passed["outcome"], "ok", "{passed}");
    say(&world, &general, "Never mind.").await;
    assert!(open_requests(&world, rook.actor, &general).await.is_empty());

    // ...and with no cooldown, the next message invites it again.
    op(
        &world,
        "conversation_agent_settings_set",
        json!({"conversation": general, "account": rook.actor, "replyCooldownSeconds": 0}),
    )
    .await;
    say(&world, &general, "Actually, it's back.").await;
    assert_eq!(open_requests(&world, rook.actor, &general).await.len(), 1);

    // Ambient mode is for channels and threads.
    let (status, refused) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({ "actorId": rook.actor }),
        ),
    )
    .await;
    assert!(status.is_success(), "{refused}");
    let dm = refused["conversation"]["id"]
        .as_str()
        .expect("DM")
        .to_owned();
    let (_, refused) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!(
                "/api/v2/w/{}/ops/conversation_agent_settings_set",
                world.organization
            ),
            Some(&Uuid::now_v7().to_string()),
            json!({"conversation": dm, "account": rook.actor, "ambient": true}),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    world.finish().await;
}

async fn person_op(
    world: &Workspace,
    who: &super::support::app::Browser,
    name: &str,
    input: Value,
) -> Value {
    let (_, body) = send(
        &world.app,
        who.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await;
    body
}

/// A2.27: a team's own guards. A deny guard stops the changes it matches
/// and nothing else; a review guard holds a change until a reviewer allows
/// it, then the same call goes through once; declined, it is refused. A
/// person is held like an agent.
#[tokio::test]
async fn guards_stop_or_hold_changes_for_people_and_agents_alike() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("teammate_guards")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let lena = &world.people["lena"];
    let space = world.project("WORK");
    let decide = |title: &str| json!({"spaceId": space, "title": title});

    let denied = op(
        &world,
        "guard_set",
        json!({"name": "No launch talk", "action": "deny",
               "operations": ["decision_record"], "terms": ["LAUNCH"]}),
    )
    .await;
    let deny_guard = denied["guard"]["id"].clone();
    let refused = tool(&world, rook, "decision_record", decide("Launch is Friday")).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert!(refused.to_string().contains("No launch talk"), "{refused}");
    let fine = tool(&world, rook, "decision_record", decide("Use Postgres")).await;
    assert_eq!(fine["outcome"], "ok", "{fine}");
    // Anyone may read the guards.
    let listed = tool(&world, rook, "guard_list", json!({})).await;
    assert_eq!(
        listed["value"]["guards"][0]["name"], "No launch talk",
        "{listed}"
    );
    // Only those who manage settings change them.
    let not_rooks = tool(
        &world,
        rook,
        "guard_set",
        json!({"guardId": deny_guard, "remove": true}),
    )
    .await;
    assert_ne!(not_rooks["outcome"], "ok", "{not_rooks}");
    op(
        &world,
        "guard_set",
        json!({"guardId": deny_guard, "remove": true}),
    )
    .await;

    // Review: held, allowed, then the same call goes through once.
    op(
        &world,
        "guard_set",
        json!({"name": "Decisions get a look", "action": "review",
               "operations": ["decision_record"], "reviewers": [world.owner.actor]}),
    )
    .await;
    let held = tool(&world, rook, "decision_record", decide("Ship weekly")).await;
    assert_eq!(held["outcome"], "ok", "{held}");
    assert_eq!(held["value"]["status"], "pending_review", "{held}");
    let request = held["value"]["requestId"].clone();
    let again = tool(&world, rook, "decision_record", decide("Ship weekly")).await;
    assert_eq!(
        again["value"]["requestId"], request,
        "a retry waits on the same review"
    );
    let decisions = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM project_decisions WHERE title = 'Ship weekly'",
        )
        .fetch_one(world.pool())
        .await
        .expect("decisions")
    };
    assert_eq!(decisions().await, 0);
    op(
        &world,
        "request_respond",
        json!({"requestId": request, "optionId": "allow"}),
    )
    .await;
    let made = tool(&world, rook, "decision_record", decide("Ship weekly")).await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert!(made["value"]["status"] != "pending_review", "{made}");
    assert_eq!(decisions().await, 1);
    // Used once: the next one is reviewed again.
    let next = tool(&world, rook, "decision_record", decide("Ship weekly")).await;
    assert_eq!(next["value"]["status"], "pending_review", "{next}");

    // A person is held the same way, and a declined change is refused.
    let held = person_op(
        &world,
        &lena.browser,
        "decision_record",
        decide("Ship daily"),
    )
    .await;
    assert_eq!(held["value"]["status"], "pending_review", "{held}");
    op(
        &world,
        "request_respond",
        json!({"requestId": held["value"]["requestId"], "optionId": "deny", "text": "Too often"}),
    )
    .await;
    let declined = person_op(
        &world,
        &lena.browser,
        "decision_record",
        decide("Ship daily"),
    )
    .await;
    assert_eq!(declined["outcome"], "permission_denied", "{declined}");
    assert!(declined.to_string().contains("Too often"), "{declined}");
    world.finish().await;
}

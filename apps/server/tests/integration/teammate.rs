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

/// A2.27: a tool called by its old name is guarded like the operation it
/// runs, and a guard may name the old tool (`page_write` stands for
/// `page_create` and `page_update`). A deny guard on everything stops the
/// page; a review guard holds it, and once allowed the same call goes
/// through once.
#[tokio::test]
async fn guards_hold_tools_called_by_their_old_names() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_guards_tools")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let space = world.project("WORK");
    let write = |title: &str| json!({"projectId": space, "title": title, "body": "Notes."});
    let pool = world.pool().clone();
    let pages = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM project_pages WHERE title = 'Plan'")
                .fetch_one(&pool)
                .await
                .expect("pages")
        }
    };

    let denied = op(
        &world,
        "guard_set",
        json!({"name": "Hands off", "action": "deny"}),
    )
    .await;
    let refused = tool(&world, rook, "page_write", write("Plan")).await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(refused.to_string().contains("Hands off"), "{refused}");
    assert_eq!(pages().await, 0);
    op(
        &world,
        "guard_set",
        json!({"guardId": denied["guard"]["id"], "remove": true}),
    )
    .await;

    op(
        &world,
        "guard_set",
        json!({"name": "Pages get a look", "action": "review",
               "operations": ["page_write"], "reviewers": [world.owner.actor]}),
    )
    .await;
    let held = tool(&world, rook, "page_write", write("Plan")).await;
    assert_eq!(held["value"]["status"], "pending_review", "{held}");
    assert_eq!(pages().await, 0);
    op(
        &world,
        "request_respond",
        json!({"requestId": held["value"]["requestId"], "optionId": "allow"}),
    )
    .await;
    let made = tool(&world, rook, "page_write", write("Plan")).await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert_eq!(pages().await, 1);
    let next = tool(&world, rook, "page_write", write("Plan")).await;
    assert_eq!(next["value"]["status"], "pending_review", "{next}");
}

/// A wrapped tool's command runs apart from the call's transaction, so the
/// review it used is committed before it runs. When the command then
/// refuses, the review is given back: the same call, made again without a
/// request id once it can work, goes through without a second review.
#[tokio::test]
async fn a_review_a_refused_command_used_is_given_back() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_guards_refused_given_back")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let space = world.project("WORK");
    let first = op(
        &world,
        "task_create",
        json!({"projectId": space, "title": "First"}),
    )
    .await;
    let second = op(
        &world,
        "task_create",
        json!({"projectId": space, "title": "Second"}),
    )
    .await;
    let (first, second) = (
        first["workItemId"].as_str().expect("first").to_owned(),
        second["workItemId"].as_str().expect("second").to_owned(),
    );
    // In different columns, the two can't be put in order.
    sqlx::query(
        "UPDATE work_items SET status_id = (SELECT id FROM work_item_statuses \
         WHERE project_id = work_items.project_id AND id <> work_items.status_id \
         ORDER BY position LIMIT 1) WHERE id = $1",
    )
    .bind(second.parse::<Uuid>().expect("id"))
    .execute(world.pool())
    .await
    .expect("another column");
    op(
        &world,
        "guard_set",
        json!({"name": "Order gets a look", "action": "review",
               "operations": ["task_reorder"], "reviewers": [world.owner.actor]}),
    )
    .await;
    let reorder = json!({"taskIds": [second, first]});
    let held = tool(&world, rook, "task_reorder", reorder.clone()).await;
    assert_eq!(held["value"]["status"], "pending_review", "{held}");
    op(
        &world,
        "request_respond",
        json!({"requestId": held["value"]["requestId"], "optionId": "allow"}),
    )
    .await;
    let refused = tool(&world, rook, "task_reorder", reorder.clone()).await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert_ne!(refused["value"]["status"], "pending_review", "{refused}");

    sqlx::query(
        "UPDATE work_items SET status_id = (SELECT status_id FROM work_items WHERE id = $2) \
         WHERE id = $1",
    )
    .bind(second.parse::<Uuid>().expect("id"))
    .bind(first.parse::<Uuid>().expect("id"))
    .execute(world.pool())
    .await
    .expect("same column");
    let made = tool(&world, rook, "task_reorder", reorder.clone()).await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let next = tool(&world, rook, "task_reorder", reorder).await;
    assert_eq!(next["value"]["status"], "pending_review", "{next}");
}

/// A review given back after the same call asked again while the first
/// ran (the first then made nothing): the unanswered second ask is
/// withdrawn, and the allowed review is used by the next call. Once a
/// reviewer has answered the second ask, that answer stands instead.
#[tokio::test]
async fn a_review_given_back_replaces_the_ask_a_retry_raised_meanwhile() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_guards_give_back")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let space = world.project("WORK");
    op(
        &world,
        "guard_set",
        json!({"name": "Pages get a look", "action": "review",
               "operations": ["page_write"], "reviewers": [world.owner.actor]}),
    )
    .await;
    for (title, second_answered) in [("Plan", false), ("Notes", true)] {
        let write = json!({"projectId": space, "title": title, "body": "Notes."});
        let held = tool(&world, rook, "page_write", write.clone()).await;
        let first_request = held["value"]["requestId"]
            .as_str()
            .expect("request")
            .to_owned();
        op(
            &world,
            "request_respond",
            json!({"requestId": first_request, "optionId": "allow"}),
        )
        .await;
        // The first call has used the review and is still running.
        let first = sqlx::query_scalar::<_, Uuid>(
            "UPDATE guard_holds SET state = 'used' WHERE request_id = $1::uuid RETURNING id",
        )
        .bind(&first_request)
        .fetch_one(world.pool())
        .await
        .expect("the first call uses its review");
        let again = tool(&world, rook, "page_write", write.clone()).await;
        assert_eq!(again["value"]["status"], "pending_review", "{again}");
        let second_request = again["value"]["requestId"]
            .as_str()
            .expect("request")
            .to_owned();
        assert_ne!(second_request, first_request);
        if second_answered {
            op(
                &world,
                "request_respond",
                json!({"requestId": second_request, "optionId": "deny"}),
            )
            .await;
        }
        // The first call made nothing, so its review comes back.
        agent_workspace_server::tenancy::system("runs.host", async {
            let mut transaction = agent_workspace_server::db::scoped_tx(world.pool())
                .await
                .expect("transaction");
            agent_workspace_server::guards::give_back_hold(&mut transaction, first)
                .await
                .expect("given back");
            transaction.commit().await.expect("commit");
        })
        .await;
        let status =
            sqlx::query_scalar::<_, String>("SELECT status FROM requests WHERE id = $1::uuid")
                .bind(&second_request)
                .fetch_one(world.pool())
                .await
                .expect("the second ask");
        let made = tool(&world, rook, "page_write", write).await;
        let pages =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM project_pages WHERE title = $1")
                .bind(title)
                .fetch_one(world.pool())
                .await
                .expect("pages");
        if second_answered {
            assert_eq!(status, "answered", "the reviewer's answer stands");
            assert_ne!(made["outcome"], "ok", "{made}");
            assert!(
                made.to_string().contains("didn't allow this"),
                "refused by the reviewer's answer, not anything else: {made}"
            );
            assert_eq!(pages, 0, "the denial wins over the given-back review");
        } else {
            assert_eq!(status, "cancelled", "the second ask is withdrawn");
            assert_eq!(made["outcome"], "ok", "{made}");
            assert_eq!(pages, 1);
        }
    }
    world.finish().await;
}

/// A2.27: a guard on records sees a bulk record write as a change to
/// records, though the call is aimed at its space; the same for a
/// collection's and a sheet's writes, under their new and old names.
#[tokio::test]
async fn guards_on_records_collections_and_sheets_see_their_writes() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_guards_kinds")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let space = world.project("WORK");
    let made = tool(
        &world,
        rook,
        "collection_create",
        json!({"projectId": space, "name": "Bugs", "fields": [{"name": "Size", "type": "number"}]}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let collection = made["value"]["collection"]["id"].clone();
    let records = json!({"projectId": space, "collection": collection,
                         "records": [{"title": "Crash"}]});

    let guard = op(
        &world,
        "guard_set",
        json!({"name": "Records stay put", "action": "deny", "targetKinds": ["record"]}),
    )
    .await;
    for name in ["record_create_many", "record_write"] {
        let refused = tool(&world, rook, name, records.clone()).await;
        assert_ne!(refused["outcome"], "ok", "{name}: {refused}");
        assert!(
            refused.to_string().contains("Records stay put"),
            "{name}: {refused}"
        );
    }
    // A collection's own change isn't a record's.
    let described = tool(
        &world,
        rook,
        "collection_update",
        json!({"projectId": space, "collection": collection, "description": "Triage"}),
    )
    .await;
    assert_eq!(described["outcome"], "ok", "{described}");
    op(
        &world,
        "guard_set",
        json!({"guardId": guard["guard"]["id"], "remove": true}),
    )
    .await;
    let added = tool(&world, rook, "record_create_many", records).await;
    assert_eq!(added["outcome"], "ok", "{added}");

    op(
        &world,
        "guard_set",
        json!({"name": "Shapes stay put", "action": "deny",
               "targetKinds": ["collection", "sheet"]}),
    )
    .await;
    for (name, arguments) in [
        (
            "collection_update",
            json!({"projectId": space, "collection": collection, "description": "Later"}),
        ),
        (
            "sheet_create",
            json!({"projectId": space, "name": "Budget"}),
        ),
        ("sheet_write", json!({"projectId": space, "name": "Budget"})),
    ] {
        let refused = tool(&world, rook, name, arguments).await;
        assert_ne!(refused["outcome"], "ok", "{name}: {refused}");
        assert!(
            refused.to_string().contains("Shapes stay put"),
            "{name}: {refused}"
        );
    }
}

/// A2.27: a keyed write that an allowed review let through, retried after
/// a lost answer, is answered from its first run: the same records, said
/// to be a replay, with no second review asked.
#[tokio::test]
async fn a_keyed_retry_after_an_allowed_review_replays() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_guards_keyed_retry")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let space = world.project("WORK");
    let made = tool(
        &world,
        rook,
        "collection_create",
        json!({"projectId": space, "name": "Bugs"}),
    )
    .await;
    let collection = made["value"]["collection"]["id"].clone();
    op(
        &world,
        "guard_set",
        json!({"name": "Records get a look", "action": "review",
               "targetKinds": ["record"], "reviewers": [world.owner.actor]}),
    )
    .await;
    let call = json!({"projectId": space, "collection": collection,
                      "records": [{"title": "Crash"}], "clientRequestId": "add-crash-1"});
    let held = tool(&world, rook, "record_create_many", call.clone()).await;
    assert_eq!(held["value"]["status"], "pending_review", "{held}");
    op(
        &world,
        "request_respond",
        json!({"requestId": held["value"]["requestId"], "optionId": "allow"}),
    )
    .await;
    let first = tool(&world, rook, "record_create_many", call.clone()).await;
    assert_eq!(first["outcome"], "ok", "{first}");
    assert!(first["value"]["records"].is_array(), "{first}");
    let again = tool(&world, rook, "record_create_many", call).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(
        again["value"]["records"], first["value"]["records"],
        "{again}"
    );
    let pending =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guard_holds WHERE state = 'pending'")
            .fetch_one(world.pool())
            .await
            .expect("holds");
    assert_eq!(pending, 0);
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collection_records WHERE title = 'Crash'",
    )
    .fetch_one(world.pool())
    .await
    .expect("records");
    assert_eq!(kept, 1);
}

/// Identical calls that overlap (a retry sent while the first attempt is
/// still being checked) share one review: each is told it waits on the same
/// request, none fails on the second ask.
#[tokio::test]
async fn overlapping_identical_calls_share_one_review() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_guards_overlapping_retries")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let space = world.project("WORK");
    let made = tool(
        &world,
        rook,
        "collection_create",
        json!({"projectId": space, "name": "Bugs"}),
    )
    .await;
    let collection = made["value"]["collection"]["id"].clone();
    op(
        &world,
        "guard_set",
        json!({"name": "Records get a look", "action": "review",
               "targetKinds": ["record"], "reviewers": [world.owner.actor]}),
    )
    .await;
    let call = json!({"projectId": space, "collection": collection,
                      "records": [{"title": "Crash"}]});
    // Over the API, so each attempt is a request of its own.
    let path = format!("/api/v2/w/{}/ops/record_create_many", world.organization);
    let key = Uuid::now_v7().to_string();
    let answers = futures_util::future::join_all((0..3).map(|_| async {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(&path)
            .header("authorization", format!("Bearer {}", rook.token))
            .header("content-type", "application/json")
            .header("idempotency-key", &key)
            .body(axum::body::Body::from(call.to_string()))
            .expect("request");
        let (status, body) = send(&world.app, request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }))
    .await;
    let first = &answers[0];
    for answer in &answers {
        assert_eq!(answer["value"]["status"], "pending_review", "{answer}");
        assert_eq!(
            answer["value"]["requestId"], first["value"]["requestId"],
            "{answer}"
        );
    }
    let pending =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guard_holds WHERE state = 'pending'")
            .fetch_one(world.pool())
            .await
            .expect("holds");
    assert_eq!(pending, 1);
}

/// A conversation's memory page is that conversation's: a change to it held
/// for review goes only to reviewers who can read the conversation, and
/// with none who can, it is refused rather than shown to the others.
#[tokio::test]
async fn a_memory_page_change_is_reviewed_only_by_the_conversations_readers() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("teammate_guards_memory")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let space = world.project("WORK");
    let channel = tool(
        &world,
        rook,
        "channel_create",
        json!({"projectId": space, "name": "rook-notes", "private": true}),
    )
    .await;
    assert_eq!(channel["outcome"], "ok", "{channel}");
    let channel = channel["value"]["id"]
        .as_str()
        .or_else(|| channel["value"]["conversation"]["id"].as_str())
        .or_else(|| channel["value"]["conversationId"].as_str())
        .expect("channel")
        .to_owned();
    let memory = tool(
        &world,
        rook,
        "memory_page_set",
        json!({"conversation": channel, "body": "First.", "clientRequestId": Uuid::now_v7()}),
    )
    .await;
    assert_eq!(memory["outcome"], "ok", "{memory}");
    let page = memory["value"]["pageId"].as_str().expect("page").to_owned();
    // The only reviewer isn't in the private channel.
    op(
        &world,
        "guard_set",
        json!({"name": "Pages get a look", "action": "review",
               "operations": ["page_write", "file_upload"], "reviewers": [world.owner.actor]}),
    )
    .await;
    let written = tool(
        &world,
        rook,
        "page_write",
        // A link to the page, as the tool takes it, is the page too.
        json!({"pageId": format!("https://example.test/pages/{page}"), "body": "Second.", "expectedVersion": 1}),
    )
    .await;
    assert_ne!(written["pendingReview"], true, "{written}");
    assert!(
        written
            .to_string()
            .contains("none of its reviewers can read"),
        "{written}"
    );
    // Nor a file kept on the page, which names it inside `thing`.
    let uploaded = tool(
        &world,
        rook,
        "file_upload",
        json!({"thing": {"kind": "page", "id": page}, "name": "plan.txt",
               "text": "Second plan."}),
    )
    .await;
    assert_ne!(uploaded["pendingReview"], true, "{uploaded}");
    assert!(
        uploaded
            .to_string()
            .contains("none of its reviewers can read"),
        "{uploaded}"
    );
    let held = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guard_holds")
        .fetch_one(world.pool())
        .await
        .expect("holds");
    assert_eq!(held, 0);
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
    // A workspace's guard isn't changed under a space's name: who may
    // change it is decided by its own scope.
    let (_, elsewhere) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/guard_set", world.organization),
            Some(&Uuid::now_v7().to_string()),
            json!({"guardId": deny_guard, "space": space, "remove": true}),
        ),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "not_found", "{elsewhere}");
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
    let keyed = || {
        let mut input = decide("Ship weekly");
        input["clientRequestId"] = "ship-weekly-1".into();
        input
    };
    let made = tool(&world, rook, "decision_record", keyed()).await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert!(made["value"]["status"] != "pending_review", "{made}");
    assert_eq!(decisions().await, 1);
    // Its answer lost, the retry gets the same answer, not a new review.
    let retried = tool(&world, rook, "decision_record", keyed()).await;
    assert_eq!(retried["replayed"], true, "{retried}");
    assert_eq!(retried["value"], made["value"], "{retried}");
    assert_eq!(decisions().await, 1);
    let reviews = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM guard_holds WHERE operation = 'decision_record' AND state = 'pending'",
    )
    .fetch_one(world.pool())
    .await
    .expect("holds");
    assert_eq!(reviews, 0, "no review asked for a finished call");
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

    // A held message in a private channel goes only to reviewers who can
    // read that channel, its request is the channel's, and the call's
    // secrets are never kept or shown.
    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/channels"),
            Some("guarded-private"),
            json!({"name": "guarded", "private": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let channel = made["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let (status, added) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/conversations/{channel}/members"),
            Some("guarded-add-rook"),
            json!({"actorId": rook.actor}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    op(
        &world,
        "guard_set",
        json!({"name": "Messages get a look", "action": "review",
               "operations": ["message_send"],
               "reviewers": [world.owner.actor, lena.actor]}),
    )
    .await;
    let word = format!("zqheld{}", &Uuid::now_v7().simple().to_string()[20..]);
    let held = tool(
        &world,
        rook,
        "message_send",
        json!({"conversationId": channel, "text": format!("The plan is {word}")}),
    )
    .await;
    assert_eq!(held["value"]["status"], "pending_review", "{held}");
    let request: Uuid = held["value"]["requestId"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("request");
    let (addressed, conversation) = sqlx::query_as::<_, (Vec<Uuid>, Option<Uuid>)>(
        "SELECT addressed_account_ids, conversation_id FROM requests WHERE id = $1",
    )
    .bind(request)
    .fetch_one(world.pool())
    .await
    .expect("request row");
    assert_eq!(
        addressed,
        vec![world.owner.actor],
        "only a reviewer in the channel"
    );
    assert_eq!(conversation.map(|id| id.to_string()), Some(channel.clone()));
    let lenas = person_op(
        &world,
        &lena.browser,
        "request_get",
        json!({"requestId": request}),
    )
    .await;
    assert_ne!(lenas["outcome"], "ok", "{lenas}");
    assert!(!lenas.to_string().contains(&word), "{lenas}");
    let owners = person_op(
        &world,
        &world.owner.browser,
        "request_get",
        json!({"requestId": request}),
    )
    .await;
    assert!(owners.to_string().contains(&word), "{owners}");
    // A decision recorded from the channel is the channel's too, though it
    // is made in the space.
    let held = tool(
        &world,
        rook,
        "decision_record",
        json!({"spaceId": space, "sourceConversationId": channel,
               "title": format!("Go with {word}")}),
    )
    .await;
    assert_eq!(held["value"]["status"], "pending_review", "{held}");
    let request: Uuid = held["value"]["requestId"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("request");
    let conversation =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT conversation_id FROM requests WHERE id = $1")
            .bind(request)
            .fetch_one(world.pool())
            .await
            .expect("request row");
    assert_eq!(conversation.map(|id| id.to_string()), Some(channel.clone()));
    let lenas = person_op(
        &world,
        &lena.browser,
        "request_get",
        json!({"requestId": request}),
    )
    .await;
    assert!(!lenas.to_string().contains(&word), "{lenas}");
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM information_schema.columns \
         WHERE table_name = 'guard_holds' AND column_name = 'input'",
    )
    .fetch_one(world.pool())
    .await
    .expect("columns");
    assert_eq!(kept, 0, "a held call's input is never stored");
    world.finish().await;
}

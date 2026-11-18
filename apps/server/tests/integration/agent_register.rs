//! A2.19 (A2 3.9, rulings R19 and R21): a host offers ways to get a new
//! agent, proposes one it made, and a person who may add agents approves it.
//! The silent failures: a host retry making two agents, someone without
//! `agents.create` approving one (directly or by answering the request), an
//! approved agent not bound to the host that made it, and one person
//! reading another's action reply.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::Person,
    test_host::TestHost,
    world::{Workspace, WorldBuilder},
};

async fn op(world: &Workspace, who: &Person, name: &str, input: Value) -> Value {
    let (_, body) = send(
        &world.app,
        who.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await;
    body
}

fn register(world: &Workspace, client_request_id: &str, reference: &str, handle: &str) -> Value {
    json!({
        "workspaceId": world.organization,
        "hostAgentRef": reference,
        "profile": {"name": "Helper", "handle": handle, "summary": "Tidies the backlog"},
        "ref": {"issuer": "https://host.example", "id": reference, "lineage": "l-01"},
        "clientRequestId": client_request_id,
    })
}

#[tokio::test]
async fn a_host_proposes_an_agent_and_someone_who_may_add_agents_approves_it() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("agent_register")
        .await
    else {
        return;
    };
    let owner = &world.owner;
    let lena = &world.people["lena"];
    let mut host = TestHost::enrol(
        &world.app,
        &owner.browser,
        world.organization,
        "Team host",
        json!({"connection": "dial_out"}),
    )
    .await;
    let (status, described) = host
        .call(
            "POST",
            "/api/v1/host/describe",
            Some(json!({
                "software": {"name": "test-host", "version": "1.0"},
                "capabilities": {"start": true},
                "reportIntervalSeconds": 60,
                "issuer": "https://host.example",
                "offers": [
                    {"id": "new", "title": "New agent", "start": {"page": "https://host.example/new"}},
                    {"id": "away", "title": "Elsewhere", "start": {"page": "https://other.example/new"}},
                ],
                "actions": [{"id": "sign-ins", "label": "Model sign-ins", "needs": "panel"}],
                "agents": [{"hostAgentRef": "helper", "name": "Helper",
                            "actions": [{"id": "pause", "label": "Pause", "needs": "confirm"}]}],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{described}");

    // The owner sees the host's offer; lena, who can't add agents, can't.
    let offers = op(&world, owner, "agent_offer_list", json!({})).await;
    assert_eq!(offers["outcome"], "ok", "{offers}");
    // The built-in host's own offers come first; this host's follow.
    let from_host: Vec<&Value> = offers["value"]["offers"]
        .as_array()
        .expect("offers")
        .iter()
        .filter(|offer| offer["hostId"] == host.id.to_string())
        .collect();
    assert_eq!(from_host[0]["offer"]["id"], "new", "{offers}");
    let refused = op(&world, lena, "agent_offer_list", json!({})).await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    // Starting an offer tells the host who chose it, over its own channel,
    // and opens its page only on its own origin.
    let started = op(
        &world,
        owner,
        "agent_offer_start",
        json!({"host": host.id, "offerId": "new"}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    let page = started["value"]["page"].as_str().expect("page");
    assert!(
        page.starts_with("https://host.example/new?command="),
        "{page}"
    );
    let elsewhere_page = op(
        &world,
        owner,
        "agent_offer_start",
        json!({"host": host.id, "offerId": "away"}),
    )
    .await;
    assert_eq!(
        elsewhere_page["value"]["page"],
        Value::Null,
        "{elsewhere_page}"
    );
    let commands = host.poll().await;
    let chosen = commands
        .iter()
        .find(|command| command["commandId"] == started["value"]["commandId"])
        .expect("offer command");
    let (_, chosen) = host.fetch(&chosen["commandId"]).await;
    assert_eq!(chosen["offer"], "new", "{chosen}");
    assert_eq!(chosen["requestedBy"], owner.actor.to_string(), "{chosen}");
    let refused = op(
        &world,
        lena,
        "agent_offer_start",
        json!({"host": host.id, "offerId": "new"}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    // Who is asked: the person who chose the offer, if they may add agents.
    for (key, chooser, asked) in [
        ("c-asks-owner", owner.actor, true),
        ("c-asks-lena", lena.actor, false),
    ] {
        let mut body = register(&world, key, key, key);
        body["requestedBy"] = json!(chooser);
        let (_, proposed) = host
            .call("POST", "/api/v1/host/agents/register", Some(body))
            .await;
        let addressed = sqlx::query_scalar::<_, Vec<Uuid>>(
            "SELECT addressed_account_ids FROM requests WHERE id = $1",
        )
        .bind(
            proposed["requestId"]
                .as_str()
                .expect("id")
                .parse::<Uuid>()
                .expect("uuid"),
        )
        .fetch_one(world.pool())
        .await
        .expect("request");
        assert_eq!(
            addressed.contains(&owner.actor),
            asked,
            "{key}: {addressed:?}"
        );
        assert!(!addressed.contains(&lena.actor), "{key}: {addressed:?}");
    }

    // Not for a workspace that isn't the host's (nor its owner's).
    let elsewhere = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(elsewhere)
        .bind(format!("elsewhere-{}", elsewhere.simple()))
        .execute(world.pool())
        .await
        .expect("workspace");
    let mut foreign = register(&world, "c-0", "helper", "helper");
    foreign["workspaceId"] = json!(elsewhere);
    let (status, refused) = host
        .call("POST", "/api/v1/host/agents/register", Some(foreign))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    // The host proposes; a retry is the same proposal, with one request.
    let (status, proposed) = host
        .call(
            "POST",
            "/api/v1/host/agents/register",
            Some(register(&world, "c-1", "helper", "helper")),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{proposed}");
    assert_eq!(proposed["status"], "proposed");
    let (status, again) = host
        .call(
            "POST",
            "/api/v1/host/agents/register",
            Some(register(&world, "c-1", "helper", "helper")),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{again}");
    assert_eq!(again["proposalId"], proposed["proposalId"]);
    let requests = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM requests WHERE subject_kind = 'agent_register' \
         AND subject_id IN (SELECT id FROM agent_proposals WHERE host_id = $1 AND client_request_id = 'c-1')",
    )
    .bind(host.id)
    .fetch_one(world.pool())
    .await
    .expect("requests");
    assert_eq!(requests, 1);

    // Lena can't approve it, directly or by answering its request.
    let decided = op(
        &world,
        lena,
        "agent_register_decide",
        json!({"proposal": proposed["proposalId"], "decision": "approve"}),
    )
    .await;
    assert_ne!(decided["outcome"], "ok", "{decided}");
    let answered = op(
        &world,
        lena,
        "request_respond",
        json!({"requestId": proposed["requestId"], "optionId": "approve"}),
    )
    .await;
    assert_ne!(answered["outcome"], "ok", "{answered}");

    // The owner answers the request: the agent is made, owned by the
    // workspace (as its host is), a member, and bound to the host.
    let answered = op(
        &world,
        owner,
        "request_respond",
        json!({"requestId": proposed["requestId"], "optionId": "approve"}),
    )
    .await;
    assert_eq!(answered["outcome"], "ok", "{answered}");
    let (status, account) = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT status, account_id FROM agent_proposals WHERE id = $1",
    )
    .bind(
        proposed["proposalId"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("uuid"),
    )
    .fetch_one(world.pool())
    .await
    .expect("proposal");
    assert_eq!(status, "approved");
    let helper = account.expect("account");
    let (kind, handle, bound) = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT actor.kind, profile.handle, binding.host_agent_ref
        FROM actors actor
        JOIN organization_actor_profiles profile
          ON profile.actor_id = actor.id AND profile.organization_id = $2
        JOIN host_agents binding ON binding.account_id = actor.id AND binding.host_id = $3
        WHERE actor.id = $1
        "#,
    )
    .bind(helper)
    .bind(world.organization)
    .bind(host.id)
    .fetch_one(world.pool())
    .await
    .expect("agent");
    assert_ne!(kind, "human");
    assert_eq!((handle.as_str(), bound.as_str()), ("helper", "helper"));
    // Retried after approval, the host learns the account.
    let (_, again) = host
        .call(
            "POST",
            "/api/v1/host/agents/register",
            Some(register(&world, "c-1", "helper", "helper")),
        )
        .await;
    assert_eq!(again["accountId"], helper.to_string(), "{again}");

    // Refused with the operation, its request is withdrawn.
    let (_, second) = host
        .call(
            "POST",
            "/api/v1/host/agents/register",
            Some(register(&world, "c-2", "other", "other")),
        )
        .await;
    let decided = op(
        &world,
        owner,
        "agent_register_decide",
        json!({"proposal": second["proposalId"], "decision": "refuse"}),
    )
    .await;
    assert_eq!(decided["value"]["status"], "refused", "{decided}");
    let open = sqlx::query_scalar::<_, String>("SELECT status FROM requests WHERE id = $1")
        .bind(
            second["requestId"]
                .as_str()
                .expect("id")
                .parse::<Uuid>()
                .expect("uuid"),
        )
        .fetch_one(world.pool())
        .await
        .expect("request");
    assert_ne!(open, "open");

    // Actions: only declared ones run, the host gets them, and only the
    // person who ran one reads its reply.
    let undeclared = op(
        &world,
        owner,
        "agent_action_run",
        json!({"account": helper, "actionId": "delete-everything"}),
    )
    .await;
    assert_ne!(undeclared["outcome"], "ok", "{undeclared}");
    let ran = op(
        &world,
        owner,
        "agent_action_run",
        json!({"account": helper, "actionId": "pause", "input": {"confirm": true}}),
    )
    .await;
    assert_eq!(ran["outcome"], "ok", "{ran}");
    let command_id = ran["value"]["commandId"].clone();
    let commands = host.poll().await;
    assert!(
        commands
            .iter()
            .any(|command| command["commandId"] == command_id),
        "{commands:?}"
    );
    let (status, fetched) = host.fetch(&command_id).await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["action"], "pause", "{fetched}");
    assert_eq!(fetched["hostAgentRef"], "helper", "{fetched}");
    let (status, acked) = host
        .ack(
            &command_id,
            json!({"outcome": "done", "result": {"paused": true}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{acked}");
    let reply = op(
        &world,
        owner,
        "agent_action_get",
        json!({"commandId": command_id}),
    )
    .await;
    assert_eq!(reply["value"]["reply"]["result"]["paused"], true, "{reply}");
    let theirs = op(
        &world,
        lena,
        "agent_action_get",
        json!({"commandId": command_id}),
    )
    .await;
    assert_ne!(theirs["outcome"], "ok", "{theirs}");
    let host_action = op(
        &world,
        owner,
        "agent_action_run",
        json!({"host": host.id, "actionId": "sign-ins"}),
    )
    .await;
    assert_eq!(host_action["outcome"], "ok", "{host_action}");

    world.finish().await;
}

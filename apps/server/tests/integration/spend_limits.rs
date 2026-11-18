//! H1.27: spend limits. The silent failures: a per-minute limit that only
//! holds people back (or only agents), a limit on one account that stops
//! everyone, and a soft limit that tells nobody.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

async fn channel(world: &Workspace, name: &str) -> String {
    let (status, channel) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{}/channels", world.project("WORK")),
            Some(&format!("channel-{name}")),
            json!({ "name": name }),
        ),
    )
    .await;
    assert!(status.is_success(), "{channel}");
    channel["conversation"]["id"]
        .as_str()
        .or_else(|| channel["id"].as_str())
        .expect("channel")
        .to_owned()
}

/// Runs an operation as a person through the v2 API.
async fn op(world: &Workspace, who: Option<&str>, name: &str, input: Value) -> (u16, Value) {
    let browser = match who {
        Some(handle) => &world.people[handle].browser,
        None => &world.owner.browser,
    };
    let key = Uuid::now_v7().to_string();
    let (status, body) = send(
        &world.app,
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(&key),
            input,
        ),
    )
    .await;
    (status.as_u16(), body)
}

async fn set(world: &Workspace, input: Value) {
    let (status, body) = op(world, None, "spend_limit_set", input).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["outcome"], "ok", "{body}");
}

/// A person's send: true when it went through, false when a limit refused
/// it (anything else fails the test).
async fn person_sends(world: &Workspace, who: &str, conversation: &str) -> bool {
    let (status, body) = op(
        world,
        Some(who),
        "message_send",
        json!({"conversationId": conversation, "text": "Hello"}),
    )
    .await;
    if body["outcome"] == "ok" {
        return true;
    }
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED.as_u16(), "{body}");
    assert_eq!(body["outcome"], "limit_reached", "{body}");
    false
}

async fn agent_sends(world: &Workspace, who: &str, conversation: &str) -> bool {
    let (_, body) = world.agents[who]
        .call(
            &world.app,
            Call::Tool {
                name: "message_send",
                arguments: json!({"conversationId": conversation, "text": "Hello",
                                  "clientRequestId": Uuid::now_v7().to_string()}),
            },
        )
        .await;
    if body["outcome"] == "ok" {
        return true;
    }
    assert_eq!(body["outcome"], "limit_reached", "{body}");
    false
}

#[tokio::test]
async fn limits_hold_people_and_agents_alike_and_tell_the_managers() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .agent("rook", "member")
        .agent("kite", "member")
        .build("h127_spend_limits")
        .await
    else {
        return;
    };
    let general = channel(&world, "limits-general").await;

    // Two messages a minute per account in this channel: a person and an
    // agent each get two, and a third account is not held back by theirs.
    set(
        &world,
        json!({"scope": "conversation", "scopeId": general, "meter": "messages",
               "period": "minute", "perAccount": true, "hardLimit": 2}),
    )
    .await;
    assert!(person_sends(&world, "ana", &general).await);
    assert!(person_sends(&world, "ana", &general).await);
    assert!(!person_sends(&world, "ana", &general).await);
    assert!(agent_sends(&world, "rook", &general).await);
    assert!(agent_sends(&world, "rook", &general).await);
    assert!(!agent_sends(&world, "rook", &general).await);
    assert!(agent_sends(&world, "kite", &general).await);

    let (_, listed) = op(&world, None, "spend_limit_list", json!({})).await;
    let limits = listed["value"]["limits"].as_array().expect("limits");
    assert!(
        limits
            .iter()
            .any(|limit| limit["period"] == "minute" && limit["perAccount"] == true),
        "{listed}"
    );
    set(
        &world,
        json!({"scope": "conversation", "scopeId": general, "meter": "messages",
               "period": "minute", "perAccount": true, "remove": true}),
    )
    .await;

    // A day's limit on one agent stops that agent and nobody else.
    let rook = world.agents["rook"].actor;
    set(
        &world,
        json!({"scope": "account", "scopeId": rook, "meter": "messages",
               "period": "day", "hardLimit": 1}),
    )
    .await;
    assert!(agent_sends(&world, "rook", &general).await);
    assert!(!agent_sends(&world, "rook", &general).await);
    assert!(agent_sends(&world, "kite", &general).await);
    assert!(person_sends(&world, "ana", &general).await);

    // A soft limit refuses nothing and lands in the managers' inbox.
    set(
        &world,
        json!({"scope": "workspace", "meter": "messages", "period": "day",
               "softLimit": 1}),
    )
    .await;
    assert!(person_sends(&world, "ana", &general).await);
    assert!(person_sends(&world, "ana", &general).await);
    let registry = agent_workspace_server::jobs::default_registry();
    let options = agent_workspace_server::jobs::WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "spend-limits-test".to_owned(),
    };
    agent_workspace_server::jobs::run_due(&world.state, &registry, &options)
        .await
        .expect("run jobs");
    let notices = sqlx::query_as::<_, (Uuid, Value)>(
        "SELECT recipient_actor_id, detail FROM notifications WHERE kind = 'spend_limit'",
    )
    .fetch_all(world.pool())
    .await
    .expect("notices");
    let jobs = sqlx::query_as::<_, (String, Option<String>, Value)>(
        "SELECT state, last_error, payload FROM background_jobs WHERE kind = 'metering.limit_notice'",
    )
    .fetch_all(world.pool())
    .await
    .expect("notice jobs");
    let given = sqlx::query_as::<_, (String, String, String, bool)>(
        "SELECT space.kind, membership.actor_id::text, membership.role, \
                role_allows(membership.role, membership.role_id, 'settings.manage') \
         FROM project_memberships membership JOIN spaces space ON space.id = membership.project_id \
         WHERE space.workspace_id = $1 AND membership.enabled",
    )
    .bind(world.organization)
    .fetch_all(world.pool())
    .await
    .expect("limit notices");
    assert!(
        notices
            .iter()
            .any(|(recipient, detail)| *recipient == world.owner.actor
                && detail["level"] == "soft"
                && detail["scopeKind"] == "workspace"),
        "{notices:?} {jobs:?} {given:?}"
    );
    // The refused agent's own limit reached the owner too. Ana (a member,
    // not a manager) heard only of the limit she reached herself.
    assert!(
        notices
            .iter()
            .any(|(recipient, detail)| *recipient == world.owner.actor
                && detail["level"] == "hard"
                && detail["scopeKind"] == "account"),
        "{notices:?}"
    );
    let ana = world.people["ana"].actor;
    assert!(
        notices
            .iter()
            .filter(|(recipient, _)| *recipient == ana)
            .all(|(_, detail)| detail["scopeKind"] == "account" && detail["level"] == "hard"),
        "{notices:?}"
    );
    world.finish().await;
}

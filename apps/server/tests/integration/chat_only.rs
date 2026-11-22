//! K4.25 (ruling R22): a workspace with only chat on, and no project
//! anywhere, is whole. Channels and private channels in the workspace's own
//! space, threads, DMs and groups, mentions, search, the inbox, Later and
//! joining by link all work, and the inbox's own words never say project,
//! task or run. It runs once with people only and once with an agent member
//! (R31: an agent joins like any account; the badge is account information).

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{Browser, send, tool},
    callers::drain_outbox,
    world::WorldBuilder,
};

/// Every module but chat's, switched off for the workspace.
const OFF: [&str; 11] = [
    "projects",
    "tasks",
    "pages",
    "whiteboards",
    "collections",
    "sheets",
    "runs",
    "code",
    "agents",
    "automations",
    "suggestions",
];

#[tokio::test]
async fn a_chat_only_workspace_is_whole_with_people_only() {
    chat_only_workspace("chat_only_people", false).await;
}

#[tokio::test]
async fn a_chat_only_workspace_is_whole_with_an_agent_member() {
    chat_only_workspace("chat_only_agent", true).await;
}

fn content(nodes: Value) -> Value {
    json!({ "content": { "version": 1, "nodes": nodes } })
}

async fn chat_only_workspace(name: &str, with_agent: bool) {
    let mut builder = WorldBuilder::default()
        .person("ana", "member")
        .person("ben", "member");
    if with_agent {
        builder = builder.agent("mara", "member");
    }
    let Some(world) = builder.build(name).await else {
        return;
    };
    let pool = world.pool();
    let (ana, ben) = (&world.people["ana"], &world.people["ben"]);
    let organization = world.organization;

    // Only chat: Messages, Channels and the inbox.
    let switches = OFF
        .iter()
        .map(|module| ((*module).to_owned(), Value::Bool(false)))
        .collect::<serde_json::Map<_, _>>();
    let (status, switched) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{organization}/ops/workspace_update"),
            Some("chat-only-switches"),
            json!({ "projectId": "workspace", "modules": switches }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{switched}");
    let others = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM spaces WHERE workspace_id = $1 AND kind <> 'workspace'",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .expect("spaces");
    assert_eq!(others, 0, "no project anywhere");
    let space = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM spaces WHERE workspace_id = $1 AND kind = 'workspace'",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .expect("the workspace's space");

    // Channels, open and private, in the workspace's own space.
    let mut channels = Vec::new();
    for (key, body) in [
        ("lounge", json!({ "name": "lounge" })),
        ("backroom", json!({ "name": "backroom", "private": true })),
    ] {
        let (status, made) = send(
            &world.app,
            ana.browser.request(
                "POST",
                &format!("/api/v1/projects/{space}/channels"),
                Some(&format!("{name}-{key}")),
                body,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{key}: {made}");
        channels.push(
            made["conversation"]["id"]
                .as_str()
                .expect("channel")
                .to_owned(),
        );
    }
    let lounge = channels[0].clone();

    // A mention in the open channel, and a thread from it.
    let word = format!("chatword{}", Uuid::now_v7().simple());
    let mut mentioned = vec![json!({ "type": "mention", "actorId": ben.actor })];
    if with_agent {
        mentioned.push(json!({ "type": "mention", "actorId": world.agents["mara"].actor }));
    }
    let mut nodes = vec![json!({ "type": "text", "text": format!("{word} ") })];
    nodes.extend(mentioned);
    let (status, sent) = send(
        &world.app,
        ana.browser.request(
            "POST",
            &format!("/api/v1/conversations/{lounge}/messages"),
            Some(&format!("{name}-mention")),
            content(Value::Array(nodes)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let message = sent["message"]["id"].as_str().expect("message").to_owned();
    let (status, thread) = send(
        &world.app,
        ana.browser.request(
            "POST",
            &format!("/api/v1/channels/{lounge}/threads"),
            Some(&format!("{name}-thread")),
            json!({ "originMessageId": message }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{thread}");
    let thread = thread["conversation"]["id"]
        .as_str()
        .expect("thread")
        .to_owned();
    let (status, reply) = send(
        &world.app,
        ben.browser.request(
            "POST",
            &format!("/api/v1/conversations/{thread}/messages"),
            Some(&format!("{name}-thread-reply")),
            content(json!([{ "type": "text", "text": "in the thread" }])),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");

    // A DM and a group.
    let (status, dm) = send(
        &world.app,
        ana.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({ "actorId": ben.actor }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {dm}");
    let dm = dm["conversation"]["id"].as_str().expect("DM").to_owned();
    let (status, said) = send(
        &world.app,
        ana.browser.request(
            "POST",
            &format!("/api/v1/conversations/{dm}/messages"),
            Some(&format!("{name}-dm")),
            content(json!([{ "type": "text", "text": "hello" }])),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{said}");
    let mut members = vec![ben.actor];
    if with_agent {
        members.push(world.agents["mara"].actor);
    }
    let (status, group) = send(
        &world.app,
        ana.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some(&format!("{name}-group")),
            json!({ "name": "crew", "memberIds": members }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");

    // An agent member reads the channel it was mentioned in.
    if with_agent {
        let mara = &world.agents["mara"];
        let read = tool(
            &world.app,
            &mara.token,
            &mara.session,
            "conversation_read",
            json!({ "conversationId": lounge }),
        )
        .await;
        assert!(read.to_string().contains(&word), "{read}");
    }

    // Ben finds the message, is told of the mention, and saves it for later.
    let (status, found) = send(
        &world.app,
        ben.browser.request(
            "GET",
            &format!("/api/v1/search?q={word}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert!(found.to_string().contains(&message), "{found}");
    drain_outbox(&world.state).await;
    let (status, inbox) = send(
        &world.app,
        ben.browser
            .request("GET", "/api/v1/inbox", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inbox}");
    let items = inbox["notifications"]
        .as_array()
        .or_else(|| inbox["items"].as_array())
        .unwrap_or_else(|| panic!("inbox items: {inbox}"));
    assert!(!items.is_empty(), "ben is told of the mention: {inbox}");
    for item in items {
        let title = item["title"].as_str().unwrap_or_default().to_lowercase();
        for word in ["project", "task", "run"] {
            assert!(!title.contains(word), "the inbox says {word:?}: {item}");
        }
    }
    let (status, saved) = ben
        .browser
        .op(&world.app, "later_save", json!({ "messageId": message }))
        .await;
    assert!(status.is_success(), "{status} {saved}");

    // Someone new joins by link and can start a DM.
    let member = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM workspace_roles WHERE organization_id = $1 AND key = 'member'",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .expect("member role");
    let (status, link) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{organization}/ops/invite_link_create"),
            Some(&format!("{name}-link")),
            json!({ "roleId": member }),
        ),
    )
    .await;
    assert_eq!(link["outcome"], "ok", "{status} {link}");
    let token = link["value"]["token"].as_str().expect("token");
    let anonymous = Browser {
        cookie: String::new(),
        csrf: String::new(),
    };
    let (status, joined) = send(
        &world.app,
        anonymous.request(
            "POST",
            "/api/v1/invite-links/accept-new",
            None,
            json!({
                "token": token,
                "loginName": format!("cleo-{name}"),
                "displayName": "Cleo",
                "handle": "cleo",
                "password": "correct horse battery staple",
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {joined}");
    let cleo = sqlx::query_scalar::<_, Uuid>(
        "SELECT membership.actor_id FROM organization_memberships membership \
         JOIN users account ON account.actor_id = membership.actor_id \
         WHERE membership.organization_id = $1 AND account.login_name = $2 AND membership.enabled",
    )
    .bind(organization)
    .bind(format!("cleo-{name}"))
    .fetch_one(pool)
    .await
    .expect("Cleo joined");
    let (status, opened) = send(
        &world.app,
        ana.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({ "actorId": cleo }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {opened}");
    world.finish().await;
}

//! K5.19: the live contract's conformance suite (`docs/contracts/live.md`).
//! It runs against this server's own live role, against a separate
//! `live`-only instance fed by a `work` instance's publishes, and, with
//! `AGENT_WORKSPACE_LIVE_CONTRACT_URL` set, against any live service at
//! that address (it must check tickets with this server's
//! `/api/v2/live/key` and take publishes at `/live/v1/publish`).
//!
//! What it checks: a ticket opens a connection and nothing else does (a
//! tampered or expired one is refused); a frame reaches exactly the
//! audiences it names; durable frames come in position order; a resume
//! through `api` replays what was missed with no gap and no repeat;
//! `ticket.stale` asks for a new ticket, which widens what is heard; a
//! signed-out session's connections close.

use std::time::{Duration, Instant};

use agent_workspace_server::{
    bootstrap::{BootstrapCommand, bootstrap_development_member},
    realtime::contract::ticket::{Audience, ConnectionKind, Signer, Ticket},
    secrets,
};
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::{
    agent_interface::{Browser, PASSWORD},
    cluster_support::{Cluster, Instance, Socket, cluster_with},
    support::two_workspaces::{second_owner_login, second_workspace_owner},
};

/// Where the suite's connections go.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// This server's own live role, in the one instance.
    InProcess,
    /// A `live`-only instance, fed by publishes from an `api,work` one (or
    /// the service at `AGENT_WORKSPACE_LIVE_CONTRACT_URL`).
    Separate,
}

struct World {
    cluster: Cluster,
    /// The live side's address.
    live: String,
    _keys: tempfile::TempDir,
}

impl World {
    /// The instance that serves `api` (and `work`).
    fn api(&self) -> &Instance {
        self.cluster.instance(0)
    }
}

async fn world(name: &str, target: Target) -> Option<World> {
    let keys = tempfile::tempdir().expect("keys");
    let keyring = keys.path().join("secrets.keyring");
    secrets::write_new_keyring(&keyring, &secrets::FileKeyring::generate()).expect("keyring");
    let keyring = keyring.to_string_lossy().into_owned();
    let shared: &[(&str, &str)] = &[("AGENT_WORKSPACE_SECRETS_KEY_PATH", &keyring)];
    let outside = std::env::var("AGENT_WORKSPACE_LIVE_CONTRACT_URL").ok();
    match (target, outside) {
        (Target::InProcess, _) => {
            let cluster = cluster_with(name, 1, shared).await?;
            let live = cluster.instance(0).base().to_owned();
            Some(World {
                cluster,
                live,
                _keys: keys,
            })
        }
        (Target::Separate, Some(url)) => {
            let mut cluster = cluster_with(name, 0, shared).await?;
            cluster
                .start(&[
                    shared[0],
                    ("AGENT_WORKSPACE_ROLES", "api,work"),
                    ("AGENT_WORKSPACE_LIVE_URL", &url),
                ])
                .await;
            Some(World {
                cluster,
                live: url.trim_end_matches('/').to_owned(),
                _keys: keys,
            })
        }
        (Target::Separate, None) => {
            let mut cluster = cluster_with(name, 0, shared).await?;
            // The live instance takes its frames from publishes only.
            let live = cluster
                .start(&[
                    shared[0],
                    ("AGENT_WORKSPACE_ROLES", "live"),
                    ("AGENT_WORKSPACE_LIVE_URL", "http://127.0.0.1:9"),
                ])
                .await
                .base()
                .to_owned();
            cluster
                .start(&[
                    shared[0],
                    ("AGENT_WORKSPACE_ROLES", "api,work"),
                    ("AGENT_WORKSPACE_LIVE_URL", &live),
                ])
                .await;
            cluster.instances.swap(0, 1);
            Some(World {
                cluster,
                live,
                _keys: keys,
            })
        }
    }
}

async fn call(
    instance: &Instance,
    browser: &Browser,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> Value {
    let (status, value) = instance
        .send(browser.request(method, path, key, body))
        .await;
    assert!(status.is_success(), "{method} {path}: {status} {value}");
    value
}

/// A project with a channel, made by `browser` in `workspace`.
async fn channel(world: &World, browser: &Browser, workspace: Uuid, key: &str) -> (Uuid, Uuid) {
    let project = call(
        world.api(),
        browser,
        "POST",
        "/api/v1/projects",
        Some(&format!("{key}-project")),
        json!({"organizationId": workspace, "key": key.to_ascii_uppercase(), "name": key}),
    )
    .await;
    let project: Uuid = project["id"]
        .as_str()
        .expect("project")
        .parse()
        .expect("id");
    let channel = call(
        world.api(),
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/channels"),
        Some(&format!("{key}-channel")),
        json!({"name": "General"}),
    )
    .await;
    let channel = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .parse()
        .expect("id");
    (project, channel)
}

async fn say(world: &World, browser: &Browser, conversation: Uuid, key: &str) -> Uuid {
    let sent = call(
        world.api(),
        browser,
        "POST",
        &format!("/api/v1/conversations/{conversation}/messages"),
        Some(key),
        json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Hello"}]}}),
    )
    .await;
    sent["message"]["id"]
        .as_str()
        .expect("message")
        .parse()
        .expect("id")
}

/// Someone else in `workspace` and `project`, signed in.
async fn person(world: &World, workspace: Uuid, project: Uuid, login: &str) -> (Uuid, Browser) {
    let pool = &world.cluster.pool;
    let actor = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor)
        .execute(pool)
        .await
        .expect("actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) \
         VALUES ($1, $2, 'member')",
    )
    .bind(workspace)
    .bind(actor)
    .execute(pool)
    .await
    .expect("workspace membership");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
         VALUES ($1, $2, $3, 'member', $4, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(project)
    .bind(actor)
    .bind(login)
    .execute(pool)
    .await
    .expect("project membership");
    bootstrap_development_member(
        pool,
        BootstrapCommand {
            login_name: login.to_owned(),
            display_name: login.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor),
            organization_id: Some(workspace),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("person");
    (actor, world.api().login(login).await)
}

async fn ticket(world: &World, browser: &Browser) -> Value {
    call(
        world.api(),
        browser,
        "GET",
        "/api/v2/live/ticket",
        None,
        Value::Null,
    )
    .await
}

/// A connection to the live side, having said `hello` with `ticket`.
async fn open(world: &World, ticket: &str) -> Socket {
    let url = format!("{}/live/v1/socket", world.live.replacen("http", "ws", 1));
    let mut socket = tokio_tungstenite::connect_async(url)
        .await
        .expect("socket")
        .0;
    socket
        .send(Message::Text(
            json!({"type": "hello", "ticket": ticket})
                .to_string()
                .into(),
        ))
        .await
        .expect("hello");
    socket
}

#[derive(Debug)]
enum Frame {
    Json(Value),
    Closed(Option<u16>),
    Nothing,
}

async fn next(socket: &mut Socket, within: Duration) -> Frame {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, socket.next()).await {
            Err(_) => return Frame::Nothing,
            Ok(Some(Ok(Message::Text(text)))) => {
                return Frame::Json(serde_json::from_str(&text).expect("frame"));
            }
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Frame::Closed(frame.map(|frame| u16::from(frame.code)));
            }
            Ok(None | Some(Err(_))) => return Frame::Closed(None),
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// Frames until one matches, skipping the rest; None when the connection
/// closes or nothing comes in time.
async fn until(
    socket: &mut Socket,
    within: Duration,
    mut wanted: impl FnMut(&Value) -> bool,
) -> Option<Value> {
    let deadline = Instant::now() + within;
    loop {
        match next(socket, deadline.saturating_duration_since(Instant::now())).await {
            Frame::Json(frame) if wanted(&frame) => return Some(frame),
            Frame::Json(_) => {}
            Frame::Closed(_) | Frame::Nothing => return None,
        }
    }
}

fn position(frame: &Value) -> i64 {
    frame["position"]
        .as_str()
        .expect("positions are strings")
        .parse()
        .expect("position")
}

fn message_in(frame: &Value, conversation: Uuid) -> bool {
    frame["type"] == "event"
        && frame["event"]["type"] == "conversation.message.created"
        && frame["event"]["conversationId"] == conversation.to_string()
}

const WAIT: Duration = Duration::from_secs(5);

async fn conformance(name: &str, target: Target) {
    let Some(world) = world(name, target).await else {
        return;
    };
    let api = world.api();
    let owner = api.login("owner").await;
    let workspace = world.cluster.organization_id;
    let (project, general) = channel(&world, &owner, workspace, "live").await;

    // Someone in another workspace.
    let other_workspace = second_workspace_owner(&world.cluster.pool).await;
    let other = api.login(&second_owner_login()).await;
    let (_, elsewhere) = channel(&world, &other, other_workspace, "away").await;

    // A ticket opens a connection; a changed or expired one doesn't.
    let issued = ticket(&world, &owner).await;
    let token = issued["ticket"].as_str().expect("ticket").to_owned();
    assert!(
        issued["url"]
            .as_str()
            .expect("url")
            .ends_with("/live/v1/socket")
    );
    let (payload, signature) = token.split_once('.').expect("ticket parts");
    let mut tampered = payload.as_bytes().to_vec();
    tampered[10] = if tampered[10] == b'A' { b'B' } else { b'A' };
    let tampered = format!(
        "{}.{signature}",
        String::from_utf8(tampered).expect("ascii")
    );
    let signer = Signer::from_config(&api.state().config).expect("signer");
    let expired = signer.sign(&Ticket {
        v: 1,
        kid: String::new(),
        account: Uuid::now_v7(),
        session: Uuid::now_v7(),
        kind: ConnectionKind::Web,
        audiences: [Audience::Workspace(workspace)].into(),
        from: 0,
        expires: chrono::Utc::now().timestamp() - 1,
        head: 0,
    });
    for refused in [tampered.as_str(), expired.as_str(), "nonsense"] {
        let mut socket = open(&world, refused).await;
        let refusal = until(&mut socket, WAIT, |frame| frame["type"] == "error").await;
        assert!(refusal.is_some(), "a bad ticket opened a connection");
        assert!(
            matches!(next(&mut socket, WAIT).await, Frame::Closed(Some(4401))),
            "closed with 4401"
        );
    }

    let mut socket = open(&world, &token).await;
    let ready = until(&mut socket, WAIT, |frame| frame["type"] == "ready")
        .await
        .expect("ready");
    // Anything published since the ticket came first.
    assert!(position(&ready) >= position(&issued), "{ready} {issued}");
    let other_ticket = ticket(&world, &other).await;
    let mut away = open(&world, other_ticket["ticket"].as_str().expect("ticket")).await;
    until(&mut away, WAIT, |frame| frame["type"] == "ready")
        .await
        .expect("ready");

    // Audiences: each hears its own workspace only; durable frames in
    // position order.
    for index in 0..3 {
        say(&world, &owner, general, &format!("live-{index}")).await;
    }
    say(&world, &other, elsewhere, "away-0").await;
    let mut last = position(&ready);
    for _ in 0..3 {
        let frame = until(&mut socket, WAIT, |frame| {
            assert!(
                !frame.to_string().contains(&elsewhere.to_string()),
                "heard another workspace: {frame}"
            );
            message_in(frame, general)
        })
        .await
        .expect("the owner's message");
        assert!(position(&frame) > last, "in position order");
        last = position(&frame);
    }
    until(&mut away, WAIT, |frame| {
        assert!(
            !frame.to_string().contains(&general.to_string()),
            "heard another workspace: {frame}"
        );
        message_in(frame, elsewhere)
    })
    .await
    .expect("its own workspace's message");

    // Resume through `api`: what was missed, once, then live from there.
    drop(socket);
    let missed = [
        say(&world, &owner, general, "live-missed-0").await,
        say(&world, &owner, general, "live-missed-1").await,
    ];
    // Missed means in the log: a message still being sequenced comes live.
    world.cluster.sequenced().await;
    let resumed = call(
        api,
        &owner,
        "GET",
        &format!("/api/v2/live/resume?after={last}"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(resumed["resync"], false, "{resumed}");
    let replayed = resumed["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|frame| frame["type"] == "event")
        .collect::<Vec<_>>();
    let mut positions = replayed
        .iter()
        .map(|frame| position(frame))
        .collect::<Vec<_>>();
    assert!(
        positions.iter().all(|position| *position > last),
        "no repeat"
    );
    let sorted = {
        let mut sorted = positions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted
    };
    assert_eq!(positions, sorted, "in order, once");
    for message in missed {
        assert!(
            replayed.iter().any(|frame| frame["event"]["payload"]
                .to_string()
                .contains(&message.to_string())
                || frame.to_string().contains(&message.to_string())),
            "the missed message {message} came back: {resumed}"
        );
    }
    let through = resumed["position"]
        .as_str()
        .expect("position")
        .parse::<i64>()
        .expect("position");
    positions.push(through);
    let mut socket = open(&world, resumed["ticket"].as_str().expect("ticket")).await;
    let ready = until(&mut socket, WAIT, |frame| frame["type"] == "ready")
        .await
        .expect("ready");
    assert!(position(&ready) >= through, "{ready}");
    let next_message = say(&world, &owner, general, "live-after").await;
    let frame = until(&mut socket, WAIT, |frame| message_in(frame, general))
        .await
        .expect("the next message");
    assert!(position(&frame) > through, "nothing replayed twice");
    assert!(
        frame.to_string().contains(&next_message.to_string()),
        "{frame}"
    );

    // `ticket.stale`: a private group made in the workspace; a new ticket
    // hears it, someone outside it never does.
    let (mara, mara_browser) = person(&world, workspace, project, "mara").await;
    let (_, rook) = person(&world, workspace, project, "rook").await;
    let rook_ticket = ticket(&world, &rook).await;
    let mut rook_socket = open(&world, rook_ticket["ticket"].as_str().expect("ticket")).await;
    until(&mut rook_socket, WAIT, |frame| frame["type"] == "ready")
        .await
        .expect("ready");
    // A person joins the workspace's direct space on their first visit.
    for browser in [&owner, &mara_browser] {
        call(api, browser, "GET", "/api/v1/direct", None, Value::Null).await;
    }
    let group = call(
        api,
        &owner,
        "POST",
        "/api/v1/direct/groups",
        Some("live-group"),
        json!({"memberIds": [mara]}),
    )
    .await;
    let group: Uuid = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .parse()
        .expect("id");
    until(&mut socket, WAIT, |frame| frame["type"] == "ticket.stale")
        .await
        .expect("the owner's ticket went stale");
    // Making a group changes access more than once; a ticket cut between
    // two of those changes goes stale again, and a client asks once more.
    let mut accepted = false;
    for _ in 0..5 {
        let refreshed = ticket(&world, &owner).await;
        socket
            .send(Message::Text(
                json!({"type": "ticket", "ticket": refreshed["ticket"]})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("new ticket");
        let answer = until(&mut socket, WAIT, |frame| {
            frame["type"] == "ticket.accepted" || frame["type"] == "ticket.stale"
        })
        .await
        .expect("an answer to the new ticket");
        if answer["type"] == "ticket.accepted" {
            accepted = true;
            break;
        }
    }
    assert!(accepted, "the new ticket is in use");
    say(&world, &owner, group, "live-group-0").await;
    say(&world, &owner, general, "live-general-after-group").await;
    until(&mut socket, WAIT, |frame| message_in(frame, group))
        .await
        .expect("the group's message, on the new ticket");
    until(&mut rook_socket, WAIT, |frame| {
        assert!(!message_in(frame, group), "Rook heard the group");
        message_in(frame, general)
    })
    .await
    .expect("the channel's message");

    // Signing out closes the session's connections.
    let kept = ticket(&world, &owner).await;
    let (status, _) = api
        .send(owner.request("POST", "/api/v1/auth/logout", None, Value::Null))
        .await;
    assert!(status.is_success(), "{status}");
    let closed = loop {
        match next(&mut socket, WAIT).await {
            Frame::Json(_) => {}
            other => break other,
        }
    };
    assert!(matches!(closed, Frame::Closed(Some(4401))), "{closed:?}");
    // A ticket cut before signing out opens nothing afterwards, on whichever
    // instance it lands: the sign-out is in the log, not one process.
    let mut late = open(&world, kept["ticket"].as_str().expect("ticket")).await;
    let refused = loop {
        match next(&mut late, WAIT).await {
            Frame::Json(frame) => assert_ne!(frame["type"], "event", "{frame}"),
            other => break other,
        }
    };
    assert!(
        matches!(refused, Frame::Closed(Some(4401 | 1012))),
        "{refused:?}"
    );
    world.cluster.finish().await;
}

#[tokio::test]
async fn the_in_process_live_role_meets_the_contract() {
    conformance("k5_19_in_process", Target::InProcess).await;
}

#[tokio::test]
async fn a_separate_live_instance_meets_the_contract() {
    conformance("k5_19_separate", Target::Separate).await;
}

/// The public key a live service checks tickets with, and nothing more.
#[tokio::test]
async fn the_ticket_key_is_public_and_tickets_need_a_session() {
    let Some(world) = world("k5_19_key", Target::InProcess).await else {
        return;
    };
    let api = world.api();
    let (status, keys) = api
        .send(
            axum::http::Request::builder()
                .uri("/api/v2/live/key")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{keys}");
    assert_eq!(keys["keys"][0]["alg"], "Ed25519");
    for path in ["/api/v2/live/ticket", "/api/v2/live/resume?after=0"] {
        let (status, _) = api
            .send(
                axum::http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    // Publishes are for a live side fed from outside; this one isn't.
    let (status, _) = api
        .send(
            axum::http::Request::builder()
                .method("POST")
                .uri("/live/v1/publish")
                .body(axum::body::Body::from("{}"))
                .expect("request"),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    world.cluster.finish().await;
}

/// K5.21: a document's room. A watcher whose ticket names the room hears
/// its changes; a connection whose ticket lacks it, or someone who may not
/// see the document, hears nothing; once access is gone, `ticket.stale`
/// and a new ticket drop the room.
async fn document_rooms(name: &str, target: Target) {
    use agent_workspace_server::realtime::{
        documents::{DocumentKind, DocumentRef},
        ephemeral::{EphemeralFrame, publish},
    };
    let Some(world) = world(name, target).await else {
        return;
    };
    let api = world.api();
    let owner = api.login("owner").await;
    let workspace = world.cluster.organization_id;
    let (project, _) = channel(&world, &owner, workspace, "rooms").await;
    let page = call(
        api,
        &owner,
        "POST",
        &format!("/api/v1/projects/{project}/pages"),
        None,
        json!({"title": "Plan", "body": "Hello"}),
    )
    .await;
    let page: Uuid = page["id"].as_str().expect("page").parse().expect("id");
    let document = DocumentRef {
        kind: DocumentKind::Page,
        id: page,
    };
    let watching = format!("documents=page.{page}");
    let (mara, mara_browser) = person(&world, workspace, project, "mara").await;
    second_workspace_owner(&world.cluster.pool).await;
    let outsider = api.login(&second_owner_login()).await;

    let watch = async |browser: &Browser| {
        call(
            api,
            browser,
            "GET",
            &format!("/api/v2/live/ticket?{watching}"),
            None,
            Value::Null,
        )
        .await
    };
    let mara_ticket = watch(&mara_browser).await;
    assert_eq!(mara_ticket["documents"], json!([format!("page.{page}")]));
    let outsider_ticket = watch(&outsider).await;
    assert_eq!(outsider_ticket["documents"], json!([]), "may not see it");
    let owner_ticket = ticket(&world, &owner).await;
    let mut sockets = Vec::new();
    for issued in [&mara_ticket, &outsider_ticket, &owner_ticket] {
        let mut socket = open(&world, issued["ticket"].as_str().expect("ticket")).await;
        until(&mut socket, WAIT, |frame| frame["type"] == "ready")
            .await
            .expect("ready");
        sockets.push(socket);
    }
    let change = |version: i64| EphemeralFrame::DocumentUpdate {
        document,
        generation: Uuid::now_v7(),
        version,
        update: "AQLy".to_owned(),
    };
    let is_update = |frame: &Value| frame["kind"] == "document.update";
    publish(api.state(), change(1)).await;
    let heard = until(&mut sockets[0], WAIT, is_update)
        .await
        .expect("the watcher hears");
    assert_eq!(heard["document"]["id"], page.to_string());
    for socket in &mut sockets[1..] {
        assert!(
            until(socket, Duration::from_secs(1), is_update)
                .await
                .is_none(),
            "only the room hears"
        );
    }

    // Access gone: the ticket goes stale, and a new one has no room.
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(mara)
    .fetch_one(&world.cluster.pool)
    .await
    .expect("version");
    call(
        api,
        &owner,
        "PATCH",
        &format!("/api/v1/projects/{project}/members/{mara}"),
        Some("rooms-disable"),
        json!({"expectedVersion": version, "enabled": false}),
    )
    .await;
    until(&mut sockets[0], WAIT, |frame| {
        frame["type"] == "ticket.stale"
    })
    .await
    .expect("Mara's ticket went stale");
    // Until a new ticket is in, the old one carries nothing that can hold
    // content (a document's changes, streamed text): it may name what Mara
    // can no longer see.
    publish(api.state(), change(2)).await;
    assert!(
        until(&mut sockets[0], Duration::from_secs(1), is_update)
            .await
            .is_none(),
        "a stale ticket hears no content"
    );
    // Sending back the ticket it already had doesn't lift that: it still
    // verifies, but it was cut before access changed.
    sockets[0]
        .send(Message::Text(
            json!({"type": "ticket", "ticket": mara_ticket["ticket"]})
                .to_string()
                .into(),
        ))
        .await
        .expect("old ticket");
    publish(api.state(), change(2)).await;
    assert!(
        until(&mut sockets[0], Duration::from_secs(1), |frame| {
            is_update(frame) || frame["type"] == "ticket.accepted"
        })
        .await
        .is_none(),
        "the old ticket is not taken back"
    );
    // Nor on a new connection: it opens stale and hears no content.
    let mut reopened = open(&world, mara_ticket["ticket"].as_str().expect("ticket")).await;
    until(&mut reopened, WAIT, |frame| frame["type"] == "ticket.stale")
        .await
        .expect("a new connection with the old ticket opens stale");
    publish(api.state(), change(2)).await;
    assert!(
        until(&mut reopened, Duration::from_secs(1), is_update)
            .await
            .is_none(),
        "the old ticket on a new connection hears no content"
    );
    let fresh = watch(&mara_browser).await;
    assert_eq!(fresh["documents"], json!([]), "the room is dropped");
    sockets[0]
        .send(Message::Text(
            json!({"type": "ticket", "ticket": fresh["ticket"]})
                .to_string()
                .into(),
        ))
        .await
        .expect("new ticket");
    until(&mut sockets[0], WAIT, |frame| {
        frame["type"] == "ticket.accepted"
    })
    .await
    .expect("the new ticket is in use");
    publish(api.state(), change(3)).await;
    assert!(
        until(&mut sockets[0], Duration::from_secs(1), is_update)
            .await
            .is_none(),
        "no longer in the room"
    );
    world.cluster.finish().await;
}

#[tokio::test]
async fn the_in_process_live_role_keeps_document_rooms() {
    document_rooms("k5_21_in_process", Target::InProcess).await;
}

#[tokio::test]
async fn a_separate_live_instance_keeps_document_rooms() {
    document_rooms("k5_21_separate", Target::Separate).await;
}

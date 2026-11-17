//! W18: Approve, Deny and Reply straight from a push notification. The
//! costly failures are silent: a token acting for someone after they lost
//! access, a token doing something its notification didn't offer, an
//! approval answered twice (Approve then Deny), a retried reply posted
//! twice, and an expired or guessed token doing anything at all.

use agent_workspace_server::{notifications, push};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    mcp_admin::Setup,
    mcp_code::{run, seeded, thread},
    message_edits::{add_person, say, set_role},
    notifications::drain,
};

/// What the service worker sends: no cookie, no CSRF token, just the token.
async fn act(setup: &Setup, body: Value) -> (StatusCode, Value) {
    super::agent_interface::send(
        &setup.app,
        Request::builder()
            .method("POST")
            .uri("/api/v1/notification-actions")
            .header("origin", "http://127.0.0.1:3100")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).expect("body")))
            .expect("request"),
    )
    .await
}

async fn mint(
    pool: &PgPool,
    organization: Uuid,
    recipient: Uuid,
    notification: Uuid,
) -> (push::actions::MintedActions, Value) {
    let view = notifications::get_many(
        pool,
        agent_workspace_server::addresses::Addresses::LEGACY,
        recipient,
        &[notification],
    )
    .await
    .expect("notification")
    .into_iter()
    .next()
    .expect("visible notification");
    let minted = push::actions::mint(pool, organization, recipient, &view)
        .await
        .expect("mint")
        .expect("the notification offers actions");
    let payload = push::payload_with_actions(&view, Some(&minted));
    (minted, payload)
}

async fn messages_by(pool: &PgPool, author: Uuid, text: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE author_actor_id = $1 AND plain_text = $2",
    )
    .bind(author)
    .bind(text)
    .fetch_one(pool)
    .await
    .expect("messages")
}

#[tokio::test]
async fn reply_from_a_mention_posts_once_and_stops_with_access() {
    let Some((setup, seed, _temporary)) = seeded("w18_reply").await else {
        return;
    };
    let pool = &setup.pool;
    let project = seed.project_id.to_string();
    let (dana, _dana_browser) = add_person(&setup, &[&project], "dana").await;
    say(
        &setup,
        &setup.browser,
        &seed.channel_id.to_string(),
        "mention-dana",
        json!({"version": 1, "nodes": [
            {"type": "mention", "actorId": dana},
            {"type": "text", "text": " can you check the build?"}
        ]}),
    )
    .await;
    drain(&setup).await;
    let notification = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM notifications WHERE recipient_actor_id = $1 AND kind = 'mention'",
    )
    .bind(dana)
    .fetch_one(pool)
    .await
    .expect("Dana's mention");

    let (minted, payload) = mint(pool, seed.organization_id, dana, notification).await;
    assert_eq!(payload["actionToken"], minted.token.as_str());
    assert_eq!(payload["actions"][0]["action"], "reply", "{payload}");
    assert_eq!(payload["actions"][0]["type"], "text", "{payload}");
    // Only the token's hash is stored.
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notification_action_tokens WHERE encode(token_hash, 'escape') LIKE '%' || $1 || '%'",
    )
    .bind(&minted.token)
    .fetch_one(pool)
    .await
    .expect("tokens");
    assert_eq!(stored, 0);

    // A mention offers Reply only, and a guessed token does nothing.
    let (status, body) = act(&setup, json!({"token": minted.token, "action": "approve"})).await;
    assert!(status.is_client_error(), "{status} {body}");
    let (status, _) = act(
        &setup,
        json!({"token": "bm90LWEtcmVhbC10b2tlbg", "action": "reply", "text": "hi"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Reply posts as Dana, once, however often the phone retries.
    for _ in 0..2 {
        let (status, body) = act(
            &setup,
            json!({"token": minted.token, "action": "reply", "text": " On it "}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["outcome"], "replied", "{body}");
    }
    assert_eq!(messages_by(pool, dana, "On it").await, 1);
    let read = sqlx::query_scalar::<_, bool>(
        "SELECT read_at IS NOT NULL FROM notifications WHERE id = $1",
    )
    .bind(notification)
    .fetch_one(pool)
    .await
    .expect("read");
    assert!(read, "acting on a notification reads it");

    // Losing the project stops a token minted while Dana had it.
    let (second, _) = mint(pool, seed.organization_id, dana, notification).await;
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(dana)
    .execute(pool)
    .await
    .expect("remove Dana");
    let (status, body) = act(
        &setup,
        json!({"token": second.token, "action": "reply", "text": "Still here?"}),
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");
    assert_eq!(messages_by(pool, dana, "Still here?").await, 0);

    // An expired token does nothing either.
    sqlx::query(
        "UPDATE project_memberships SET enabled = true WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(dana)
    .execute(pool)
    .await
    .expect("restore Dana");
    sqlx::query("UPDATE notification_action_tokens SET expires_at = now() - interval '1 minute'")
        .execute(pool)
        .await
        .expect("expire");
    let (status, _) = act(
        &setup,
        json!({"token": second.token, "action": "reply", "text": "Late"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(messages_by(pool, dana, "Late").await, 0);
}

#[tokio::test]
async fn approve_from_the_notification_answers_once_as_the_person() {
    let Some((setup, seed, _temporary)) = seeded("w18_approve").await else {
        return;
    };
    let pool = &setup.pool;
    let project = seed.project_id.to_string();
    let (dana, _dana_browser) = add_person(&setup, &[&project], "dana").await;
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(pool)
    .await
    .expect("repository ready");
    let asking = thread(pool, &seed, seed.channel_id, "Install packages").await;
    let (blocked, _) = run(pool, &seed, asking, "install-packages").await;
    for next in ["starting", "working", "blocked"] {
        sqlx::query("UPDATE agent_runs SET state = $2 WHERE id = $1")
            .bind(blocked)
            .bind(next)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("run to {next}: {error}"));
    }
    // Approvals go to the space's owners; Dana is one, so the request
    // puts a card in her inbox.
    set_role(pool, &project, dana, "owner").await;
    let approval =
        super::support::requests::approval(pool, blocked, "w18-install", "Run npm install?").await;
    let notification = card(pool, dana, approval).await;

    let (minted, payload) = mint(pool, seed.organization_id, dana, notification).await;
    let offered = payload["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .map(|action| action["action"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(&offered[..2], ["approve", "deny"], "{payload}");

    let (status, body) = act(&setup, json!({"token": minted.token, "action": "approve"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "approved", "{body}");
    let (state, responder, answer) = answered(pool, approval).await;
    assert_eq!(state, "answered");
    assert_eq!(responder, Some(dana));
    assert_eq!(answer, Some(json!({"optionId": "approve"})));
    let run_state = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(blocked)
        .fetch_one(pool)
        .await
        .expect("run");
    assert_eq!(run_state, "working");

    // A retry of Approve is fine; Deny after Approve is refused.
    let (status, body) = act(&setup, json!({"token": minted.token, "action": "approve"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = act(&setup, json!({"token": minted.token, "action": "deny"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (_, _, still) = answered(pool, approval).await;
    assert_eq!(still, Some(json!({"optionId": "approve"})));

    // An answered approval offers nothing more.
    let view = notifications::get_many(
        pool,
        agent_workspace_server::addresses::Addresses::LEGACY,
        dana,
        &[notification],
    )
    .await
    .expect("notification")
    .into_iter()
    .next()
    .expect("visible");
    assert!(push::actions::offered(&view).is_empty());

    // Should the card come to show the run's next approval, the old push's
    // buttons must not answer it.
    let next =
        super::support::requests::approval(pool, blocked, "w18-force", "Force-push main?").await;
    sqlx::query("UPDATE notifications SET subject_id = $2 WHERE id = $1")
        .bind(notification)
        .bind(next)
        .execute(pool)
        .await
        .expect("regroup");
    let (status, body) = act(&setup, json!({"token": minted.token, "action": "approve"})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (next_state, _, _) = answered(pool, next).await;
    assert_eq!(next_state, "open");
}

/// The inbox card a request put in front of `recipient`.
async fn card(pool: &PgPool, recipient: Uuid, request: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM notifications WHERE recipient_actor_id = $1 AND subject_kind = 'request' AND subject_id = $2",
    )
    .bind(recipient)
    .bind(request)
    .fetch_one(pool)
    .await
    .expect("the request's card")
}

/// A request's status, who answered and the answer.
async fn answered(pool: &PgPool, request: Uuid) -> (String, Option<Uuid>, Option<Value>) {
    sqlx::query_as::<_, (String, Option<Uuid>, Option<Value>)>(
        "SELECT status, answered_by_account_id, answer FROM requests WHERE id = $1",
    )
    .bind(request)
    .fetch_one(pool)
    .await
    .expect("request")
}

#[tokio::test]
async fn a_request_is_answered_from_its_notification_once_as_the_person() {
    use super::{
        agent_interface::{create_agent, initialize, tool},
        mcp_admin::{create_project, setup},
        mcp_tasks::{add_member, space},
    };
    let Some(setup) = setup("w18_request_push").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let project = create_project(&setup, "PSH", "Push").await;
    let (mara, _mara_browser) = add_person(&setup, &[&project], "mara").await;
    let space = space(&setup).await;
    let (scout, scout_token) = create_agent(app, &setup.browser, &space, "Scout", "scout").await;
    add_member(&setup, &project, scout, "member").await;
    let session = initialize(app, &scout_token).await.expect("Scout MCP");
    let ask = |title: &str, extra: Value| {
        let mut arguments = json!({
            "spaceId": project,
            "title": title,
            "audience": {"accounts": ["@mara"]},
        });
        arguments
            .as_object_mut()
            .expect("arguments")
            .extend(extra.as_object().expect("extra").clone());
        let token = scout_token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, "request_create", arguments).await }
    };
    let setup = &setup;
    let notification_for = |request: String| async move {
        drain(setup).await;
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM notifications WHERE recipient_actor_id = $1 AND subject_id = $2::uuid",
        )
        .bind(mara)
        .bind(request)
        .fetch_one(pool)
        .await
        .expect("request notification")
    };

    // Yes and no: Approve and Deny, answered as Mara through request_respond.
    let asked = ask(
        "Ship the colour change?",
        json!({"options": [
            {"id": "ship", "label": "Ship it", "style": "approve"},
            {"id": "hold", "label": "Hold", "style": "deny"},
        ]}),
    )
    .await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    let request = asked["value"]["id"].as_str().expect("request").to_owned();
    let notification = notification_for(request.clone()).await;
    let (minted, payload) = mint(pool, setup.organization_id, mara, notification).await;
    let offered = payload["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .map(|action| action["action"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(&offered[..2], ["approve", "deny"], "{payload}");
    let (status, body) = act(setup, json!({"token": minted.token, "action": "approve"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "approved", "{body}");
    let (state, answer, by) = sqlx::query_as::<_, (String, Option<Value>, Option<Uuid>)>(
        "SELECT status, answer, answered_by_account_id FROM requests WHERE id = $1::uuid",
    )
    .bind(&request)
    .fetch_one(pool)
    .await
    .expect("request");
    assert_eq!(state, "answered");
    assert_eq!(by, Some(mara));
    assert_eq!(
        answer
            .as_ref()
            .and_then(|answer| answer["optionId"].as_str()),
        Some("ship"),
        "{answer:?}"
    );
    // A retry is fine; Deny after Approve is refused and changes nothing.
    let (status, body) = act(setup, json!({"token": minted.token, "action": "approve"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = act(setup, json!({"token": minted.token, "action": "deny"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // A typed answer: Reply, and the text is the answer.
    let asked = ask("What should the banner say?", json!({"allowText": true})).await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    let request = asked["value"]["id"].as_str().expect("request").to_owned();
    let notification = notification_for(request.clone()).await;
    let (minted, payload) = mint(pool, setup.organization_id, mara, notification).await;
    assert_eq!(payload["actions"][0]["action"], "reply", "{payload}");
    let (status, body) = act(setup, json!({"token": minted.token, "action": "approve"})).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    let (status, body) = act(
        setup,
        json!({"token": minted.token, "action": "reply", "text": "Back soon"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answer =
        sqlx::query_scalar::<_, Option<Value>>("SELECT answer FROM requests WHERE id = $1::uuid")
            .bind(&request)
            .fetch_one(pool)
            .await
            .expect("request");
    assert_eq!(
        answer.as_ref().and_then(|answer| answer["text"].as_str()),
        Some("Back soon"),
        "{answer:?}"
    );
}

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
    message_edits::{add_person, say},
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
    let view = notifications::get_many(pool, recipient, &[notification])
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
    // One that needs a person: the token acts as Dana, a person.
    let approval = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, eligible_role, human_required,
            kind, summary, options, provider_request_id
        ) VALUES ($1, $2, $3, 'member', true, 'approval', 'Run npm install?',
                  '[{"id":"approve","label":"Allow"},{"id":"deny","label":"Deny"}]'::jsonb,
                  'w18-' || $1::text)
        "#,
    )
    .bind(approval)
    .bind(blocked)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("approval");
    let notification = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO notifications (
            id, project_id, recipient_actor_id, kind, subject_kind, subject_id,
            conversation_id, agent_run_id, approval_id, group_key
        ) VALUES ($1, $2, $3, 'approval_requested', 'agent_run', $4, $5, $4, $6, $7)
        "#,
    )
    .bind(notification)
    .bind(seed.project_id)
    .bind(dana)
    .bind(blocked)
    .bind(asking)
    .bind(approval)
    .bind(format!("approval:{approval}"))
    .execute(pool)
    .await
    .expect("notification");

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
    let (state, responder, response) = sqlx::query_as::<_, (String, Option<Uuid>, Option<Value>)>(
        "SELECT status, responded_by_actor_id, response FROM agent_approvals WHERE id = $1",
    )
    .bind(approval)
    .fetch_one(pool)
    .await
    .expect("approval");
    assert_eq!(state, "resolved");
    assert_eq!(responder, Some(dana));
    assert_eq!(response, Some(json!({"decision": "approve"})));
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
    let still =
        sqlx::query_scalar::<_, Value>("SELECT response FROM agent_approvals WHERE id = $1")
            .bind(approval)
            .fetch_one(pool)
            .await
            .expect("approval");
    assert_eq!(still, json!({"decision": "approve"}));

    // An answered approval offers nothing more.
    let view = notifications::get_many(pool, dana, &[notification])
        .await
        .expect("notification")
        .into_iter()
        .next()
        .expect("visible");
    assert!(push::actions::offered(&view).is_empty());
}

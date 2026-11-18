//! C2.39: who may message whom. Someone who asks first gets a stranger's
//! first message as a request and nothing more until they accept; a block
//! refuses messages both ways, hides the DM from the blocker and keeps the
//! blocked account's mentions out of their inbox; a DM stays readable after
//! its two people stop sharing a workspace, but takes no new messages.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{app::send, callers::Person, world::WorldBuilder};

fn text(value: &str) -> Value {
    json!({ "version": 1, "nodes": [{ "type": "text", "text": value }] })
}

async fn say(
    app: &axum::Router,
    who: &Person,
    conversation: &str,
    words: &str,
) -> (StatusCode, Value) {
    send(
        app,
        who.browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": text(words)}),
        ),
    )
    .await
}

async fn op(app: &axum::Router, by: &Person, name: &str, input: Value) -> (StatusCode, Value) {
    send(
        app,
        by.browser.request(
            "POST",
            &format!("/api/v2/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await
}

async fn standing(app: &axum::Router, who: &Person, conversation: &str) -> Value {
    let (status, body) = op(
        app,
        who,
        "dm_standing_get",
        json!({"conversationId": conversation}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["value"].clone()
}

/// The DMs and groups in `who`'s list.
async fn listed(app: &axum::Router, who: &Person) -> Vec<String> {
    let (status, overview) = send(
        app,
        who.browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    overview["conversations"]
        .as_array()
        .expect("conversations")
        .iter()
        .map(|item| item["id"].as_str().expect("id").to_owned())
        .collect()
}

fn refused(status: StatusCode, body: &Value, why: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
}

#[tokio::test]
async fn requests_blocks_and_a_shared_workspace_decide_who_reaches_whom() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nina", "member")
        .dm("lena", "mara")
        .dm("nina", "mara")
        .dm("lena", "nina")
        .build("c2_39_dm_rules")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = world.project("WORK");
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let nina = &world.people["nina"];
    let lena_mara = world.conversations["lena+mara"].clone();
    let nina_mara = world.conversations["nina+mara"].clone();
    let lena_nina = world.conversations["lena+nina"].clone();

    // Mara asks people first. Lena's first message is a request; nothing
    // more goes until Mara accepts it.
    let (status, body) = op(app, mara, "profile_update", json!({"dmPolicy": "ask"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"]["dmPolicy"], "ask");
    let (status, body) = op(app, mara, "profile_update", json!({"dmPolicy": "never"})).await;
    assert!(!status.is_success(), "{body}");
    let (status, body) = say(app, lena, &lena_mara, "hello mara").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = say(app, lena, &lena_mara, "are you there?").await;
    refused(status, &body, "waiting");
    let lena_sees = standing(app, lena, &lena_mara).await;
    assert_eq!(lena_sees["canSend"], false, "{lena_sees}");
    assert_eq!(lena_sees["reason"], "waiting", "{lena_sees}");
    assert_eq!(lena_sees["request"]["direction"], "outgoing");
    let mara_sees = standing(app, mara, &lena_mara).await;
    assert_eq!(mara_sees["request"]["direction"], "incoming", "{mara_sees}");
    assert_eq!(mara_sees["request"]["state"], "pending");
    // Only the person asked answers it.
    let (status, body) = op(
        app,
        lena,
        "dm_request_respond",
        json!({"conversationId": lena_mara, "accept": true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = op(
        app,
        mara,
        "dm_request_respond",
        json!({"conversationId": lena_mara, "accept": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = say(app, lena, &lena_mara, "thanks").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Nina's request is declined: the DM leaves Mara's list, and Nina, who
    // isn't told, still can't send.
    let (status, body) = say(app, nina, &nina_mara, "hi").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(listed(app, mara).await.contains(&nina_mara));
    let (status, body) = op(
        app,
        mara,
        "dm_request_respond",
        json!({"conversationId": nina_mara, "accept": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!listed(app, mara).await.contains(&nina_mara));
    let nina_sees = standing(app, nina, &nina_mara).await;
    assert_eq!(nina_sees["request"]["state"], "pending", "{nina_sees}");
    let (status, body) = say(app, nina, &nina_mara, "hello?").await;
    refused(status, &body, "declined");

    // Lena blocks Nina: the DM leaves Lena's list, neither can send (in a
    // thread under the DM either), and Nina's mention of Lena in a channel
    // doesn't reach Lena's inbox.
    assert!(listed(app, lena).await.contains(&lena_nina));
    let (status, sent) = say(app, lena, &lena_nina, "a topic").await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let (status, made) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/conversations/{lena_nina}/threads"),
            Some("dm-rules-thread"),
            json!({"originMessageId": sent["message"]["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let thread = made["conversation"]["id"]
        .as_str()
        .or_else(|| made["id"].as_str())
        .expect("thread")
        .to_owned();
    let (status, body) = say(app, nina, &thread, "in the thread").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = op(
        app,
        lena,
        "sidebar_item_move",
        json!({"conversationId": lena_nina, "starred": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"]["sections"][0]["items"][0]["hidden"], false);
    let (status, body) = op(
        app,
        lena,
        "account_block_set",
        json!({"account": nina.actor, "blocked": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Starred, it keeps its place, marked hidden so it isn't shown.
    let (status, sidebar) = op(app, lena, "sidebar_list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{sidebar}");
    assert_eq!(
        sidebar["value"]["sections"][0]["items"][0]["hidden"], true,
        "{sidebar}"
    );
    assert_eq!(
        body["value"]["blocked"][0]["accountId"],
        json!(nina.actor),
        "{body}"
    );
    assert!(!listed(app, lena).await.contains(&lena_nina));
    let (status, body) = say(app, nina, &lena_nina, "hey").await;
    refused(status, &body, "blocked by them");
    let (status, body) = say(app, lena, &lena_nina, "hey").await;
    refused(status, &body, "blocked them");
    let (status, body) = say(app, nina, &thread, "still in the thread").await;
    refused(status, &body, "blocked, in a thread under the DM");
    assert_eq!(standing(app, lena, &lena_nina).await["reason"], "blocked");
    assert_eq!(standing(app, nina, &lena_nina).await["reason"], "closed");
    let (status, made) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("rules-channel"),
            json!({"name": "rules", "private": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let channel = made["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let (status, body) = op(app, nina, "channel_join", json!({"channelId": channel})).await;
    assert_eq!(status, StatusCode::OK, "join: {body}");
    let (status, body) = say(app, nina, &channel, "@lena look at this").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let from_nina = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND actor_id = $2",
    )
    .bind(lena.actor)
    .bind(nina.actor)
    .fetch_one(pool)
    .await
    .expect("notifications");
    assert_eq!(from_nina, 0);
    // Someone in no workspace of yours can't be blocked by id.
    let (status, body) = op(
        app,
        lena,
        "account_block_set",
        json!({"account": Uuid::now_v7(), "blocked": true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = op(
        app,
        lena,
        "account_block_set",
        json!({"account": nina.actor, "blocked": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"]["blocked"], json!([]), "{body}");
    assert!(listed(app, lena).await.contains(&lena_nina));
    let (status, body) = say(app, nina, &lena_nina, "friends again").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Lena and Mara stop sharing a workspace: the DM is still readable,
    // but takes no new messages.
    sqlx::query(
        "UPDATE organization_memberships SET enabled = false \
         WHERE actor_id = $1 AND organization_id = (SELECT organization_id FROM projects WHERE id = $2)",
    )
    .bind(mara.actor)
    .bind(work.parse::<Uuid>().expect("project id"))
    .execute(pool)
    .await
    .expect("disable");
    let (status, body) = say(app, lena, &lena_mara, "still there?").await;
    refused(status, &body, "not shared");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("share a workspace")),
        "{body}"
    );
    assert_eq!(
        standing(app, lena, &lena_mara).await["reason"],
        "not_shared"
    );
    let (status, history) = send(
        app,
        lena.browser.request(
            "GET",
            &format!("/api/v1/conversations/{lena_mara}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{history}");

    world.finish().await;
}

//! C2.13: keyword alerts. A message naming someone's keyword tells them
//! only where they can read it and haven't muted it, never of their own
//! message; a keyword is a whole word in any case; each person's list is
//! their own.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{Browser, send},
    callers::drain_outbox,
    world::WorldBuilder,
};

async fn op(
    app: &axum::Router,
    browser: &Browser,
    workspace: Uuid,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    let key = Uuid::now_v7().to_string();
    send(
        app,
        browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&key),
            input,
        ),
    )
    .await
}

#[tokio::test]
async fn keywords_reach_only_people_who_can_read_and_listen() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("OTHER")
        .person("lena", "member")
        .person("mara", "member")
        .person("nina", "member")
        .private_group("pair", &["lena", "mara"])
        .build("c2_13_keywords")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let nina = &world.people["nina"];
    let other = Uuid::parse_str(world.project("OTHER")).expect("other");
    // Nina is in the workspace but not in OTHER.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(other)
    .bind(nina.actor)
    .execute(pool)
    .await
    .expect("nina leaves OTHER");

    // Everyone listens for "launch"; the list is each person's own.
    for person in [lena, mara, nina] {
        let (status, set) = op(
            app,
            &person.browser,
            workspace,
            "notification_keyword_set",
            json!({"keywords": ["  Launch ", "launch", "design review"]}),
        )
        .await;
        assert!(status.is_success(), "{set}");
        assert_eq!(
            set["value"]["keywords"],
            json!(["Launch", "design review"]),
            "{set}"
        );
    }
    let (_, refused) = op(
        app,
        &lena.browser,
        workspace,
        "notification_keyword_set",
        json!({"keywords": ["x"]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let (_, listed) = op(
        app,
        &mara.browser,
        workspace,
        "notification_keyword_list",
        json!({}),
    )
    .await;
    assert_eq!(
        listed["value"]["keywords"],
        json!(["Launch", "design review"]),
        "{listed}"
    );

    let channel = |browser: &Browser, project: String, name: &'static str| {
        let request = browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some(&format!("keywords-{name}")),
            json!({ "name": name }),
        );
        async move {
            let (status, made) = send(app, request).await;
            assert!(status.is_success(), "{made}");
            made["conversation"]["id"]
                .as_str()
                .or_else(|| made["id"].as_str())
                .expect("channel")
                .to_owned()
        }
    };
    let post = |browser: &Browser, path: String, text: &'static str| {
        let request = browser.request(
            "POST",
            &path,
            Some(&Uuid::now_v7().to_string()),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": text}]}}),
        );
        async move {
            let (status, sent) = send(app, request).await;
            assert!(status.is_success(), "{sent}");
            Uuid::parse_str(
                sent["message"]["id"]
                    .as_str()
                    .or_else(|| sent["id"].as_str())
                    .expect("message"),
            )
            .expect("id")
        }
    };
    let told = |actor: Uuid, message: Uuid| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
             WHERE recipient_actor_id = $1 AND kind = 'keyword' AND message_id = $2",
        )
        .bind(actor)
        .bind(message)
        .fetch_one(pool)
        .await
        .expect("notifications")
    };

    let work = channel(
        &world.owner.browser,
        world.project("WORK").to_owned(),
        "general",
    )
    .await;
    let elsewhere = channel(&world.owner.browser, other.to_string(), "plans").await;

    // In a channel all three read: Mara and Nina hear, Lena wrote it.
    let first = post(
        &lena.browser,
        format!("/api/v1/conversations/{work}/messages"),
        "The LAUNCH moves to Friday",
    )
    .await;
    // Part of a word isn't the word.
    let partial = post(
        &lena.browser,
        format!("/api/v1/conversations/{work}/messages"),
        "We relaunched it",
    )
    .await;
    // Nina can't read OTHER.
    let hidden = post(
        &lena.browser,
        format!("/api/v1/conversations/{elsewhere}/messages"),
        "launch notes",
    )
    .await;
    // A thread in Lena and Mara's group is theirs alone.
    let (status, thread) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/channels/{}/threads", world.conversations["pair"]),
            Some("keywords-pair-thread"),
            json!({"name": "Plans"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{thread}");
    let thread = thread["conversation"]["id"]
        .as_str()
        .expect("thread")
        .to_owned();
    let private = post(
        &lena.browser,
        format!("/api/v1/threads/{thread}/messages"),
        "the design review is at 3",
    )
    .await;
    // Mara mutes the channel (after hearing of what came before).
    drain_outbox(&world.state).await;
    let (status, muted) = send(
        app,
        mara.browser.request(
            "PUT",
            &format!("/api/v1/conversations/{work}/notification-level"),
            Some("keywords-mute"),
            json!({"level": "nothing"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{muted}");
    let after_mute = post(
        &lena.browser,
        format!("/api/v1/conversations/{work}/messages"),
        "launch, again",
    )
    .await;
    drain_outbox(&world.state).await;
    assert_eq!(told(mara.actor, first).await, 1);
    assert_eq!(told(nina.actor, first).await, 1);
    assert_eq!(told(lena.actor, first).await, 0, "never your own message");
    assert_eq!(told(nina.actor, partial).await, 0, "a whole word only");
    assert_eq!(told(mara.actor, hidden).await, 1);
    assert_eq!(told(nina.actor, hidden).await, 0, "not in that project");
    assert_eq!(told(mara.actor, private).await, 1);
    assert_eq!(told(nina.actor, private).await, 0, "not in the group");
    assert_eq!(told(mara.actor, after_mute).await, 0, "muted");
    assert_eq!(told(nina.actor, after_mute).await, 1);
    // A notification names the message, never its words.
    let detail = sqlx::query_scalar::<_, Value>(
        "SELECT detail FROM notifications WHERE kind = 'keyword' AND message_id = $1 LIMIT 1",
    )
    .bind(first)
    .fetch_one(pool)
    .await
    .expect("detail");
    assert!(
        !detail.to_string().to_lowercase().contains("friday"),
        "{detail}"
    );

    world.finish().await;
}

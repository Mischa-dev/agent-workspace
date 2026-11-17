//! C2.1: lists never show a conversation to someone who can't read it. A
//! `members` channel nobody added them to, a thread in a DM and a thread in a
//! group stay out of a non-member's channel list, the sidebar of the
//! project, the sidebar of the space that holds DMs and groups, and search,
//! even though they are in every one of those projects.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{app::send, callers::Person, world::WorldBuilder};

fn text(value: &str) -> Value {
    json!({ "version": 1, "nodes": [{ "type": "text", "text": value }] })
}

async fn get(app: &axum::Router, person: &Person, path: &str) -> Value {
    let (status, body) = send(app, person.browser.request("GET", path, None, Value::Null)).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

#[tokio::test]
async fn non_members_see_no_private_conversation_in_any_list() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nina", "member")
        .dm("lena", "mara")
        .private_group("ops", &["lena", "mara"])
        .build("c2_1_privacy")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = world.project("WORK");
    let lena = &world.people["lena"];
    let nina = &world.people["nina"];

    // A private channel in the project, made by SQL: nobody is on it.
    let private = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name, visibility) VALUES ($1, $2::uuid, 'channel', 'private-ops', 'members')",
    )
    .bind(private)
    .bind(work)
    .execute(pool)
    .await
    .expect("private channel");

    // A thread in the DM and one in the group, each with a word to find.
    let mut hidden = vec![private.to_string()];
    let mut words = Vec::new();
    for (key, conversation) in [
        ("dm", &world.conversations["lena+mara"]),
        ("group", &world.conversations["ops"]),
    ] {
        let (status, thread) = send(
            app,
            lena.browser.request(
                "POST",
                &format!("/api/v1/conversations/{conversation}/threads"),
                Some(&format!("privacy-thread-{key}")),
                json!({"name": format!("{key} plans")}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{key}: {thread}");
        let thread = thread["conversation"]["id"]
            .as_str()
            .expect("thread")
            .to_owned();
        let word = format!("zq{key}{}", &Uuid::now_v7().simple().to_string()[20..]);
        let (status, sent) = send(
            app,
            lena.browser.request(
                "POST",
                &format!("/api/v1/conversations/{thread}/messages"),
                Some(&format!("privacy-message-{key}")),
                json!({"content": text(&format!("secret {word}"))}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{key}: {sent}");
        hidden.push(conversation.clone());
        hidden.push(thread);
        words.push(word);
    }
    let space =
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM conversations WHERE id = $1::uuid")
            .bind(&world.conversations["lena+mara"])
            .fetch_one(pool)
            .await
            .expect("space")
            .to_string();

    // A member finds the words: the searches below would see them.
    for word in &words {
        let found = get(app, lena, &format!("/api/v1/search?q={word}")).await;
        assert!(found.to_string().contains(word.as_str()), "{word}: {found}");
    }

    for project in [work, space.as_str()] {
        for path in [
            format!("/api/v1/projects/{project}/channels"),
            format!("/api/v1/projects/{project}/conversation-sidebar"),
            format!("/api/v1/projects/{project}/conversation-sidebar?includeArchived=true"),
        ] {
            let (status, listed) =
                send(app, nina.browser.request("GET", &path, None, Value::Null)).await;
            assert!(
                status == StatusCode::OK || status == StatusCode::NOT_FOUND,
                "{path}: {status} {listed}"
            );
            for id in &hidden {
                assert!(
                    !listed.to_string().contains(id.as_str()),
                    "{path} shows {id}: {listed}"
                );
            }
        }
    }
    for word in &words {
        let found = get(app, nina, &format!("/api/v1/search?q={word}")).await;
        assert!(
            !found.to_string().contains(word.as_str()),
            "{word}: {found}"
        );
        assert_eq!(found["hits"], json!([]), "{found}");
    }

    world.finish().await;
}

/// C2.6: a private channel is made private, its maker managing it. Only its
/// members find it, its messages or its timeline; a member who doesn't
/// manage it can't bring others in; whoever is removed loses it at once,
/// and anyone can leave.
#[tokio::test]
async fn private_channels_belong_to_their_members() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nina", "member")
        .build("c2_6_private_channels")
        .await
    else {
        return;
    };
    let app = &world.app;
    let work = world.project("WORK");
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let nina = &world.people["nina"];

    let (status, made) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("private-launch"),
            json!({"name": "launch", "private": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["conversation"]["private"], true, "{made}");
    let channel = made["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let word = format!("zqlaunch{}", &Uuid::now_v7().simple().to_string()[20..]);
    let (status, sent) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/conversations/{channel}/messages"),
            Some("private-launch-message"),
            json!({"content": text(&format!("launch date {word}"))}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");

    let sees = |person: &'static str| {
        let who = &world.people[person];
        let channel = channel.clone();
        let word = word.clone();
        async move {
            let listed = get(app, who, &format!("/api/v1/projects/{work}/channels")).await;
            let listed = listed.to_string().contains(channel.as_str());
            let (status, _) = send(
                app,
                who.browser.request(
                    "GET",
                    &format!("/api/v1/conversations/{channel}/timeline"),
                    None,
                    Value::Null,
                ),
            )
            .await;
            let found = get(app, who, &format!("/api/v1/search?q={word}")).await;
            let found = found.to_string().contains(word.as_str());
            assert_eq!(
                listed,
                status == StatusCode::OK,
                "{person}: list and timeline agree"
            );
            assert_eq!(listed, found, "{person}: list and search agree");
            listed
        }
    };
    assert!(sees("lena").await);
    assert!(!sees("mara").await);
    assert!(!sees("nina").await);

    let add = |by: &Person, who: Uuid, key: &str| {
        by.browser.request(
            "POST",
            &format!("/api/v1/conversations/{channel}/members"),
            Some(key),
            json!({"actorId": who}),
        )
    };
    let remove = |by: &Person, who: Uuid| {
        by.browser.request(
            "POST",
            &format!("/api/v1/conversations/{channel}/members/{who}/remove"),
            None,
            Value::Null,
        )
    };
    // Someone outside it can't add themselves.
    let (status, refused) = send(app, add(nina, nina.actor, "nina-self")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    let (status, added) = send(app, add(lena, mara.actor, "add-mara")).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert!(sees("mara").await);
    // Mara is in it but doesn't manage it.
    let (status, refused) = send(app, add(mara, nina.actor, "mara-adds-nina")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(!sees("nina").await);
    let (status, added) = send(app, add(lena, nina.actor, "add-nina")).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert!(sees("nina").await);
    let (status, removed) = send(app, remove(lena, nina.actor)).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert!(!sees("nina").await);
    // Mara leaves on her own.
    let (status, left) = send(app, remove(mara, mara.actor)).await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert!(!sees("mara").await);
    assert!(sees("lena").await);

    world.finish().await;
}

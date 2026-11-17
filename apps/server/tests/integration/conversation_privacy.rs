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

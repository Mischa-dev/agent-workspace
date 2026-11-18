//! C2.1: lists never show a conversation to someone who can't read it. A
//! `members` channel nobody added them to, a thread in a DM and a thread in a
//! group stay out of a non-member's channel list, the sidebar of the
//! project, the sidebar of the space that holds DMs and groups, and search,
//! even though they are in every one of those projects.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller, Person},
    world::WorldBuilder,
};

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
    // C2.23: a link card reads the thing with the reader's access, so a
    // link to any of them draws nothing for Nina, and the group's own
    // members still get theirs.
    for id in &hidden {
        for kind in ["conversation", "thread", "channel"] {
            let path = format!("/api/v1/things/{kind}/{id}");
            let (status, card) =
                send(app, nina.browser.request("GET", &path, None, Value::Null)).await;
            assert_ne!(status, StatusCode::OK, "{path}: {card}");
            assert!(!card.to_string().contains("plans"), "{path}: {card}");
        }
    }
    let group_thread = &hidden[4];
    let card = get(app, lena, &format!("/api/v1/things/thread/{group_thread}")).await;
    assert_eq!(card["thing"]["title"], "group plans", "{card}");

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
    let members = |by: &Person| {
        by.browser.request(
            "POST",
            "/api/v2/ops/conversation_member_list",
            None,
            json!({"conversationId": channel}),
        )
    };
    let (status, listed) = send(app, members(lena)).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["value"]["canManage"], true, "{listed}");
    assert_eq!(listed["value"]["members"][0]["manager"], true, "{listed}");
    assert_eq!(
        listed["value"]["members"].as_array().map(Vec::len),
        Some(2),
        "{listed}"
    );
    let (status, listed) = send(app, members(mara)).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["value"]["canManage"], false, "{listed}");
    // Someone outside it learns nothing about who's in it.
    let (status, refused) = send(app, members(nina)).await;
    assert_ne!(status, StatusCode::OK, "{refused}");
    assert!(
        !refused.to_string().contains(&lena.actor.to_string()),
        "{refused}"
    );
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

#[tokio::test]
async fn channels_are_joined_left_and_browsed() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .build("c2_7_joining_channels")
        .await
    else {
        return;
    };
    let app = &world.app;
    let work = world.project("WORK");
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let op = |by: &Person, name: &str, input: Value| {
        let key = Uuid::now_v7().to_string();
        by.browser
            .request("POST", &format!("/api/v2/ops/{name}"), Some(&key), input)
    };
    let create = |by: &Person, name: &str, auto_join: bool| {
        by.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some(&format!("c2-7-{name}")),
            json!({"name": name, "autoJoin": auto_join}),
        )
    };

    let (status, lobby) = send(app, create(lena, "lobby", true)).await;
    assert_eq!(status, StatusCode::CREATED, "{lobby}");
    let lobby = lobby["conversation"]["id"]
        .as_str()
        .expect("lobby")
        .to_owned();
    let (status, quiet) = send(app, create(lena, "quiet", false)).await;
    assert_eq!(status, StatusCode::CREATED, "{quiet}");
    let quiet = quiet["conversation"]["id"]
        .as_str()
        .expect("quiet")
        .to_owned();

    // Everyone joins an auto-join channel; only its maker joins the other.
    assert!(in_sidebar(app, mara, work, &lobby).await);
    assert!(!in_sidebar(app, mara, work, &quiet).await);
    // Still found, read and joined.
    let (status, listed) = send(app, op(mara, "channel_list", json!({"spaceId": work}))).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let quiet_listed = listed["value"]["channels"]
        .as_array()
        .expect("channels")
        .iter()
        .find(|channel| channel["id"] == quiet.as_str())
        .cloned()
        .expect("quiet is listed");
    assert_eq!(quiet_listed["joined"], false, "{listed}");
    let (status, joined) = send(app, op(mara, "channel_join", json!({"channelId": quiet}))).await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["value"]["joined"], true, "{joined}");
    assert_eq!(joined["value"]["memberCount"], 2, "{joined}");
    assert!(in_sidebar(app, mara, work, &quiet).await);

    // Leaving takes it out of the sidebar, but its messages are still hers
    // to find.
    let (status, left) = send(app, op(mara, "channel_leave", json!({"channelId": lobby}))).await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert_eq!(left["value"]["stillVisible"], true, "{left}");
    assert!(!in_sidebar(app, mara, work, &lobby).await);
    let word = format!("zqlobby{}", &Uuid::now_v7().simple().to_string()[20..]);
    let (status, sent) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/conversations/{lobby}/messages"),
            Some("c2-7-lobby-message"),
            json!({"content": text(&format!("in the lobby {word}"))}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let found = get(app, mara, &format!("/api/v1/search?q={word}")).await;
    assert!(found.to_string().contains(word.as_str()), "{found}");

    // Anyone posting there sets the topic; who joins it is for its managers.
    let (status, topic) = send(
        app,
        op(
            mara,
            "channel_update",
            json!({"channelId": quiet, "topic": "Calm things"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{topic}");
    assert_eq!(topic["value"]["topic"], "Calm things", "{topic}");
    let (status, refused) = send(
        app,
        op(
            mara,
            "channel_update",
            json!({"channelId": quiet, "autoJoin": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, opened) = send(
        app,
        op(
            lena,
            "channel_update",
            json!({"channelId": quiet, "autoJoin": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["value"]["autoJoin"], true, "{opened}");

    // Someone arriving in a space joins its auto-join channels only.
    let (status, side) = send(
        app,
        world.owner.browser.request(
            "POST",
            "/api/v1/projects",
            Some("c2-7-side"),
            json!({"organizationId": world.organization, "key": "SIDE", "name": "Side"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{side}");
    let side = side["id"].as_str().expect("side").to_owned();
    let (status, open) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{side}/channels"),
            Some("c2-7-side-open"),
            json!({"name": "open"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{open}");
    let open = open["conversation"]["id"]
        .as_str()
        .expect("open")
        .to_owned();
    let (status, closed) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{side}/channels"),
            Some("c2-7-side-closed"),
            json!({"name": "closed", "autoJoin": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{closed}");
    let closed = closed["conversation"]["id"]
        .as_str()
        .expect("closed")
        .to_owned();
    let (status, added) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{side}/members"),
            None,
            json!({"actorId": mara.actor, "role": "member"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{added}");
    assert!(in_sidebar(app, mara, &side, &open).await);
    assert!(!in_sidebar(app, mara, &side, &closed).await);

    world.finish().await;
}

async fn in_sidebar(app: &axum::Router, who: &Person, project: &str, channel: &str) -> bool {
    get(
        app,
        who,
        &format!("/api/v1/projects/{project}/conversation-sidebar"),
    )
    .await
    .to_string()
    .contains(channel)
}

/// C2.9: sidebar sections are each account's own. Losing access hides an
/// item and keeps its row, and an agent arranges its own as people do.
#[tokio::test]
async fn sidebar_sections_keep_to_what_you_can_see() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .dm("lena", "mara")
        .private_group("ops", &["lena", "mara"])
        .build("c2_9_sidebar_sections")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let rook = &world.agents["rook"];
    let dm = world.conversations["lena+mara"].clone();
    let ops = world.conversations["ops"].clone();
    let op = |by: &Person, name: &str, input: Value| {
        let key = Uuid::now_v7().to_string();
        by.browser
            .request("POST", &format!("/api/v2/ops/{name}"), Some(&key), input)
    };
    let section = |sidebar: &Value, name: &str| {
        sidebar["value"]["sections"]
            .as_array()
            .expect("sections")
            .iter()
            .find(|section| section["name"] == name)
            .cloned()
    };
    let items = |section: &Value| {
        section["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["conversationId"].as_str().expect("id").to_owned())
            .collect::<Vec<_>>()
    };

    // Lena names a section, puts the group in it and stars the DM.
    let (status, made) = send(
        app,
        op(lena, "sidebar_section_set", json!({"name": "Launch"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let launch = section(&made, "Launch").expect("Launch")["id"].clone();
    let (status, moved) = send(
        app,
        op(
            lena,
            "sidebar_item_move",
            json!({"conversationId": ops, "sectionId": launch}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let (status, starred) = send(
        app,
        op(
            lena,
            "sidebar_item_move",
            json!({"conversationId": dm, "starred": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{starred}");
    let sections = starred["value"]["sections"].as_array().expect("sections");
    assert_eq!(sections[0]["kind"], "starred", "{starred}");
    assert_eq!(items(&sections[0]), vec![dm.clone()]);
    assert_eq!(
        items(&section(&starred, "Launch").expect("Launch")),
        vec![ops.clone()]
    );
    // The same name twice is refused, whatever its case.
    let (status, twice) = send(
        app,
        op(lena, "sidebar_section_set", json!({"name": "launch"})),
    )
    .await;
    assert!(!status.is_success(), "{twice}");

    // They're Lena's own: Mara has none, and can't use Lena's section.
    let (status, theirs) = send(app, op(mara, "sidebar_list", json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    assert_eq!(theirs["value"]["sections"], json!([]), "{theirs}");
    let (status, refused) = send(
        app,
        op(
            mara,
            "sidebar_item_move",
            json!({"conversationId": ops, "sectionId": launch}),
        ),
    )
    .await;
    assert!(!status.is_success(), "{refused}");

    // Lena leaves the group: it drops out of her section, and its row stays
    // for when she's back.
    sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND actor_id = $2")
        .bind(Uuid::parse_str(&ops).expect("ops"))
        .bind(lena.actor)
        .execute(pool)
        .await
        .expect("lena leaves");
    let (status, after) = send(app, op(lena, "sidebar_list", json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert!(
        items(&section(&after, "Launch").expect("Launch")).is_empty(),
        "{after}"
    );
    assert!(!after.to_string().contains(&ops), "{after}");
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sidebar_items WHERE account_id = $1 AND conversation_id = $2",
    )
    .bind(lena.actor)
    .bind(Uuid::parse_str(&ops).expect("ops"))
    .fetch_one(pool)
    .await
    .expect("kept");
    assert_eq!(kept, 1);
    // Nor can she put it back while she can't see it.
    let (status, hidden) = send(
        app,
        op(
            lena,
            "sidebar_item_move",
            json!({"conversationId": ops, "starred": true}),
        ),
    )
    .await;
    assert!(!status.is_success(), "{hidden}");

    // Removing a section puts what was in it back where it was.
    let starred_id = sections[0]["id"].clone();
    let (status, removed) = send(
        app,
        op(
            lena,
            "sidebar_section_set",
            json!({"sectionId": starred_id, "delete": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert!(section(&removed, "Starred").is_none(), "{removed}");

    // An agent stars a channel for itself, as a person does.
    let work = world.project("WORK");
    let (status, made) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("c2-9-lobby"),
            json!({"name": "lobby", "autoJoin": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let general = made["conversation"]["id"]
        .as_str()
        .expect("lobby")
        .to_owned();
    let (_, starred) = rook
        .call(
            app,
            Call::Tool {
                name: "sidebar_item_move",
                arguments: json!({"conversationId": general, "starred": true}),
            },
        )
        .await;
    assert_eq!(starred["outcome"], "ok", "{starred}");
    let (_, listed) = rook
        .call(
            app,
            Call::Tool {
                name: "sidebar_list",
                arguments: json!({}),
            },
        )
        .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(
        listed["value"]["sections"][0]["items"][0]["conversationId"],
        general.as_str(),
        "{listed}"
    );

    world.finish().await;
}

/// C2.18: `@design` notifies the group's members who can read the
/// conversation, and no one else; an archived group notifies nobody.
#[tokio::test]
async fn group_mentions_reach_only_members_who_can_read() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nina", "member")
        .person("omar", "member")
        .agent("rook", "member")
        .private_group("ops", &["lena", "mara"])
        .build("c2_18_group_mentions")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = world.project("WORK");
    let lena = &world.people["lena"];
    let ops = world.conversations["ops"].clone();
    let workspace =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1::uuid")
            .bind(work)
            .fetch_one(pool)
            .await
            .expect("workspace");

    // Design: Mara, Nina and an agent; Old: Omar, archived.
    let (design, old) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, handle, archived, members) in [
        (
            design,
            "design",
            false,
            vec![
                world.people["mara"].actor,
                world.people["nina"].actor,
                world.agents["rook"].actor,
            ],
        ),
        (old, "old-team", true, vec![world.people["omar"].actor]),
    ] {
        sqlx::query(
            "INSERT INTO account_groups (id, workspace_id, handle, name, archived_at) \
             VALUES ($1, $2, $3, $3, CASE WHEN $4 THEN now() END)",
        )
        .bind(id)
        .bind(workspace)
        .bind(handle)
        .bind(archived)
        .execute(pool)
        .await
        .expect("group");
        for member in members {
            sqlx::query(
                "INSERT INTO account_group_members (group_id, workspace_id, account_id) VALUES ($1, $2, $3)",
            )
            .bind(id)
            .bind(workspace)
            .bind(member)
            .execute(pool)
            .await
            .expect("member");
        }
    }
    let (status, made) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("c2-18-channel"),
            json!({"name": "studio"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let studio = made["conversation"]["id"]
        .as_str()
        .expect("studio")
        .to_owned();

    let mentioned = |conversation: String| async move {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT recipient_actor_id FROM notifications \
             WHERE kind = 'mention' AND conversation_id = $1::uuid ORDER BY recipient_actor_id",
        )
        .bind(&conversation)
        .fetch_all(pool)
        .await
        .expect("mentions")
    };
    for (conversation, key) in [(&ops, "ops"), (&studio, "studio")] {
        let (status, sent) = send(
            app,
            lena.browser.request(
                "POST",
                &format!("/api/v1/conversations/{conversation}/messages"),
                Some(&format!("c2-18-{key}")),
                json!({"content": text("@design and @old-team, a look? not @designers")}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{sent}");
    }
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("catch up");

    // In the group only Mara can read it; in the channel the whole group can.
    let mara = world.people["mara"].actor;
    let nina = world.people["nina"].actor;
    let omar = world.people["omar"].actor;
    assert_eq!(mentioned(ops.clone()).await, vec![mara]);
    let in_studio = mentioned(studio.clone()).await;
    for member in [mara, nina] {
        assert!(in_studio.contains(&member), "{member} in {in_studio:?}");
    }
    // An agent in the group gets no inbox item, the same as an agent named
    // directly: mention notifications reach accounts with project access.
    assert!(!in_studio.contains(&omar), "an archived group notified");
    assert!(!in_studio.contains(&lena.actor), "the author notified");

    // The list suggests it as typed after @, never the archived one.
    let (status, listed) = send(
        app,
        lena.browser.request(
            "POST",
            "/api/v2/ops/group_list",
            None,
            json!({"prefix": "@des", "members": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        listed["value"]["groups"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    assert_eq!(listed["value"]["groups"][0]["handle"], "design");
    assert_eq!(listed["value"]["groups"][0]["memberCount"], 3);

    world.finish().await;
}

/// C2.32: a note to self is a DM with only its writer in it. Opening it
/// again finds the same one, and nobody else reads, lists or finds it.
#[tokio::test]
async fn a_note_to_self_is_read_only_by_its_writer() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .build("c2_32_note_to_self")
        .await
    else {
        return;
    };
    let app = &world.app;
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let open = || async {
        let (status, opened) = send(
            app,
            lena.browser.request(
                "POST",
                "/api/v1/direct/dms",
                None,
                json!({"actorId": lena.actor}),
            ),
        )
        .await;
        assert!(status.is_success(), "{opened}");
        opened["conversation"]["id"]
            .as_str()
            .expect("note to self")
            .to_owned()
    };
    let note = open().await;
    assert_eq!(open().await, note, "one note to self each");
    let word = format!("zqnote{}", &Uuid::now_v7().simple().to_string()[20..]);
    let (status, sent) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/conversations/{note}/messages"),
            Some("note-to-self"),
            json!({"content": text(&format!("remember {word}"))}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");

    let found = get(app, lena, &format!("/api/v1/search?q={word}")).await;
    assert!(found.to_string().contains(&word), "{found}");
    let found = get(app, mara, &format!("/api/v1/search?q={word}")).await;
    assert_eq!(found["hits"], json!([]), "{found}");
    let (status, read) = send(
        app,
        mara.browser.request(
            "GET",
            &format!("/api/v1/conversations/{note}/messages"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(!status.is_success(), "{read}");
    let listed = get(app, mara, "/api/v1/direct").await;
    assert!(!listed.to_string().contains(&note), "{listed}");
    let listed = get(app, lena, "/api/v1/direct").await;
    assert!(listed.to_string().contains(&note), "{listed}");

    world.finish().await;
}

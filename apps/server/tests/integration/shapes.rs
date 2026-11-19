//! M1.17: shapes. The silent failures: a Friends workspace that still
//! shows tasks, projects, agents or runs somewhere (its modules, its
//! settings, an agent's tool list), an agent that can't join because
//! Agents is off, "Just me" drifting from Team's switches, and changing
//! shape back and forth losing a message, page or record.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{create_agent, send, tools_list},
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

/// Words a Friends workspace mustn't show (module ids and tool prefixes).
const NOT_FOR_FRIENDS: [&str; 4] = ["task", "project", "agent", "run"];

async fn tool(world: &Workspace, name: &'static str, arguments: Value) -> Value {
    world.apps["desk"]
        .call(&world.app, Call::Tool { name, arguments })
        .await
        .1
}

async fn set_shape(world: &Workspace, shape: &str) -> Value {
    let set = tool(world, "workspace_shape_set", json!({"shape": shape})).await;
    assert_eq!(set["outcome"], "ok", "{set}");
    set
}

/// The switchable modules that are on, by id.
async fn modules_on(world: &Workspace) -> Vec<String> {
    let listed = tool(world, "module_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    listed["value"]["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .filter(|module| module["switchable"] == true && module["on"] == true)
        .map(|module| module["id"].as_str().expect("id").to_owned())
        .collect()
}

/// Every message, page and record in the workspace that is still there.
async fn kept(world: &Workspace) -> Vec<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT t.id FROM messages t JOIN projects p ON p.id = t.project_id
        WHERE p.organization_id = $1
          AND to_jsonb(t)->>'deleted_at' IS NULL
        UNION ALL
        SELECT t.id FROM project_pages t JOIN projects p ON p.id = t.project_id
        WHERE p.organization_id = $1
          AND to_jsonb(t)->>'deleted_at' IS NULL AND to_jsonb(t)->>'archived_at' IS NULL
        UNION ALL
        SELECT t.id FROM collection_records t JOIN projects p ON p.id = t.project_id
        WHERE p.organization_id = $1 AND t.archived_at IS NULL
        ORDER BY 1
        "#,
    )
    .bind(world.organization)
    .fetch_all(world.pool())
    .await
    .expect("kept")
}

#[tokio::test]
async fn friends_shows_only_chat_and_changing_shape_loses_nothing() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .connected_app("desk")
        .build("m117_friends")
        .await
    else {
        return;
    };
    // Some of everything first, while every module is on.
    let channel = tool(
        &world,
        "channel_create",
        json!({"projectId": "WORK", "name": "trips"}),
    )
    .await;
    let channel_id = channel["value"]["id"].as_str().expect("channel").to_owned();
    let sent = tool(
        &world,
        "message_send",
        json!({"conversationId": channel_id, "text": "Where to next?"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let page = tool(
        &world,
        "page_write",
        json!({"projectId": "WORK", "title": "Packing list", "body": "Socks."}),
    )
    .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    let collection = tool(
        &world,
        "collection_write",
        json!({"projectId": "WORK", "name": "Places", "fields": [{"name": "Why", "type": "text"}]}),
    )
    .await;
    assert_eq!(collection["outcome"], "ok", "{collection}");
    let records = tool(
        &world,
        "record_write",
        json!({"projectId": "WORK", "collection": "Places",
               "records": [{"title": "Lisbon", "fields": {"Why": "the light"}}],
               "clientRequestId": "places"}),
    )
    .await;
    assert_eq!(records["outcome"], "ok", "{records}");
    let before = kept(&world).await;
    assert!(before.len() >= 3, "{before:?}");
    // Every module is on to begin with; counted here, not pinned, so a
    // new module doesn't break this test.
    let everything = modules_on(&world).await;

    // Previewing changes nothing.
    let preview = tool(
        &world,
        "workspace_shape_set",
        json!({"shape": "friends", "preview": true}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    assert_eq!(modules_on(&world).await, everything);

    let set = set_shape(&world, "friends").await;
    assert_eq!(set["value"]["shape"], "shapes.friends", "{set}");
    let on = modules_on(&world).await;
    assert_eq!(on, ["messages", "inbox"], "only chat is on");
    for word in NOT_FOR_FRIENDS {
        assert!(!on.iter().any(|id| id.contains(word)), "{word} in {on:?}");
    }
    // An agent's tool list names none of them.
    let rook = &world.agents["rook"];
    let tools = tools_list(&world.app, &rook.token, &rook.session).await;
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name == "message_send"), "{names:?}");
    for word in NOT_FOR_FRIENDS {
        let shown = names
            .iter()
            .filter(|name| {
                name.split('_')
                    .next()
                    .is_some_and(|first| first.starts_with(word))
            })
            .collect::<Vec<_>>();
        assert!(
            shown.is_empty(),
            "{word} tools in a Friends workspace: {shown:?}"
        );
    }
    // Nor do its settings.
    let space = send(
        &world.app,
        world
            .owner
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let settings = tool(&world, "settings_get", json!({"projectId": space})).await;
    if settings["outcome"] == "ok" {
        for setting in settings["value"]["settings"].as_array().expect("settings") {
            let key = setting["key"].as_str().expect("key");
            let area = key.split('.').next().unwrap_or(key);
            assert!(
                !NOT_FOR_FRIENDS.iter().any(|word| area.starts_with(word)) && area != "launch",
                "{key} shows in a Friends workspace"
            );
        }
    }

    // An agent joins with Agents left off, and the one already here still talks.
    // DMs are in no space (K4.17), so it joins in the workspace's own.
    assert!(!modules_on(&world).await.contains(&"agents".to_owned()));
    let workspace_space = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM spaces WHERE workspace_id = $1 AND kind = 'workspace'",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("workspace space")
    .to_string();
    let (_wren, _token) = create_agent(
        &world.app,
        &world.owner.browser,
        &workspace_space,
        "Wren",
        "wren",
    )
    .await;
    let hello = rook
        .call(
            &world.app,
            Call::Tool {
                name: "conversation_list",
                arguments: json!({}),
            },
        )
        .await
        .1;
    assert_eq!(hello["outcome"], "ok", "{hello}");

    // Friends, then Team, then Friends: nothing is lost.
    set_shape(&world, "team").await;
    assert_eq!(
        modules_on(&world).await.len(),
        everything.len(),
        "Team turns everything on"
    );
    set_shape(&world, "friends").await;
    let after = kept(&world).await;
    let lost = before
        .iter()
        .filter(|id| !after.contains(id))
        .collect::<Vec<_>>();
    assert!(lost.is_empty(), "lost {lost:?}");
    set_shape(&world, "team").await;
    let page_id = page["value"]["id"]
        .as_str()
        .or_else(|| page["value"]["page"]["id"].as_str())
        .expect("page id");
    let read = tool(&world, "page_get", json!({"pageId": page_id})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    world.finish().await;
}

#[tokio::test]
async fn just_me_matches_team_and_every_shape_is_a_package() {
    let Some(world) = WorldBuilder::default()
        .connected_app("desk")
        .build("m117_shapes")
        .await
    else {
        return;
    };
    let listed = tool(&world, "directory_list", json!({"shapes": true})).await;
    let shapes = listed["value"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed}"))
        .iter()
        .map(|entry| entry["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        shapes,
        [
            "shapes.team",
            "shapes.friends",
            "shapes.just_me",
            "shapes.documents",
            "shapes.blank"
        ]
    );
    // Each one previews as a package install.
    for shape in &shapes {
        let preview = tool(&world, "package_preview", json!({"entry": shape})).await;
        assert_eq!(preview["outcome"], "ok", "{shape}: {preview}");
    }
    set_shape(&world, "team").await;
    let team = modules_on(&world).await;
    set_shape(&world, "just_me").await;
    assert_eq!(
        modules_on(&world).await,
        team,
        "Just me has Team's switches"
    );
    let current = tool(&world, "directory_list", json!({"shapes": true})).await;
    assert_eq!(current["value"]["shape"], "shapes.just_me", "{current}");
    set_shape(&world, "blank").await;
    assert_eq!(modules_on(&world).await, ["messages"]);
    world.finish().await;
}

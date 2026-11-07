//! AWD-75: modules. The silent, costly failures: a module that's switched
//! off still reachable by an agent, a member switching modules, a role
//! narrowing that grants instead of narrows, the owner role losing a module,
//! and switching off deleting anything.

use super::{
    agent_interface::{Browser, connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::space,
    message_edits::{add_person, create_channel, direct_space, say, text},
    roles_and_access::{listed_tools, public_call},
    w20_files::{send_with, upload, upload_to, with_data_root},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

fn modules_path(setup: &Setup) -> String {
    format!("/api/v1/organizations/{}/modules", setup.organization_id)
}

async fn modules(setup: &Setup, browser: &Browser) -> Value {
    let (status, body) = send(
        &setup.app,
        browser.request("GET", &modules_path(setup), None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn module<'a>(listed: &'a Value, id: &str) -> &'a Value {
    listed["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .find(|module| module["id"] == id)
        .unwrap_or_else(|| panic!("{id} in {listed}"))
}

pub(super) async fn switch(setup: &Setup, browser: &Browser, body: Value) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request("PUT", &modules_path(setup), None, body),
    )
    .await
}

async fn layout(setup: &Setup, browser: &Browser, project: &str) -> Value {
    let (status, body) = send(
        &setup.app,
        browser.request(
            "GET",
            &format!("/api/v1/projects/{project}/layout"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["effective"].clone()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

async fn search_titles(setup: &Setup, browser: &Browser, text: &str) -> Vec<String> {
    let (status, body) = send(
        &setup.app,
        browser.request(
            "GET",
            &format!("/api/v1/search?q={text}&kinds=tasks"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter_map(|hit| hit["title"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_module_switched_off_is_hidden_for_people_and_agents_and_comes_back_whole() {
    let Some(setup) = setup("awd75_switches").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (_bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("awd75-lantern"),
            json!({"title": "Lantern repair"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    assert_eq!(
        search_titles(&setup, &setup.browser, "Lantern").await.len(),
        1
    );

    // Everything starts on, and members can read but not switch.
    let listed = modules(&setup, &bea_browser).await;
    assert_eq!(listed["canManage"], false);
    assert!(
        listed["modules"]
            .as_array()
            .expect("modules")
            .iter()
            .all(|module| module["on"] == true)
    );
    let (status, body) = switch(&setup, &bea_browser, json!({"switches": {"tasks": false}})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // The workspace itself can't be switched, and unknown modules are refused.
    let (status, _) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"workspace": false}}),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let (status, _) = switch(&setup, &setup.browser, json!({"switches": {"nope": false}})).await;
    assert!(status.is_client_error(), "{status}");

    // The owner turns Tasks off; Runs needs it, so it goes too.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"tasks": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(module(&changed, "tasks")["on"], false);
    assert_eq!(module(&changed, "runs")["on"], false);
    assert_eq!(module(&changed, "runs")["switchedOn"], true);
    assert!(
        module(&changed, "runs")["offBecause"]
            .as_str()
            .is_some_and(|why| why.contains("needs Tasks")),
        "{changed}"
    );
    assert_eq!(module(&changed, "pages")["on"], true);
    let audited = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE action = 'modules.changed' AND target_id = $1",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("audit row");
    assert_eq!(strings(&audited["after"]["off"]), ["tasks"], "{audited}");
    assert_eq!(strings(&audited["before"]["off"]), Vec::<String>::new());

    // What people see follows: the layout hides Tasks' parts and says why.
    let effective = layout(&setup, &bea_browser, &work).await;
    assert!(strings(&effective["modulesOff"]).contains(&"tasks".to_owned()));
    assert!(strings(&effective["modulesOff"]).contains(&"runs".to_owned()));
    assert!(strings(&effective["hidden"]).contains(&"area.board".to_owned()));
    assert!(!strings(&effective["hidden"]).contains(&"area.pages".to_owned()));
    // Search leaves tasks out.
    assert!(
        search_titles(&setup, &setup.browser, "Lantern")
            .await
            .is_empty()
    );

    // Agents: an app connected for Bea doesn't list Tasks' tools, and a call
    // says the module is off, over MCP and the public API alike.
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let tools = listed_tools(app, &bea_app, &session).await;
    for gone in ["task_list", "task_create", "run_start", "milestone_list"] {
        assert!(!tools.contains(&gone.to_owned()), "{gone} in {tools:?}");
    }
    for kept in ["module_list", "message_send", "page_list"] {
        assert!(tools.contains(&kept.to_owned()), "{kept} not in {tools:?}");
    }
    let refused = tool(
        app,
        &bea_app,
        &session,
        "task_list",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(refused["retryCategory"], "module", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("Tasks") && message.contains("turned off")),
        "{refused}"
    );
    let (status, refused) =
        public_call(app, &bea_app, "task_list", json!({"projectId": work})).await;
    assert!(
        !status.is_success() || refused["outcome"] == "permission_denied",
        "{status}: {refused}"
    );
    let described = tool(
        app,
        &bea_app,
        &session,
        "module_list",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(described["outcome"], "ok", "{described}");
    assert_eq!(module(&described["value"], "tasks")["on"], false);

    // Off means hidden, never deleted: on again, everything is back.
    let (status, changed) =
        switch(&setup, &setup.browser, json!({"switches": {"tasks": true}})).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(module(&changed, "runs")["on"], true);
    assert_eq!(
        search_titles(&setup, &setup.browser, "Lantern").await.len(),
        1
    );
    let listed = tool(
        app,
        &bea_app,
        &session,
        "task_list",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");

    // A preset switches many at once: Chat with my agents leaves projects out.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"preset": "chat_with_agents"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["preset"], "chat_with_agents");
    for off in ["projects", "channels", "tasks", "pages", "runs"] {
        assert_eq!(module(&changed, off)["on"], false, "{off}");
    }
    assert_eq!(module(&changed, "messages")["on"], true);
}

#[tokio::test]
async fn roles_narrow_modules_and_never_widen_them() {
    let Some(setup) = setup("awd75_roles").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let roles_path = format!("/api/v1/organizations/{}/roles", setup.organization_id);
    let (status, listed) = send(
        app,
        setup.browser.request("GET", &roles_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let role_of = |key: &str| {
        listed["roles"]
            .as_array()
            .expect("roles")
            .iter()
            .find(|role| role["key"] == key)
            .cloned()
            .unwrap_or_else(|| panic!("{key}"))
    };
    let member = role_of("member");
    let owner = role_of("owner");

    // Only modules that can be switched off, and never for owners.
    let (status, body) = send(
        app,
        setup.browser.request(
            "POST",
            &roles_path,
            None,
            json!({"name": "Odd", "baseRole": "member", "permissions": member["permissions"], "modulesOff": ["workspace"]}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {body}");
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("{roles_path}/{}", owner["id"].as_str().expect("id")),
            None,
            json!({"modulesOff": ["runs"]}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {body}");

    // A role without Runs.
    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            &roles_path,
            None,
            json!({"name": "No runs", "baseRole": "member", "permissions": member["permissions"], "modulesOff": ["runs"]}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    assert_eq!(strings(&made["modulesOff"]), ["runs"], "{made}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("membership version");
    let (status, body) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/role",
                setup.organization_id
            ),
            None,
            json!({"roleId": made["id"], "applyToProjects": true, "expectedVersion": version}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Bea doesn't get Runs; the owner still does, and so does the workspace.
    let bea_layout = layout(&setup, &bea_browser, &work).await;
    assert_eq!(strings(&bea_layout["modulesOff"]), ["runs"], "{bea_layout}");
    assert!(strings(&bea_layout["hidden"]).contains(&"area.runs".to_owned()));
    let owner_layout = layout(&setup, &setup.browser, &work).await;
    assert!(
        strings(&owner_layout["modulesOff"]).is_empty(),
        "{owner_layout}"
    );
    let listed = modules(&setup, &bea_browser).await;
    assert_eq!(module(&listed, "runs")["on"], false);
    assert_eq!(module(&listed, "runs")["switchedOn"], true);
    assert!(
        module(&listed, "runs")["offBecause"]
            .as_str()
            .is_some_and(|why| why.contains("your role"))
    );

    // Her app follows her role: no run tools, task tools as before.
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let tools = listed_tools(app, &bea_app, &session).await;
    assert!(!tools.contains(&"run_start".to_owned()), "{tools:?}");
    assert!(tools.contains(&"task_list".to_owned()), "{tools:?}");
    let refused = tool(
        app,
        &bea_app,
        &session,
        "launch_list",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // A role can't turn on what the workspace has off.
    let (status, _) = send(
        app,
        setup.browser.request(
            "PUT",
            &modules_path(&setup),
            None,
            json!({"switches": {"pages": false}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let bea_layout = layout(&setup, &bea_browser, &work).await;
    let off = strings(&bea_layout["modulesOff"]);
    assert!(
        off.contains(&"pages".to_owned()) && off.contains(&"runs".to_owned()),
        "{off:?}"
    );
}

/// The ids of the files a search for `text` finds for `browser`.
async fn found_files(setup: &Setup, browser: &Browser, text: &str) -> Vec<String> {
    let (status, body) = send(
        &setup.app,
        browser.request(
            "GET",
            &format!("/api/v1/search?q={text}&kinds=files"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter_map(|hit| hit["id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn file_search_follows_the_module_each_file_came_from() {
    let Some((setup, _root)) = with_data_root("awd75_file_search").await else {
        return;
    };
    let owner = &setup.browser;
    let space = space(&setup).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let (rook, _rook_browser) = add_person(&setup, &[&space, &work], "rook").await;

    // One file in a channel, one in a DM, one on a task, all named alike.
    let channel = super::message_edits::create_channel(&setup, &work, "general").await;
    let in_channel = upload_to(&setup, owner, &channel, "ledger-channel.txt").await;
    let (status, sent) = send_with(
        &setup,
        owner,
        &channel,
        "ledger-channel",
        "",
        &[&in_channel],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let (status, dm) = send(
        &setup.app,
        owner.request("POST", "/api/v1/direct/dms", None, json!({"actorId": rook})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("dm").to_owned();
    let in_dm = upload_to(&setup, owner, &dm, "ledger-dm.txt").await;
    let (status, sent) = send_with(&setup, owner, &dm, "ledger-dm", "", &[&in_dm]).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let (status, task) = send(
        &setup.app,
        owner.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("awd75-ledger-task"),
            json!({"title": "Close the books"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let (status, on_task) = upload(
        &setup,
        owner,
        &format!("/api/v1/projects/{work}/work-items/{task}/files?name=ledger-task.txt"),
        "text/plain",
        b"ledger",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{on_task}");
    let on_task = on_task["id"].as_str().expect("file").to_owned();
    let mut all = vec![in_channel.clone(), in_dm.clone(), on_task.clone()];
    all.sort();
    let mut found = found_files(&setup, owner, "ledger").await;
    found.sort();
    assert_eq!(found, all);

    // Channels off: the channel's file goes, the DM's and the task's stay.
    let (status, changed) = switch(&setup, owner, json!({"switches": {"channels": false}})).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let found = found_files(&setup, owner, "ledger").await;
    assert!(!found.contains(&in_channel), "{found:?}");
    assert!(
        found.contains(&in_dm) && found.contains(&on_task),
        "{found:?}"
    );

    // Tasks off instead: the task's file goes, the channel's comes back.
    let (status, changed) = switch(
        &setup,
        owner,
        json!({"switches": {"channels": true, "tasks": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let found = found_files(&setup, owner, "ledger").await;
    assert!(!found.contains(&on_task), "{found:?}");
    assert!(
        found.contains(&in_channel) && found.contains(&in_dm),
        "{found:?}"
    );

    // Off means hidden, never deleted: all on, all three are found again.
    let (status, changed) = switch(&setup, owner, json!({"switches": {"tasks": true}})).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let mut found = found_files(&setup, owner, "ledger").await;
    found.sort();
    assert_eq!(found, all);
}

/// A thread in `conversation` started from `origin`, as its owner.
async fn start_thread(setup: &Setup, conversation: &str, origin: &str, key: &str) -> String {
    let (status, thread) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/threads"),
            Some(key),
            json!({"name": "Follow-up", "originMessageId": origin}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{thread}");
    thread["conversation"]["id"]
        .as_str()
        .expect("thread")
        .to_owned()
}

#[tokio::test]
async fn conversation_tools_follow_the_module_where_the_conversation_lives() {
    let Some(setup) = setup("awd75_conversation_tools").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let direct = direct_space(&setup).await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &direct], "bea").await;
    let channel = create_channel(&setup, &work, "general").await;
    let in_channel = say(
        &setup,
        &setup.browser,
        &channel,
        "awd75-c1",
        text("Release notes"),
    )
    .await;
    let channel_thread = start_thread(&setup, &channel, &in_channel, "awd75-ct").await;
    let (status, dm) = send(
        app,
        setup
            .browser
            .request("POST", "/api/v1/direct/dms", None, json!({"actorId": bea})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("dm").to_owned();
    let in_dm = say(&setup, &setup.browser, &dm, "awd75-d1", text("Lunch?")).await;
    let dm_thread = start_thread(&setup, &dm, &in_dm, "awd75-dt").await;

    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("app");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let listed_ids = |listed: &Value| -> Vec<String> {
        listed["value"]["conversations"]
            .as_array()
            .expect("conversations")
            .iter()
            .filter_map(|conversation| conversation["id"].as_str().map(str::to_owned))
            .collect()
    };
    let listed = call("conversation_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(listed_ids(&listed).contains(&channel), "{listed}");
    assert!(listed_ids(&listed).contains(&dm), "{listed}");

    // Channels off, Messages on: a project's channel and its thread are
    // turned off for the conversation tools, which say so.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"channels": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    for (name, arguments) in [
        ("conversation_read", json!({"conversationId": channel})),
        (
            "conversation_open",
            json!({"conversationId": channel_thread}),
        ),
        (
            "message_send",
            json!({"conversationId": channel, "text": "Still here?", "clientRequestId": "awd75-off-1"}),
        ),
        (
            "message_send",
            json!({"conversationId": channel_thread, "text": "Still here?", "clientRequestId": "awd75-off-2"}),
        ),
    ] {
        let refused = call(name, arguments).await;
        assert_eq!(refused["outcome"], "permission_denied", "{name}: {refused}");
        assert_eq!(refused["retryCategory"], "module", "{name}: {refused}");
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|message| message.contains("Channels")),
            "{name}: {refused}"
        );
        assert!(
            !refused.to_string().contains("Release notes"),
            "{name}: {refused}"
        );
    }
    let posted = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE plain_text = 'Still here?'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(posted, 0, "a message went into a channel that's off");

    // As things too: the channel's thread is turned off, a DM thread isn't.
    let hidden = call(
        "thing_open",
        json!({"kind": "thread", "id": channel_thread}),
    )
    .await;
    assert_eq!(hidden["outcome"], "not_found", "{hidden}");
    let shown = call("thing_open", json!({"kind": "thread", "id": dm_thread})).await;
    assert_eq!(shown["outcome"], "ok", "{shown}");
    // It links where DM threads open, not to the Channels page.
    assert_eq!(
        shown["value"]["thing"]["href"],
        format!("/messages/{dm}?thread={dm_thread}"),
        "{shown}"
    );
    // And the thread tools stay listed for DMs; creating a channel doesn't.
    let tools = listed_tools(app, &token, &session).await;
    for listed in ["thread_start", "thread_update"] {
        assert!(
            tools.iter().any(|tool| tool == listed),
            "{listed}: {tools:?}"
        );
    }
    assert!(
        !tools.iter().any(|tool| tool == "channel_create"),
        "{tools:?}"
    );

    // A group's thread links the same way, opened itself or linked from a
    // task.
    let (status, group) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some("awd75-group"),
            json!({"memberIds": [bea]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let in_group = say(&setup, &setup.browser, &group, "awd75-g1", text("Plans?")).await;
    let group_thread = start_thread(&setup, &group, &in_group, "awd75-gt").await;
    let group_href = format!("/messages/{group}?thread={group_thread}");
    let opened = call("thing_open", json!({"kind": "thread", "id": group_thread})).await;
    assert_eq!(opened["value"]["thing"]["href"], group_href, "{opened}");
    let created = call(
        "task_create",
        json!({"projectId": work, "title": "Follow the group's plans"}),
    )
    .await;
    assert_eq!(created["outcome"], "applied", "{created}");
    let task = created["workItemId"].as_str().expect("task").to_owned();
    sqlx::query(
        r#"
        INSERT INTO work_item_links (work_item_id, project_id, target_kind, target_id, created_by_actor_id)
        SELECT item.id, item.project_id, 'conversation', $2::uuid, membership.actor_id
        FROM work_items item
        JOIN organization_memberships membership
          ON membership.organization_id = $3 AND membership.role = 'owner'
        WHERE item.id = $1::uuid
        "#,
    )
    .bind(&task)
    .bind(&group_thread)
    .bind(setup.organization_id)
    .execute(&setup.pool)
    .await
    .expect("link");
    let linked = call("thing_open", json!({"kind": "task", "id": task})).await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    assert!(
        linked["value"]["links"]
            .as_array()
            .is_some_and(|links| links.iter().any(|link| {
                link["other"]["id"] == group_thread.as_str()
                    && link["other"]["href"] == group_href.as_str()
            })),
        "{linked}"
    );

    // And conversation_list leaves the project.s channels and threads out,
    // previews and all, while DMs stay.
    let listed = call("conversation_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let ids = listed_ids(&listed);
    assert!(ids.contains(&dm), "{listed}");
    assert!(
        !ids.contains(&channel) && !ids.contains(&channel_thread),
        "{listed}"
    );
    assert!(!listed.to_string().contains("Release notes"), "{listed}");

    // DMs and their threads follow Messages, which is on.
    let read = call("conversation_read", json!({"conversationId": dm})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let opened = call("conversation_open", json!({"conversationId": dm_thread})).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let sent = call(
        "message_send",
        json!({"conversationId": dm_thread, "text": "Noon works", "clientRequestId": "awd75-on-1"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");

    // The conversation tools follow the conversation's project whichever
    // module lists them: editing a message in the channel needs Channels,
    // and starting a thread in the DM needs only Messages.
    let edited = call(
        "message_edit",
        json!({"messageId": in_channel, "text": "Release notes, edited", "clientRequestId": "awd75-off-3"}),
    )
    .await;
    assert_eq!(edited["outcome"], "permission_denied", "{edited}");
    assert_eq!(edited["retryCategory"], "module", "{edited}");
    assert!(
        edited["message"]
            .as_str()
            .is_some_and(|message| message.contains("Channels")),
        "{edited}"
    );
    let in_dm_again = say(&setup, &setup.browser, &dm, "awd75-d2", text("And Friday?")).await;
    let threaded = call(
        "thread_start",
        json!({"conversationId": dm, "fromMessageId": in_dm_again}),
    )
    .await;
    assert_eq!(threaded["outcome"], "ok", "{threaded}");

    // A channel that is there, in a project Bea isn't in, with Channels off:
    // her app is refused as before (the tool gate answers "forbidden" for a
    // project she isn't in, before any module check), and never told that
    // the module is off or anything about the channel.
    let elsewhere = create_project(&setup, "ELSE", "Elsewhere").await;
    let unseen = create_channel(&setup, &elsewhere, "private-plans").await;
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let bea_session = initialize(app, &bea_app).await.expect("Bea's app");
    let ask = |name: &'static str, conversation: String, key: &'static str| {
        let token = bea_app.clone();
        let session = bea_session.clone();
        let arguments = if name == "message_send" {
            json!({"conversationId": conversation, "text": "Hello", "clientRequestId": key})
        } else {
            json!({"conversationId": conversation})
        };
        async move { tool(app, &token, &session, name, arguments).await }
    };
    for name in ["conversation_read", "conversation_open", "message_send"] {
        let hidden = ask(name, unseen.clone(), "awd75-unseen").await;
        let missing = ask(name, Uuid::now_v7().to_string(), "awd75-missing").await;
        assert!(
            hidden["outcome"] == "forbidden" || hidden["outcome"] == "not_found",
            "{name}: {hidden}"
        );
        assert_eq!(missing["outcome"], "not_found", "{name}: {missing}");
        assert_ne!(hidden["retryCategory"], "module", "{name}: {hidden}");
        let said = hidden.to_string();
        assert!(
            !said.contains("Channels") && !said.contains("private-plans"),
            "{name}: {hidden}"
        );
    }

    // A conversation that isn't there still says only that.
    let nowhere = Uuid::now_v7().to_string();
    for (name, arguments) in [
        ("conversation_read", json!({"conversationId": nowhere})),
        (
            "message_send",
            json!({"conversationId": nowhere, "text": "Hello", "clientRequestId": "awd75-none"}),
        ),
    ] {
        let missing = call(name, arguments).await;
        assert_eq!(missing["outcome"], "not_found", "{name}: {missing}");
    }

    // Off means hidden, never deleted: Channels on again, the channel reads.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"channels": true}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let read = call("conversation_read", json!({"conversationId": channel})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert!(read.to_string().contains("Release notes"), "{read}");
}

#[tokio::test]
async fn task_lists_leave_out_projects_where_tasks_is_off() {
    let Some(setup) = setup("awd75_task_lists").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    for (project, title, key) in [
        (&work, "Lantern repair", "awd75-t1"),
        (&other, "Harbour survey", "awd75-t2"),
    ] {
        let (status, made) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{project}/work-items"),
                Some(key),
                json!({"title": title}),
            ),
        )
        .await;
        assert!(status.is_success(), "{status}: {made}");
    }
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let titles = |result: &Value| {
        let mut titles = result["value"]["tasks"]
            .as_array()
            .unwrap_or_else(|| panic!("tasks: {result}"))
            .iter()
            .map(|task| task["title"].as_str().expect("title").to_owned())
            .collect::<Vec<_>>();
        titles.sort();
        titles
    };
    let mixed = json!({"projectIds": [work, other]});
    let listed = tool(app, &bea_app, &session, "task_list", json!({})).await;
    assert_eq!(
        titles(&listed),
        ["Harbour survey", "Lantern repair"],
        "{listed}"
    );

    // In Other only, Bea's role leaves Tasks out; she still may see tasks.
    let roles_path = format!("/api/v1/organizations/{}/roles", setup.organization_id);
    let (status, roles) = send(
        app,
        setup.browser.request("GET", &roles_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{roles}");
    let member = roles["roles"]
        .as_array()
        .expect("roles")
        .iter()
        .find(|role| role["key"] == "member")
        .cloned()
        .expect("member");
    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            &roles_path,
            None,
            json!({"name": "No tasks module", "baseRole": "member", "permissions": member["permissions"], "modulesOff": ["tasks"]}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(other.parse::<Uuid>().expect("project"))
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{other}/members/{bea}"),
            Some("awd75-bea-other"),
            json!({"expectedVersion": version, "roleId": made["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");

    // Unscoped and with both projects named, only Work's task is listed; the
    // tool still works, since Tasks is on in Work.
    for arguments in [json!({}), mixed] {
        let listed = tool(app, &bea_app, &session, "task_list", arguments.clone()).await;
        assert_eq!(listed["outcome"], "ok", "{arguments}: {listed}");
        assert_eq!(titles(&listed), ["Lantern repair"], "{arguments}: {listed}");
        assert!(
            !listed.to_string().contains("Harbour survey"),
            "{arguments}: {listed}"
        );
    }
    // The owner's role doesn't narrow modules: she still lists both.
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &token).await.expect("owner app");
    let listed = tool(
        app,
        &token,
        &owner_session,
        "task_list",
        json!({"projectIds": [work, other]}),
    )
    .await;
    assert_eq!(
        titles(&listed),
        ["Harbour survey", "Lantern repair"],
        "{listed}"
    );
}

/// A role with these modules off, as the member role otherwise is, put on
/// `actor` in `project`.
pub(super) async fn narrow_in_project(
    setup: &Setup,
    project: &str,
    actor: Uuid,
    modules_off: &[&str],
    key: &str,
) {
    let roles_path = format!("/api/v1/organizations/{}/roles", setup.organization_id);
    let (status, roles) = send(
        &setup.app,
        setup.browser.request("GET", &roles_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{roles}");
    let member = roles["roles"]
        .as_array()
        .expect("roles")
        .iter()
        .find(|role| role["key"] == "member")
        .cloned()
        .expect("member");
    let (status, made) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &roles_path,
            None,
            json!({"name": format!("Narrowed {key}"), "baseRole": "member", "permissions": member["permissions"], "modulesOff": modules_off}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project.parse::<Uuid>().expect("project"))
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{project}/members/{actor}"),
            Some(key),
            json!({"expectedVersion": version, "roleId": made["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
}

#[tokio::test]
async fn things_follow_the_modules_of_their_own_project() {
    let Some(setup) = setup("awd75_things").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    let in_work = create_channel(&setup, &work, "general").await;
    let in_other = create_channel(&setup, &other, "harbour").await;
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let open = |channel: &str| {
        let token = bea_app.clone();
        let session = session.clone();
        let channel = channel.to_owned();
        async move {
            tool(
                app,
                &token,
                &session,
                "thing_open",
                json!({"kind": "channel", "id": channel}),
            )
            .await
        }
    };
    let browse = |channel: &str| {
        let request = bea_browser.request(
            "GET",
            &format!("/api/v1/things/channel/{channel}"),
            None,
            Value::Null,
        );
        async move { send(app, request).await }
    };
    for channel in [&in_work, &in_other] {
        assert_eq!(open(channel).await["outcome"], "ok");
        assert_eq!(browse(channel).await.0, StatusCode::OK);
    }

    // In Other only, Bea's role leaves Channels out. Channels is still on for
    // her in Work, so the union of her modules has it; Other's channel goes.
    narrow_in_project(&setup, &other, bea, &["channels"], "awd75-things-bea").await;
    let hidden = open(&in_other).await;
    assert_eq!(hidden["outcome"], "not_found", "{hidden}");
    assert!(!hidden.to_string().contains("harbour"), "{hidden}");
    let (status, body) = browse(&in_other).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(!body.to_string().contains("harbour"), "{body}");
    assert_eq!(open(&in_work).await["outcome"], "ok");
    assert_eq!(browse(&in_work).await.0, StatusCode::OK);
}

#[tokio::test]
async fn notifications_follow_the_recipient_s_role_modules() {
    let Some(setup) = setup("awd75_notification_roles").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("awd75-notified-task"),
            json!({"title": "Ledger review"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {task}");
    let task = task["id"]
        .as_str()
        .expect("task")
        .parse::<Uuid>()
        .expect("task id");
    sqlx::query(
        r#"
        INSERT INTO notifications (
            id, project_id, recipient_actor_id, kind, subject_kind, subject_id,
            work_item_id, group_key
        ) VALUES ($1, $2, $3, 'assigned', 'work_item', $4, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(bea)
    .bind(task)
    .bind(format!("assigned:{task}"))
    .execute(&setup.pool)
    .await
    .expect("notification");
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let count = |result: &Value| {
        result["value"]["notifications"]
            .as_array()
            .unwrap_or_else(|| panic!("notifications: {result}"))
            .len()
    };
    let listed = tool(app, &bea_app, &session, "notification_list", json!({})).await;
    assert_eq!(count(&listed), 1, "{listed}");

    // Her role in Work turns Tasks off: the notification she already has
    // leaves every read, though it's kept.
    narrow_in_project(&setup, &work, bea, &["tasks"], "awd75-notified-bea").await;
    let listed = tool(app, &bea_app, &session, "notification_list", json!({})).await;
    assert_eq!(count(&listed), 0, "{listed}");
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1",
    )
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(kept, 1, "turning a module off deleted a notification");

    // Back on a role that leaves Tasks on, it's back.
    narrow_in_project(&setup, &work, bea, &[], "awd75-notified-bea-again").await;
    let listed = tool(app, &bea_app, &session, "notification_list", json!({})).await;
    assert_eq!(count(&listed), 1, "{listed}");
}

#[tokio::test]
async fn task_lists_count_and_page_only_workspaces_with_tasks_on() {
    let Some(setup) = setup("awd75_task_pages").await else {
        return;
    };
    let app = &setup.app;
    // The owner is in two workspaces: this test's, and a second one.
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Second', $2)")
        .bind(second)
        .bind(format!("second-{}", second.simple()))
        .execute(&setup.pool)
        .await
        .expect("second workspace");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(second)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("second membership");
    let first_project = create_project(&setup, "AAA", "A work").await;
    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some("create-BBB"),
            json!({"organizationId": second, "key": "BBB", "name": "B work"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let second_project = made["id"].as_str().expect("project").to_owned();
    let mut first_tasks = Vec::new();
    for (project, title) in [
        (&first_project, "A one"),
        (&first_project, "A two"),
        (&second_project, "B one"),
        (&second_project, "B two"),
        (&second_project, "B three"),
    ] {
        let key = format!("awd75-{title}").replace(' ', "-");
        let (status, made) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{project}/work-items"),
                Some(key.as_str()),
                json!({"title": title}),
            ),
        )
        .await;
        assert!(status.is_success(), "{status}: {made}");
        if project == &first_project {
            first_tasks.push(made["id"].as_str().expect("task").to_owned());
        }
    }
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("owner app");

    // Every page, one task at a time: the titles, and each page's total.
    let pages = |arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move {
            let (mut titles, mut totals, mut cursor) = (Vec::new(), Vec::new(), None::<String>);
            for _ in 0..10 {
                let mut asked = arguments.clone();
                asked["limit"] = json!(1);
                if let Some(cursor) = &cursor {
                    asked["cursor"] = json!(cursor);
                }
                let listed = tool(app, &token, &session, "task_list", asked).await;
                assert_eq!(listed["outcome"], "ok", "{listed}");
                totals.push(listed["value"]["total"].as_i64().expect("total"));
                for task in listed["value"]["tasks"].as_array().expect("tasks") {
                    titles.push(task["title"].as_str().expect("title").to_owned());
                }
                cursor = listed["value"]["nextCursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
            }
            assert!(cursor.is_none(), "the pages never ended");
            titles.sort();
            (titles, totals)
        }
    };
    let (titles, totals) = pages(json!({})).await;
    assert_eq!(titles, ["A one", "A two", "B one", "B three", "B two"]);
    assert!(totals.iter().all(|total| *total == 5), "{totals:?}");

    // Tasks off in this test's workspace, on in the second: unscoped and with
    // both projects named, only the second's tasks are counted and paged.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"tasks": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    for arguments in [
        json!({}),
        json!({"projectIds": [first_project, second_project]}),
    ] {
        let (titles, totals) = pages(arguments.clone()).await;
        assert_eq!(titles, ["B one", "B three", "B two"], "{arguments}");
        assert_eq!(totals.len(), 3, "{arguments}: {totals:?}");
        assert!(
            totals.iter().all(|total| *total == 3),
            "{arguments}: {totals:?}"
        );
    }
    // A batch change of the first's tasks, naming no project, is refused as
    // Tasks being off there, and changes nothing.
    let refused = tool(
        app,
        &token,
        &session,
        "task_update_many",
        json!({"taskIds": first_tasks, "priority": "urgent"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(refused["retryCategory"], "module", "{refused}");
    let urgent =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_items WHERE priority = 'urgent'")
            .fetch_one(&setup.pool)
            .await
            .expect("count");
    assert_eq!(urgent, 0, "a batch changed tasks where Tasks is off");

    // Only the first's project named: nothing, and a total of none.
    let listed = tool(
        app,
        &token,
        &session,
        "task_list",
        json!({"projectIds": [first_project]}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["total"], 0, "{listed}");
    assert_eq!(
        listed["value"]["tasks"].as_array().map(Vec::len),
        Some(0),
        "{listed}"
    );
}

#[tokio::test]
async fn a_project_role_s_modules_reach_boards_links_and_module_list() {
    let Some(setup) = setup("awd75_project_role_reach").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{other}/work-items"),
            Some("awd75-reach-task"),
            json!({"title": "Harbour survey"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    let mut boards = Vec::new();
    for (project, title) in [(&work, "Work map"), (&other, "Harbour map")] {
        let made = tool(
            app,
            &owner_app,
            &owner_session,
            "whiteboard_create",
            json!({"projectId": project, "title": title}),
        )
        .await;
        assert_eq!(made["outcome"], "ok", "{made}");
        boards.push(
            made["value"]["whiteboard"]["id"]
                .as_str()
                .expect("board")
                .to_owned(),
        );
    }
    let (in_work, in_other) = (boards[0].clone(), boards[1].clone());
    // Work's board links to Other's task (links stay within the workspace).
    let linked = tool(
        app,
        &owner_app,
        &owner_session,
        "whiteboard_link",
        json!({"whiteboardId": in_work, "targetKind": "task", "targetId": task}),
    )
    .await;
    assert_eq!(linked["outcome"], "ok", "{linked}");

    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let call = |name: &'static str, arguments: Value| {
        let token = bea_app.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let read = call("whiteboard_read", json!({"whiteboardId": in_other})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let opened = call("thing_open", json!({"kind": "whiteboard", "id": in_work})).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(opened.to_string().contains("Harbour survey"), "{opened}");
    let listed = call("module_list", json!({"projectId": other})).await;
    assert_eq!(module(&listed["value"], "tasks")["on"], true, "{listed}");

    // In Other only, Bea's role leaves Whiteboards and Tasks out.
    narrow_in_project(
        &setup,
        &other,
        bea,
        &["whiteboards", "tasks"],
        "awd75-reach-bea",
    )
    .await;
    for (name, arguments) in [
        ("whiteboard_read", json!({"whiteboardId": in_other})),
        (
            "whiteboard_update",
            json!({"whiteboardId": in_other, "title": "Renamed"}),
        ),
    ] {
        let refused = call(name, arguments).await;
        assert_eq!(refused["outcome"], "permission_denied", "{name}: {refused}");
        assert_eq!(refused["retryCategory"], "module", "{name}: {refused}");
    }
    let still = call("whiteboard_read", json!({"whiteboardId": in_work})).await;
    assert_eq!(still["outcome"], "ok", "{still}");
    let renamed =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM whiteboards WHERE title = 'Renamed'")
            .fetch_one(&setup.pool)
            .await
            .expect("count");
    assert_eq!(renamed, 0, "a board in a module that's off was changed");
    // Work's board still opens, but its link to Other's task is left out.
    let opened = call("thing_open", json!({"kind": "whiteboard", "id": in_work})).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(!opened.to_string().contains("Harbour survey"), "{opened}");
    // module_list says so for Other, not for Work or the workspace.
    let listed = call("module_list", json!({"projectId": other})).await;
    assert_eq!(module(&listed["value"], "tasks")["on"], false, "{listed}");
    assert_eq!(
        module(&listed["value"], "whiteboards")["on"],
        false,
        "{listed}"
    );
    let listed = call("module_list", json!({"projectId": work})).await;
    assert_eq!(module(&listed["value"], "tasks")["on"], true, "{listed}");
    let listed = call("module_list", json!({})).await;
    assert_eq!(module(&listed["value"], "tasks")["on"], true, "{listed}");
}

#[tokio::test]
async fn boards_across_a_workspace_follow_its_modules_and_credential_scope() {
    use agent_workspace_server::mcp::{IssueCredentialCommand, issue_credential};
    let Some(setup) = setup("awd75_workspace_boards").await else {
        return;
    };
    let app = &setup.app;
    let owner =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&setup.pool)
            .await
            .expect("owner");
    let work = create_project(&setup, "WORK", "Work").await;
    // A second workspace the owner is also in, where Whiteboards stays on.
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Second', $2)")
        .bind(second)
        .bind(format!("second-{}", second.simple()))
        .execute(&setup.pool)
        .await
        .expect("second workspace");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(second)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("second membership");
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("owner app");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let mut made = Vec::new();
    for arguments in [
        json!({"title": "Team board", "workspaceId": setup.organization_id}),
        json!({"title": "Work board", "projectId": work}),
        json!({"title": "Second board", "workspaceId": second}),
    ] {
        let board = call("whiteboard_create", arguments.clone()).await;
        assert_eq!(board["outcome"], "ok", "{arguments}: {board}");
        made.push(
            board["value"]["whiteboard"]["id"]
                .as_str()
                .expect("board")
                .to_owned(),
        );
    }
    let (across, in_work, in_second) = (made[0].clone(), made[1].clone(), made[2].clone());

    // An older credential confined to Work opens Work's board but never one
    // across the workspace, as whiteboards::access has it.
    let confined = issue_credential(
        &setup.pool,
        IssueCredentialCommand {
            actor_id: owner,
            project_id: work.parse().expect("work"),
            client_name: "Confined".to_owned(),
            scopes: vec!["read".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("credential");
    let confined_session = initialize(app, &confined.token).await.expect("session");
    let opened = tool(
        app,
        &confined.token,
        &confined_session,
        "thing_open",
        json!({"kind": "whiteboard", "id": in_work}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let refused = tool(
        app,
        &confined.token,
        &confined_session,
        "thing_open",
        json!({"kind": "whiteboard", "id": across}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    assert!(!refused.to_string().contains("Team board"), "{refused}");

    // Whiteboards off in this workspace, on in the second: this workspace's
    // boards, across it or in a project, are hidden as things and in lists.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"whiteboards": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let hidden = call("thing_open", json!({"kind": "whiteboard", "id": across})).await;
    assert_eq!(hidden["outcome"], "not_found", "{hidden}");
    let (status, body) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/things/whiteboard/{across}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let shown = call("thing_open", json!({"kind": "whiteboard", "id": in_second})).await;
    assert_eq!(shown["outcome"], "ok", "{shown}");
    let listed = call("whiteboard_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let ids = listed["value"]["whiteboards"]
        .as_array()
        .expect("whiteboards")
        .iter()
        .map(|board| board["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![in_second.clone()], "{listed}");
    assert!(
        !ids.contains(&across) && !ids.contains(&in_work),
        "{listed}"
    );
}

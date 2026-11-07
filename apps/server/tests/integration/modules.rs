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

async fn switch(setup: &Setup, browser: &Browser, body: Value) -> (StatusCode, Value) {
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
    let (bea, _bea_browser) = add_person(&setup, &[&work, &direct], "bea").await;
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

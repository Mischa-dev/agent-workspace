//! AWD-75: aggregate lists and the inbox count only where their module is on.
//! The silent failure: an app connected for a whole account, or an agent,
//! listing what belongs to a workspace or project where the module is off,
//! because the module is on somewhere else.

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_code::{run, seeded, thread},
    mcp_task_board::seed_pull_request,
    mcp_tasks::{add_member, space},
    message_edits::create_channel,
    modules::{narrow_in_project, switch},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// A second workspace with `owner` as its owner.
async fn second_workspace(pool: &PgPool, owner: Uuid) -> Uuid {
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Second', $2)")
        .bind(second)
        .bind(format!("second-{}", second.simple()))
        .execute(pool)
        .await
        .expect("second workspace");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(second)
    .bind(owner)
    .execute(pool)
    .await
    .expect("second membership");
    second
}

/// The `field` of each item in a list.
fn each(list: &Value, field: &str) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item[field].as_str().map(str::to_owned))
        .collect()
}

async fn switched_off(setup: &Setup, body: Value) {
    let (status, changed) = switch(setup, &setup.browser, body).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
}

/// The reply request asking `agent` in `conversation`.
async fn request_of(pool: &PgPool, agent: Uuid, conversation: &str) -> String {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM reply_requests WHERE agent_actor_id = $1 AND conversation_id = $2::uuid",
    )
    .bind(agent)
    .bind(conversation)
    .fetch_one(pool)
    .await
    .expect("reply request")
    .to_string()
}

#[tokio::test]
async fn account_lists_leave_out_a_workspace_where_their_module_is_off() {
    let Some(setup) = setup("awd75_lists").await else {
        return;
    };
    let app = &setup.app;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    let second = second_workspace(&setup.pool, owner).await;
    let here = create_project(&setup, "AAA", "A work").await;
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
    let there = made["id"].as_str().expect("project").to_owned();
    for (project, title) in [(&here, "A errand"), (&there, "B errand")] {
        let key = format!("awd75-{title}").replace(' ', "-");
        let (status, made) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{project}/work-items"),
                Some(key.as_str()),
                json!({"title": title, "assigneeActorId": owner}),
            ),
        )
        .await;
        assert!(status.is_success(), "{status}: {made}");
    }
    create_agent(app, &setup.browser, &here, "Ada", "ada").await;
    create_agent(app, &setup.browser, &there, "Bo", "bo").await;
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("owner app");
    let call = |name: &'static str| {
        let token = token.clone();
        let session = session.clone();
        async move {
            let result = tool(app, &token, &session, name, json!({})).await;
            assert_eq!(result["outcome"], "ok", "{name}: {result}");
            result
        }
    };
    // Everything on: both workspaces' projects, tasks and agents.
    let listed = each(&call("projects_list").await["value"]["projects"], "name");
    assert!(
        listed.contains(&"A work".to_owned()) && listed.contains(&"B work".to_owned()),
        "{listed:?}"
    );
    let mine = each(
        &call("my_work").await["value"]["assigned"]["items"],
        "title",
    );
    assert!(
        mine.contains(&"A errand".to_owned()) && mine.contains(&"B errand".to_owned()),
        "{mine:?}"
    );
    let met = each(&call("agent_list").await["value"]["agents"], "name");
    assert!(
        met.contains(&"Ada".to_owned()) && met.contains(&"Bo".to_owned()),
        "{met:?}"
    );

    // Projects (and so Tasks) and Agents off here, on in the second
    // workspace: the account-wide lists keep only the second's.
    switched_off(
        &setup,
        json!({"switches": {"projects": false, "tasks": false, "agents": false}}),
    )
    .await;
    let listed = each(&call("projects_list").await["value"]["projects"], "name");
    assert!(!listed.contains(&"A work".to_owned()), "{listed:?}");
    assert!(listed.contains(&"B work".to_owned()), "{listed:?}");
    let mine = each(
        &call("my_work").await["value"]["assigned"]["items"],
        "title",
    );
    assert_eq!(mine, ["B errand"]);
    let met = each(&call("agent_list").await["value"]["agents"], "name");
    assert_eq!(met, ["Bo"]);
}

#[tokio::test]
async fn code_and_run_lists_leave_out_a_workspace_where_code_is_off() {
    let Some((setup, seed, _temporary)) = seeded("awd75_code_lists").await else {
        return;
    };
    let app = &setup.app;
    second_workspace(&setup.pool, seed.human_actor_id).await;
    // A run needs a prepared repository, as in mcp_code's tests.
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), version = version + 1 WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&setup.pool)
    .await
    .expect("prepared");
    let thread_id = thread(&setup.pool, &seed, seed.channel_id, "awd75-runs").await;
    let (run_id, _) = run(&setup.pool, &seed, thread_id, "awd75-runs").await;
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("owner app");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move {
            let result = tool(app, &token, &session, name, arguments).await;
            assert_eq!(result["outcome"], "ok", "{name}: {result}");
            result
        }
    };
    let repository = seed.repository_id.to_string();
    let run_id = run_id.to_string();
    let repositories = call("repository_list", json!({})).await;
    assert!(
        each(&repositories["value"]["repositories"], "id").contains(&repository),
        "{repositories}"
    );
    let runs = call("run_list", json!({"state": "all"})).await;
    assert!(
        each(&runs["value"]["runs"], "id").contains(&run_id),
        "{runs}"
    );

    // Code (and so Runs) off in the seed's workspace, on in the second:
    // neither the repository nor the run is listed any more.
    switched_off(&setup, json!({"switches": {"code": false, "runs": false}})).await;
    let repositories = call("repository_list", json!({})).await;
    assert!(
        !each(&repositories["value"]["repositories"], "id").contains(&repository),
        "{repositories}"
    );
    let runs = call("run_list", json!({"state": "all"})).await;
    assert!(
        !each(&runs["value"]["runs"], "id").contains(&run_id),
        "{runs}"
    );
}

#[tokio::test]
async fn an_agent_s_inbox_leaves_out_requests_where_channels_is_off() {
    let Some(setup) = setup("awd75_inbox").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    // The agent lives in the Messages space and is a member of Work.
    let (ada, ada_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Ada", "ada").await;
    add_member(&setup, &work, ada, "member").await;
    let channel = create_channel(&setup, &work, "general").await;
    let (status, tagged) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/channels/{channel}/messages"),
            Some("awd75-tag"),
            json!({"content": {"version": 1, "nodes": [
                {"type": "mention", "actorId": ada},
                {"type": "text", "text": " the release plan, please"}
            ]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tagged}");
    let (status, dm) = send(
        app,
        setup
            .browser
            .request("POST", "/api/v1/direct/dms", None, json!({"actorId": ada})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("dm").to_owned();
    let (status, said) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{dm}/messages"),
            Some("awd75-dm"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Lunch?"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{said}");
    // Bo, another agent in Work, is asked in the same channel, and in a DM
    // with the owner that Ada can't see.
    let (bo, _) = create_agent(app, &setup.browser, &space(&setup).await, "Bo", "bo").await;
    add_member(&setup, &work, bo, "member").await;
    let (status, tagged_bo) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/channels/{channel}/messages"),
            Some("awd75-tag-bo"),
            json!({"content": {"version": 1, "nodes": [
                {"type": "mention", "actorId": bo},
                {"type": "text", "text": " the release notes too"}
            ]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tagged_bo}");
    let (status, bo_dm) = send(
        app,
        setup
            .browser
            .request("POST", "/api/v1/direct/dms", None, json!({"actorId": bo})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{bo_dm}");
    let bo_dm = bo_dm["conversation"]["id"].as_str().expect("dm").to_owned();
    let (status, said) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{bo_dm}/messages"),
            Some("awd75-bo-dm"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Status?"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{said}");
    let session = initialize(app, &ada_token).await.expect("agent");
    let asked = |name: &'static str| {
        let token = ada_token.clone();
        let session = session.clone();
        async move {
            let result = tool(app, &token, &session, name, json!({})).await;
            assert_eq!(result["outcome"], "ok", "{name}: {result}");
            result
        }
    };
    let inbox = asked("inbox_list").await;
    let waiting = each(&inbox["value"]["requests"], "replyConversationId");
    assert!(
        waiting.contains(&channel) && waiting.contains(&dm),
        "{inbox}"
    );

    // Channels off, Messages on: the channel's request leaves the inbox and
    // the briefing, names and excerpt included; the DM's stays.
    switched_off(&setup, json!({"switches": {"channels": false}})).await;
    let inbox = asked("inbox_list").await;
    assert_eq!(
        each(&inbox["value"]["requests"], "replyConversationId"),
        vec![dm.clone()],
        "{inbox}"
    );
    assert!(!inbox.to_string().contains("release plan"), "{inbox}");
    let briefing = asked("briefing").await;
    assert_eq!(
        each(&briefing["value"]["replyRequests"], "replyConversationId"),
        vec![dm.clone()],
        "{briefing}"
    );
    assert!(!briefing.to_string().contains("release plan"), "{briefing}");
    // Nothing was deleted: the request is still there.
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM reply_requests WHERE agent_actor_id = $1 AND conversation_id = $2::uuid",
    )
    .bind(ada)
    .bind(&channel)
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(kept, 1);

    // reply_claim checks the request before the module. Ada's own channel
    // request is refused because Channels is off there. Bo's request in the
    // same channel, Bo's DM she can't see, and a request that doesn't exist
    // all answer not_found, saying nothing about modules.
    let claim = |request: String| {
        let token = ada_token.clone();
        let session = session.clone();
        async move {
            tool(
                app,
                &token,
                &session,
                "reply_claim",
                json!({"requestId": request}),
            )
            .await
        }
    };
    let own = claim(request_of(&setup.pool, ada, &channel).await).await;
    assert_eq!(own["outcome"], "permission_denied", "{own}");
    assert_eq!(own["retryCategory"], "module", "{own}");
    for request in [
        request_of(&setup.pool, bo, &channel).await,
        request_of(&setup.pool, bo, &bo_dm).await,
        Uuid::now_v7().to_string(),
    ] {
        let refused = claim(request).await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
        assert_ne!(refused["retryCategory"], "module", "{refused}");
        assert!(!refused.to_string().contains("Channels"), "{refused}");
    }
}

#[tokio::test]
async fn code_lists_keep_the_messages_space_while_code_is_on() {
    let Some(setup) = setup("awd75_code_direct").await else {
        return;
    };
    let app = &setup.app;
    // A second workspace keeps Code on somewhere, so the lists answer rather
    // than the tool being turned off everywhere.
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    second_workspace(&setup.pool, owner).await;
    // A repository and a pull request in the Messages space, which the Code
    // tools have always listed.
    let direct = space(&setup).await;
    let repository = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, enabled, trusted, preparation_state
        )
        VALUES ($1, $2::uuid, 9200000752, 'Fixture', 'awd75-direct',
                'https://github.com/Fixture/awd75-direct.git', 'main',
                '/nonexistent/awd75-direct.git', true, true, 'pending')
        "#,
    )
    .bind(repository)
    .bind(&direct)
    .execute(&setup.pool)
    .await
    .expect("repository");
    let (pull_request, _) = seed_pull_request(&setup, &direct, repository, 1).await;
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("owner app");
    let call = |name: &'static str| {
        let token = token.clone();
        let session = session.clone();
        async move {
            let result = tool(app, &token, &session, name, json!({})).await;
            assert_eq!(result["outcome"], "ok", "{name}: {result}");
            result
        }
    };
    let repository = repository.to_string();
    let pull_request = pull_request.to_string();

    // Code on: both are listed.
    let repositories = call("repository_list").await;
    assert!(
        each(&repositories["value"]["repositories"], "id").contains(&repository),
        "{repositories}"
    );
    let pull_requests = call("pull_request_list").await;
    assert!(
        each(&pull_requests["value"]["pullRequests"], "id").contains(&pull_request),
        "{pull_requests}"
    );

    // Code off in the workspace: neither is.
    switched_off(&setup, json!({"switches": {"code": false, "runs": false}})).await;
    let repositories = call("repository_list").await;
    assert!(
        !each(&repositories["value"]["repositories"], "id").contains(&repository),
        "{repositories}"
    );
    let pull_requests = call("pull_request_list").await;
    assert!(
        !each(&pull_requests["value"]["pullRequests"], "id").contains(&pull_request),
        "{pull_requests}"
    );
}

#[tokio::test]
async fn an_anchored_agent_s_lists_span_the_projects_it_reaches() {
    let Some(setup) = setup("awd75_anchored").await else {
        return;
    };
    let app = &setup.app;
    let home = create_project(&setup, "HOME", "Home base").await;
    let other = create_project(&setup, "OTHER", "Other work").await;
    // Ada is anchored in Home base, where her role turns Tasks off, and is a
    // member of Other work, where it's on.
    let (ada, ada_token) = create_agent(app, &setup.browser, &home, "Ada", "ada").await;
    add_member(&setup, &other, ada, "member").await;
    narrow_in_project(&setup, &home, ada, &["tasks"], "awd75-anchor").await;
    let owner = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner).await.expect("owner app");
    for (project, title) in [("HOME", "Anchored errand"), ("OTHER", "Elsewhere errand")] {
        let created = tool(
            app,
            &owner,
            &owner_session,
            "task_create",
            json!({"projectId": project, "title": title}),
        )
        .await;
        assert_eq!(created["outcome"], "applied", "{created}");
    }
    let session = initialize(app, &ada_token).await.expect("agent");
    // A list naming no project spans both and leaves out Home base's tasks.
    let listed = tool(app, &ada_token, &session, "task_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let text = listed.to_string();
    assert!(text.contains("Elsewhere errand"), "{listed}");
    assert!(!text.contains("Anchored errand"), "{listed}");
    // Naming Home base, Tasks is still off for her there.
    let here = tool(
        app,
        &ada_token,
        &session,
        "task_list",
        json!({"projectId": "HOME"}),
    )
    .await;
    assert_eq!(here["outcome"], "permission_denied", "{here}");
    assert_eq!(here["retryCategory"], "module", "{here}");
}

#[tokio::test]
async fn the_projects_list_says_which_modules_are_off_in_each_project() {
    let Some(setup) = setup("awd75_projects_off").await else {
        return;
    };
    let work = create_project(&setup, "AAA", "A work").await;
    let flag = |listed: &Value| {
        listed["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .find(|project| project["id"] == work.as_str())
            .map(|project| project["modulesOff"].clone())
            .expect("the project")
    };
    let (status, listed) = send(
        &setup.app,
        setup
            .browser
            .request("GET", "/api/v1/projects", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(flag(&listed), Value::Null, "{listed}");
    switched_off(
        &setup,
        json!({"switches": {"projects": false, "tasks": false, "agents": false}}),
    )
    .await;
    let (status, listed) = send(
        &setup.app,
        setup
            .browser
            .request("GET", "/api/v1/projects", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let off = flag(&listed);
    for module in ["projects", "tasks", "agents"] {
        assert!(
            off.as_array()
                .is_some_and(|off| off.iter().any(|entry| entry == module)),
            "{module}: {listed}"
        );
    }
    // The project on its own (as opening or switching to it answers) says
    // so too, so the rail doesn't get its tile back.
    let (status, single) = send(
        &setup.app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{single}");
    assert_eq!(single["modulesOff"], off, "{single}");
}

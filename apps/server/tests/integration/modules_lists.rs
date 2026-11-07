//! AWD-75: aggregate lists and the inbox count only where their module is on.
//! The silent failure: an app connected for a whole account, or an agent,
//! listing what belongs to a workspace or project where the module is off,
//! because the module is on somewhere else.

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_code::{run, seeded, thread},
    mcp_tasks::{add_member, space},
    message_edits::create_channel,
    modules::switch,
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
        [dm.clone()],
        "{inbox}"
    );
    assert!(!inbox.to_string().contains("release plan"), "{inbox}");
    let briefing = asked("briefing").await;
    assert_eq!(
        each(&briefing["value"]["replyRequests"], "replyConversationId"),
        [dm.clone()],
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
}

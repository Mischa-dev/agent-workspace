//! How people and agents work through MCP (docs/plans/agent-interface.md).
//!
//! These cover the parts where a bug would be silent and costly: an app
//! connected for a person's whole account reaches exactly what that person
//! can see, private conversations stay private in lists and search, revoking
//! a connection works at once, read-only means read-only, threads under a DM
//! stay inside it, and two agents can never hold the same task.

use agent_workspace_server::bootstrap::{BootstrapCommand, bootstrap_admin};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

pub(super) use super::support::{
    app::{
        Browser, PASSWORD, connect_app, create_agent, creates_once, initialize, login, send,
        test_app, test_app_with, tool,
    },
    db::schema_pool,
};

#[tokio::test]
async fn connected_apps_and_agents_see_exactly_what_they_should() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("agent_interface_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    let owner = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Interface Organization".to_owned()),
            organization_slug: Some(format!("interface-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let app = test_app(pool.clone(), &database_url);
    let browser = login(&app).await;

    // The Messages space, two agents in it, and a separate project.
    let space = send(
        &app,
        browser.request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (mara, mara_token) = create_agent(&app, &browser, &space, "Mara", "mara").await;
    let (rook, rook_token) = create_agent(&app, &browser, &space, "Rook", "rook").await;
    let (status, project) = send(
        &app,
        browser.request(
            "POST",
            "/api/v1/projects",
            Some("interface-project"),
            json!({"organizationId": owner.organization_id, "key": "WORK", "name": "Work"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project_id = project["id"].as_str().expect("project").to_owned();

    // The owner talks with Mara. Mara and Rook have a DM the owner isn't in.
    let (_, owner_dm) = send(
        &app,
        browser.request("POST", "/api/v1/direct/dms", None, json!({"actorId": mara})),
    )
    .await;
    let owner_dm = owner_dm["conversation"]["id"]
        .as_str()
        .expect("owner DM")
        .to_owned();
    let mara_session = initialize(&app, &mara_token).await.expect("Mara MCP");
    let private = tool(
        &app,
        &mara_token,
        &mara_session,
        "conversation_start",
        json!({"people": ["@rook"], "text": "Private note about the zeppelin budget"}),
    )
    .await;
    assert_eq!(private["outcome"], "ok", "{private}");
    let private_dm = private["value"]["conversation"]["id"]
        .as_str()
        .expect("private DM")
        .to_owned();
    assert!(!visible(&pool, &private_dm, owner.actor_id).await);

    // Connect an app for the owner's whole account, with full access.
    let codex = connect_app(&app, &browser, "Codex", "full").await;
    let session = initialize(&app, &codex).await.expect("Codex MCP");

    let projects = tool(&app, &codex, &session, "projects_list", json!({})).await;
    let names = projects["value"]["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .map(|project| project["name"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(names.contains(&"Work".to_owned()), "{projects}");
    assert!(names.contains(&"Messages".to_owned()), "{projects}");

    // Lists, opening, and search never reach the DM the owner isn't in.
    let listed = tool(&app, &codex, &session, "conversation_list", json!({})).await;
    let ids = listed["value"]["conversations"]
        .as_array()
        .expect("conversations")
        .iter()
        .map(|conversation| conversation["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(ids.contains(&owner_dm), "{listed}");
    assert!(!ids.contains(&private_dm), "{listed}");
    let opened = tool(
        &app,
        &codex,
        &session,
        "conversation_open",
        json!({"conversationId": private_dm}),
    )
    .await;
    assert_ne!(opened["outcome"], "ok", "{opened}");
    let found = tool(
        &app,
        &codex,
        &session,
        "search",
        json!({"query": "zeppelin"}),
    )
    .await;
    assert_eq!(found["outcome"], "ok", "{found}");
    assert_eq!(found["value"]["results"], json!([]), "{found}");
    let mara_found = tool(
        &app,
        &mara_token,
        &mara_session,
        "search",
        json!({"query": "zeppelin"}),
    )
    .await;
    assert_eq!(
        mara_found["value"]["results"].as_array().map(Vec::len),
        Some(1),
        "a member of the DM does find it: {mara_found}"
    );
    let rejected = tool(
        &app,
        &codex,
        &session,
        "message_send",
        json!({"conversationId": private_dm, "text": "let me in"}),
    )
    .await;
    assert_ne!(rejected["outcome"], "ok", "{rejected}");

    // Messages sent through the app say so, until the label is turned off.
    let sent = tool(
        &app,
        &codex,
        &session,
        "message_send",
        json!({"conversationId": owner_dm, "text": "Hello @mara from Codex", "clientRequestId": "codex-hello"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let message_id = sent["value"]["messageId"]
        .as_str()
        .expect("message")
        .to_owned();
    let timeline = send(
        &app,
        browser.request(
            "GET",
            &format!("/api/v1/conversations/{owner_dm}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await
    .1;
    let last = timeline["entries"]
        .as_array()
        .and_then(|entries| entries.last())
        .cloned()
        .expect("last entry");
    assert_eq!(last["message"]["author"]["id"], owner.actor_id.to_string());
    assert_eq!(last["message"]["via"], "Codex");
    assert_eq!(
        last["message"]["mentionedActors"][0]["id"],
        mara.to_string(),
        "@handles become mentions"
    );
    let apps = send(
        &app,
        browser.request("GET", "/api/v1/account/connected-apps", None, Value::Null),
    )
    .await
    .1;
    let codex_app = apps["apps"]
        .as_array()
        .and_then(|apps| apps.iter().find(|app| app["clientName"] == "Codex"))
        .cloned()
        .expect("Codex in connected apps");
    assert_eq!(codex_app["access"], "full");
    assert_eq!(codex_app["actingAs"], "you");
    let codex_grant = codex_app["id"].as_str().expect("grant").to_owned();
    let (status, _) = send(
        &app,
        browser.request(
            "PATCH",
            &format!("/api/v1/account/connected-apps/{codex_grant}"),
            None,
            json!({"showVia": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let timeline = send(
        &app,
        browser.request(
            "GET",
            &format!("/api/v1/conversations/{owner_dm}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await
    .1;
    assert!(
        timeline["entries"]
            .as_array()
            .and_then(|entries| entries.last())
            .is_some_and(|entry| entry["message"].get("via").is_none()),
        "the via label is hidden once the toggle is off"
    );

    // A thread under the DM: only DM members see it, it follows the DM's
    // reply rules, and its status and outcome can change.
    let thread = tool(
        &app,
        &codex,
        &session,
        "thread_start",
        json!({"conversationId": owner_dm, "fromMessageId": message_id}),
    )
    .await;
    assert_eq!(thread["outcome"], "ok", "{thread}");
    assert_eq!(thread["value"]["created"], true);
    let thread_id = thread["value"]["id"].as_str().expect("thread").to_owned();
    let again = tool(
        &app,
        &codex,
        &session,
        "thread_start",
        json!({"conversationId": owner_dm, "fromMessageId": message_id}),
    )
    .await;
    assert_eq!(again["value"]["id"], thread_id, "same message, same thread");
    assert!(visible(&pool, &thread_id, mara).await);
    assert!(!visible(&pool, &thread_id, rook).await);
    let (status, _) = send(
        &app,
        browser.request(
            "POST",
            &format!("/api/v1/conversations/{thread_id}/messages"),
            Some("thread-ask"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Mara, can you look?"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT required FROM reply_requests WHERE conversation_id = $1 AND agent_actor_id = $2",
        )
        .bind(thread_id.parse::<Uuid>().expect("thread UUID"))
        .bind(mara)
        .fetch_one(&pool)
        .await
        .expect("reply request in DM thread"),
        "a message in a DM's thread asks the DM's agent, like the DM itself"
    );
    let settled = tool(
        &app,
        &codex,
        &session,
        "thread_update",
        json!({"threadId": thread_id, "status": "decided", "outcome": "Going with plan B"}),
    )
    .await;
    assert_eq!(settled["outcome"], "ok", "{settled}");
    assert_eq!(settled["value"]["status"], "Decided");
    let threads = send(
        &app,
        browser.request(
            "GET",
            &format!("/api/v1/conversations/{owner_dm}/threads"),
            None,
            Value::Null,
        ),
    )
    .await
    .1;
    assert_eq!(threads["threads"][0]["outcome"], "Going with plan B");
    // The chip shows who replied: here only the owner has.
    assert_eq!(
        threads["threads"][0]["participants"]
            .as_array()
            .expect("participants")
            .iter()
            .map(|actor| actor["id"].as_str().expect("participant").to_owned())
            .collect::<Vec<_>>(),
        vec![owner.actor_id.to_string()]
    );

    // Retrying thread_start with an opening message doesn't post it twice.
    for _ in 0..2 {
        let retried = tool(
            &app,
            &codex,
            &session,
            "thread_start",
            json!({"conversationId": owner_dm, "fromMessageId": message_id, "text": "Picking this up"}),
        )
        .await;
        assert_eq!(retried["outcome"], "ok", "{retried}");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM messages WHERE conversation_id = $1 AND plain_text = 'Picking this up'",
        )
        .bind(thread_id.parse::<Uuid>().expect("thread UUID"))
        .fetch_one(&pool)
        .await
        .expect("opening messages"),
        1
    );

    // Tasks: the app creates one in the Work project; two agents race to
    // claim it and exactly one wins.
    for agent in [mara, rook] {
        let (status, added) = send(
            &app,
            browser.request(
                "POST",
                &format!("/api/v1/projects/{project_id}/members"),
                None,
                json!({"actorId": agent, "role": "member"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{added}");
    }
    let created = tool(
        &app,
        &codex,
        &session,
        "task_create",
        json!({
            "projectId": project_id,
            "title": "Ship the zeppelin",
            "description": "Steps:\n- inflate\n- launch",
            "clientRequestId": "task-1"
        }),
    )
    .await;
    assert_eq!(created["outcome"], "applied", "{created}");
    let task_id = created["workItemId"].as_str().expect("task").to_owned();
    // A fresh task and a person's briefing have empty lists, which the results
    // leave out; `tool` checks that their outputSchemas allow that.
    let opened = tool(
        &app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": task_id}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(
        opened["value"]["task"]["description"], "Steps:\n- inflate\n- launch",
        "{opened}"
    );
    let briefing = tool(&app, &codex, &session, "briefing", json!({})).await;
    assert_eq!(briefing["outcome"], "ok", "{briefing}");
    let unclaimed = tool(
        &app,
        &codex,
        &session,
        "project_open",
        json!({"projectId": project_id}),
    )
    .await;
    assert_eq!(unclaimed["outcome"], "ok", "{unclaimed}");
    let rook_session = initialize(&app, &rook_token).await.expect("Rook MCP");
    let (mara_claim, rook_claim) = tokio::join!(
        tool(
            &app,
            &mara_token,
            &mara_session,
            "task_claim",
            json!({"taskId": task_id})
        ),
        tool(
            &app,
            &rook_token,
            &rook_session,
            "task_claim",
            json!({"taskId": task_id})
        ),
    );
    let outcomes = [mara_claim["outcome"].clone(), rook_claim["outcome"].clone()];
    assert_eq!(
        outcomes.iter().filter(|outcome| **outcome == "ok").count(),
        1,
        "exactly one agent holds the task: {mara_claim} {rook_claim}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == "conflict")
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_claims")
            .fetch_one(&pool)
            .await
            .expect("claims"),
        1
    );

    // Pages keep every version, and a restore brings an old one back.
    let page = tool(
        &app,
        &codex,
        &session,
        "page_write",
        json!({"projectId": project_id, "title": "Theming", "body": "Use tokens.", "isGuide": true}),
    )
    .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    let page_id = page["value"]["id"].as_str().expect("page").to_owned();
    let edited = tool(
        &app,
        &codex,
        &session,
        "page_write",
        json!({"pageId": page_id, "body": "Use raw hex.", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(edited["value"]["version"], 2, "{edited}");
    let stale = tool(
        &app,
        &codex,
        &session,
        "page_write",
        json!({"pageId": page_id, "body": "Lost update", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let restored = tool(
        &app,
        &codex,
        &session,
        "page_write",
        json!({"pageId": page_id, "restoreVersion": 1}),
    )
    .await;
    assert_eq!(restored["value"]["body"], "Use tokens.");
    assert_eq!(restored["value"]["version"], 3);
    let overview = tool(
        &app,
        &codex,
        &session,
        "project_open",
        json!({"projectId": project_id}),
    )
    .await;
    assert_eq!(
        overview["value"]["guide"]["body"], "Use tokens.",
        "{overview}"
    );
    // A link to the page works as its ID, even from an app anchored elsewhere.
    let by_link = tool(
        &app,
        &codex,
        &session,
        "page_get",
        json!({"pageId": format!("http://127.0.0.1:3100/projects/{project_id}/pages/{page_id}")}),
    )
    .await;
    assert_eq!(by_link["outcome"], "ok", "{by_link}");

    // Tasks link only to things in their own project, so a link can't show
    // another project's conversation or page to the task's readers.
    let (status, side) = send(
        &app,
        browser.request(
            "POST",
            "/api/v1/projects",
            Some("interface-side-project"),
            json!({"organizationId": owner.organization_id, "key": "SIDE", "name": "Side"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{side}");
    let side_page = tool(
        &app,
        &codex,
        &session,
        "page_write",
        json!({"projectId": side["id"], "title": "Side notes", "body": "Elsewhere."}),
    )
    .await;
    assert_eq!(side_page["outcome"], "ok", "{side_page}");
    for (key, target) in [
        ("pageId", side_page["value"]["id"].clone()),
        ("conversationId", json!(owner_dm)),
        ("messageId", json!(message_id)),
    ] {
        let mut arguments = json!({"taskId": task_id});
        arguments[key] = target;
        let linked = tool(&app, &codex, &session, "task_link", arguments).await;
        assert_eq!(linked["outcome"], "not_found", "{key}: {linked}");
    }
    let linked = tool(
        &app,
        &codex,
        &session,
        "task_link",
        json!({"taskId": task_id, "pageId": page_id}),
    )
    .await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_links")
            .fetch_one(&pool)
            .await
            .expect("links"),
        1
    );

    // A decision from a private DM stays private.
    let decided = tool(
        &app,
        &mara_token,
        &mara_session,
        "decision_record",
        json!({"conversationId": private_dm, "title": "Zeppelin budget is capped"}),
    )
    .await;
    assert_eq!(decided["outcome"], "ok", "{decided}");
    let owner_search = tool(&app, &codex, &session, "search", json!({"query": "capped"})).await;
    assert_eq!(
        owner_search["value"]["results"],
        json!([]),
        "{owner_search}"
    );

    // A read-only connection can look but not change anything.
    let reader = connect_app(&app, &browser, "Reader", "read").await;
    let reader_session = initialize(&app, &reader).await.expect("Reader MCP");
    let read_only = tool(
        &app,
        &reader,
        &reader_session,
        "message_send",
        json!({"conversationId": owner_dm, "text": "should not send"}),
    )
    .await;
    assert_eq!(read_only["outcome"], "permission_denied", "{read_only}");
    let unread_kept = tool(
        &app,
        &reader,
        &reader_session,
        "mark_read",
        json!({"conversationId": owner_dm}),
    )
    .await;
    assert_eq!(unread_kept["outcome"], "permission_denied", "{unread_kept}");

    // Revoking the connection stops it at once.
    let (status, _) = send(
        &app,
        browser.request(
            "POST",
            &format!("/api/v1/account/connected-apps/{codex_grant}/revoke"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        initialize(&app, &codex).await.is_none(),
        "revoked app is refused"
    );

    drop(app);
    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&admin, &schema)
        .await
        .expect("drop schema");
}

#[tokio::test]
async fn recording_a_decision_again_records_it_once() {
    let Some(setup) = super::mcp_admin::setup("decision_retry").await else {
        return;
    };
    let work = super::mcp_admin::create_project(&setup, "WORK", "Work").await;
    let space = super::mcp_tasks::space(&setup).await;
    let (rook, token) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    super::mcp_tasks::add_member(&setup, &work, rook, "member").await;
    let session = initialize(&setup.app, &token).await.expect("agent MCP");
    let key = "decision-retry-1";
    creates_once(
        &setup.app,
        &setup.pool,
        &token,
        &session,
        "decision_record",
        json!({"projectId": work, "title": "Ship on Fridays", "clientRequestId": key}),
        json!({"projectId": work, "title": "Never ship on Fridays", "clientRequestId": key}),
        "SELECT count(*) FROM project_decisions",
    )
    .await;
}

async fn visible(pool: &PgPool, conversation: &str, actor: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
        .bind(conversation.parse::<Uuid>().expect("conversation UUID"))
        .bind(actor)
        .fetch_one(pool)
        .await
        .expect("visibility")
}

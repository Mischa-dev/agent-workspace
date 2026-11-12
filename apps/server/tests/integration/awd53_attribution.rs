//! AWD-53 criterion 10: every version of a page says who wrote it, and an
//! agent's says which of its sessions (a run, or its runner's own session)
//! it came from, naming that session's conversation only to people who can
//! see the conversation.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
};

#[tokio::test]
async fn a_pages_history_names_the_agent_session_behind_each_version() {
    let Some(setup) = setup("awd53_attribution").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let session = initialize(app, &mara_token).await.expect("Mara MCP");

    // Written with no session going: the version names Mara and nothing else.
    let written = tool(
        app,
        &mara_token,
        &session,
        "page_write",
        json!({"projectId": "WORK", "title": "Mara's notes", "body": "One"}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let page = written["value"]["id"].as_str().expect("page").to_owned();

    // Mara starts working in a channel, as a runner binding its session does.
    let (status, channel) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("drafting-channel"),
            json!({"name": "drafting"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let conversation: Uuid = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .parse()
        .expect("id");
    let project_id: Uuid = work.parse().expect("project id");
    sqlx::query(
        "INSERT INTO agent_sessions (id, organization_id, project_id, conversation_id,
             agent_actor_id, source, harness, external_ref)
         SELECT $1, organization_id, id, $2, $3, 'runner', 'claude_code', 'session-1'
         FROM projects WHERE id = $4",
    )
    .bind(Uuid::now_v7())
    .bind(conversation)
    .bind(mara)
    .bind(project_id)
    .execute(&setup.pool)
    .await
    .expect("session");

    let appended = tool(
        app,
        &mara_token,
        &session,
        "page_edit",
        json!({"pageId": page, "edits": [{"op": "append", "markdown": "Two"}]}),
    )
    .await;
    assert_eq!(appended["value"]["version"], 2, "{appended}");

    let (status, read) = send(
        app,
        setup
            .browser
            .request("GET", &format!("/api/v1/pages/{page}"), None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let revisions = read["revisions"].as_array().expect("revisions");
    let version = |number: i64| {
        revisions
            .iter()
            .find(|revision| revision["version"] == number)
            .unwrap_or_else(|| panic!("version {number} in {read}"))
            .clone()
    };
    let latest = version(2);
    assert_eq!(latest["author"], "Mara", "{latest}");
    assert_eq!(latest["sessionHarness"], "claude_code", "{latest}");
    assert_eq!(
        latest["sessionConversationId"],
        json!(conversation),
        "{latest}"
    );
    assert_eq!(latest["sessionConversationKind"], "channel", "{latest}");
    let first = version(1);
    assert!(first.get("sessionHarness").is_none(), "{first}");

    // Mara's next run in the same channel reuses the session row; the version
    // written earlier keeps naming what wrote it.
    sqlx::query("UPDATE agent_sessions SET harness = 'codex' WHERE agent_actor_id = $1")
        .bind(mara)
        .execute(&setup.pool)
        .await
        .expect("next run");
    let (_, read) = send(
        app,
        setup
            .browser
            .request("GET", &format!("/api/v1/pages/{page}"), None, Value::Null),
    )
    .await;
    let kept = read["revisions"]
        .as_array()
        .expect("revisions")
        .iter()
        .find(|revision| revision["version"] == 2)
        .expect("version 2")
        .clone();
    assert_eq!(kept["sessionHarness"], "claude_code", "{kept}");

    // Once the channel is private to people the reader isn't among, the
    // version still says a session wrote it, but not where.
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1")
        .bind(conversation)
        .execute(&setup.pool)
        .await
        .expect("make it private");
    sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND actor_id <> $2")
        .bind(conversation)
        .bind(mara)
        .execute(&setup.pool)
        .await
        .expect("leave only Mara");
    let (_, read) = send(
        app,
        setup
            .browser
            .request("GET", &format!("/api/v1/pages/{page}"), None, Value::Null),
    )
    .await;
    let hidden = read["revisions"]
        .as_array()
        .expect("revisions")
        .iter()
        .find(|revision| revision["version"] == 2)
        .expect("version 2")
        .clone();
    assert_eq!(hidden["sessionHarness"], "claude_code", "{hidden}");
    for field in [
        "sessionConversationId",
        "sessionConversationName",
        "sessionRunId",
    ] {
        assert!(hidden.get(field).is_none(), "{field} in {hidden}");
    }

    // A person's edits never carry a session.
    let (status, saved) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/pages/{page}"),
            None,
            json!({"body": "One\n\nTwo\n\nThree", "expectedVersion": 2}),
        ),
    )
    .await;
    assert!(status.is_success(), "{saved}");
    let unattributed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM project_page_revisions
         WHERE page_id = $1::uuid AND version = 3 AND agent_session_id IS NULL",
    )
    .bind(&page)
    .fetch_one(&setup.pool)
    .await
    .expect("version 3");
    assert_eq!(unattributed, 1);
}

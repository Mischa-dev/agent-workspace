//! K2.5: decisions as operations, the template slice. A retry with the same
//! request id records one decision, from the web (its `Idempotency-Key`) and
//! from MCP (`clientRequestId`); a decision from a private conversation the
//! caller can't see is `not_found` on every edge; the v1 route answers with
//! the same body as before it became a wrapper.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

async fn decisions_titled(pool: &sqlx::PgPool, title: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM project_decisions WHERE title = $1")
        .bind(title)
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn decisions_are_recorded_once_and_stay_private() {
    let Some(setup) = setup("k2_decisions").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (_, mara) = create_agent(app, &setup.browser, &work, "Mara", "mara").await;
    let (_, _rook) = create_agent(app, &setup.browser, &work, "Rook", "rook").await;
    let mara_session = initialize(app, &mara).await.expect("Mara MCP");

    // The web app's route: the same body as before, recorded once per request id.
    let route = format!("/api/v1/projects/{work}/decisions");
    let body = json!({"title": "Ship on Fridays", "detail": "Small changes only."});
    let (status, first) = send(
        app,
        setup
            .browser
            .request("POST", &route, Some("web-d-1"), body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let mut fields = first
        .as_object()
        .expect("decision")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    fields.sort();
    assert_eq!(
        fields,
        [
            "createdAt",
            "decidedBy",
            "detail",
            "id",
            "projectId",
            "sourceConversationId",
            "sourceConversationKind",
            "sourceConversationName",
            "sourceMessageId",
            "status",
            "supersededByDecisionId",
            "title",
            "via",
        ],
        "{first}"
    );
    assert_eq!(first["projectId"], work.as_str());
    assert_eq!(first["status"], "active");
    let (status, again) = send(
        app,
        setup.browser.request("POST", &route, Some("web-d-1"), body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["id"], first["id"], "{again}");
    assert_eq!(decisions_titled(&setup.pool, "Ship on Fridays").await, 1);

    // The list route reads through decision_list and still says who may record.
    let (status, listed) = send(app, setup.browser.request("GET", &route, None, Value::Null)).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["canRecord"], true, "{listed}");
    assert_eq!(listed["decisions"][0]["id"], first["id"], "{listed}");

    // MCP: an agent's retry with the same clientRequestId is answered once.
    let arguments = json!({
        "projectId": "WORK",
        "title": "Keep the board small",
        "clientRequestId": "mcp-d-1",
    });
    let recorded = tool(
        app,
        &mara,
        &mara_session,
        "decision_record",
        arguments.clone(),
    )
    .await;
    assert_eq!(recorded["outcome"], "ok", "{recorded}");
    let replayed = tool(app, &mara, &mara_session, "decision_record", arguments).await;
    assert_eq!(replayed["outcome"], "ok", "{replayed}");
    assert_eq!(replayed["replayed"], true, "{replayed}");
    assert_eq!(replayed["value"]["id"], recorded["value"]["id"]);
    assert_eq!(
        decisions_titled(&setup.pool, "Keep the board small").await,
        1
    );

    // Mara and Rook have a DM the owner isn't in: the owner can't decide from it.
    let private = tool(
        app,
        &mara,
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
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/decision_record",
            Some("web-private-1"),
            json!({"sourceConversationId": private_dm, "title": "Zeppelin budget is capped"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &route,
            Some("web-private-2"),
            json!({"sourceConversationId": private_dm, "title": "Zeppelin budget is capped"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(
        decisions_titled(&setup.pool, "Zeppelin budget is capped").await,
        0
    );

    // Mara, who is in it, can: the decision is as private as the DM.
    let decided = tool(
        app,
        &mara,
        &mara_session,
        "decision_record",
        json!({"conversationId": private_dm, "title": "Zeppelin budget is capped"}),
    )
    .await;
    assert_eq!(decided["outcome"], "ok", "{decided}");
    let space = decided["value"]["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (status, owner_view) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/decision_list",
            None,
            json!({"spaceId": space}),
        ),
    )
    .await;
    // The owner sees none of it: not the space's decision, or not the space.
    assert!(
        (status == StatusCode::OK && owner_view["value"]["decisions"] == json!([]))
            || owner_view["outcome"] == "not_found",
        "{owner_view}"
    );
}

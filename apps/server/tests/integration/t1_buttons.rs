//! T1.21: button fields. The silent, costly failures: a press that runs as
//! the button's maker instead of whoever pressed it; a press by someone
//! without the operation's permission that half-runs; a retried press that
//! runs twice; a button that names a field and breaks when it's renamed.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::WorldBuilder,
};

async fn decisions_titled(pool: &sqlx::PgPool, title: &str) -> Vec<uuid::Uuid> {
    sqlx::query_scalar(
        "SELECT decided_by_actor_id FROM project_decisions WHERE title = $1 ORDER BY created_at",
    )
    .bind(title)
    .fetch_all(pool)
    .await
    .expect("decisions")
}

/// A v2 operation from a person's browser.
async fn op(
    world: &super::support::world::Workspace,
    who: &super::support::callers::Person,
    name: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    send(
        &world.app,
        who.browser
            .request("POST", &format!("/api/v2/ops/{name}"), key, body),
    )
    .await
}

#[tokio::test]
async fn buttons_run_as_the_presser_and_refuse_whole() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("vera", "viewer")
        .agent("rook", "member")
        .connected_app("Codex")
        .build("t1_buttons")
        .await
    else {
        return;
    };
    let codex = &world.apps["Codex"];
    let tool = |name: &'static str, arguments: Value| Call::Tool { name, arguments };

    let (_, created) = codex
        .call(
            &world.app,
            tool(
                "collection_write",
                json!({
                    "projectId": "WORK",
                    "name": "Orders",
                    "titleName": "Order",
                    "fields": [
                        {"name": "Stage", "type": "single_choice", "options": [{"name": "Open"}, {"name": "Shipped"}]},
                        {"name": "Notes", "type": "text"},
                        {"name": "Decide", "type": "button", "button": {
                            "label": "Mark shipped",
                            "steps": [{"operation": "decision_record", "arguments": {
                                "projectId": "{{space.id}}",
                                "title": "Shipped {{Order}} from {{Stage}} (#{{record.number}})",
                            }}],
                        }},
                        {"name": "Broken", "type": "button", "button": {
                            "steps": [{"operation": "decision_record", "arguments": {
                                "projectId": "{{space.id}}",
                                "title": "{{Notes}}",
                            }}],
                        }},
                    ],
                }),
            ),
        )
        .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let collection = created["value"]["collection"]["id"]
        .as_str()
        .expect("collection")
        .to_owned();
    let (_, described) = codex
        .call(
            &world.app,
            tool(
                "collection_describe",
                json!({"projectId": "WORK", "collection": "Orders"}),
            ),
        )
        .await;
    let field_id = |name: &str| {
        described["value"]["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|field| field["name"] == name)
            .and_then(|field| field["id"].as_str())
            .unwrap_or_else(|| panic!("no {name}: {described}"))
            .to_owned()
    };
    let decide = field_id("Decide");
    let broken = field_id("Broken");

    let (_, written) = codex
        .call(
            &world.app,
            tool(
                "record_write",
                json!({"projectId": "WORK", "collection": "Orders", "records": [
                    {"title": "Apples", "fields": {"Stage": "Open"}},
                ]}),
            ),
        )
        .await;
    assert_eq!(written["outcome"], "ok", "{written}");

    // A button has no value to write.
    let (_, refused) = codex
        .call(
            &world.app,
            tool(
                "record_write",
                json!({"projectId": "WORK", "collection": "Orders", "records": [
                    {"record": "#1", "fields": {"Decide": "x"}},
                ]}),
            ),
        )
        .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    // A member can press it; a viewer sees why not.
    let lena = &world.people["lena"];
    let vera = &world.people["vera"];
    let (status, can) = op(
        &world,
        lena,
        "button_get",
        None,
        json!({"collection": collection, "field": decide}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{can}");
    assert_eq!(can["value"]["canPress"], true, "{can}");
    assert_eq!(can["value"]["button"]["label"], "Mark shipped");
    let (status, cannot) = op(
        &world,
        vera,
        "button_get",
        None,
        json!({"collection": collection, "field": decide}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cannot}");
    assert_eq!(cannot["value"]["canPress"], false, "{cannot}");
    assert!(
        cannot["value"]["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "{cannot}"
    );

    // Pressing runs as the presser, with the record filled in, once per
    // request id.
    let press = json!({"collection": collection, "field": decide, "record": "#1"});
    let title = "Shipped Apples from Open (#1)";
    let (status, pressed) = op(&world, lena, "button_press", Some("press-1"), press.clone()).await;
    assert_eq!(status, StatusCode::OK, "{pressed}");
    assert_eq!(pressed["value"]["steps"][0]["operation"], "decision_record");
    let (status, again) = op(&world, lena, "button_press", Some("press-1"), press.clone()).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(decisions_titled(world.pool(), title).await, [lena.actor]);

    // A viewer's press is refused before anything runs.
    let (status, denied) = op(&world, vera, "button_press", Some("press-v"), press.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(decisions_titled(world.pool(), title).await.len(), 1);

    // An agent presses it as itself.
    let rook = &world.agents["rook"];
    let (_, by_agent) = rook
        .call(
            &world.app,
            tool(
                "button_press",
                json!({"collection": collection, "field": decide, "record": "1", "clientRequestId": "rook-1"}),
            ),
        )
        .await;
    assert_eq!(by_agent["outcome"], "ok", "{by_agent}");
    assert_eq!(
        decisions_titled(world.pool(), title).await,
        [lena.actor, rook.actor]
    );

    // A step that's refused says which, and why.
    let (status, failed) = op(
        &world,
        lena,
        "button_press",
        Some("press-b"),
        json!({"collection": collection, "field": broken, "record": "#1"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{failed}");
    assert!(
        failed["message"]
            .as_str()
            .is_some_and(|text| text.contains("Step 1")),
        "{failed}"
    );

    // Renaming a field it fills in keeps it working, shown by the new name.
    let (_, renamed) = codex
        .call(
            &world.app,
            tool(
                "collection_write",
                json!({"projectId": "WORK", "collection": "Orders", "fields": [{"field": "Stage", "name": "Phase"}]}),
            ),
        )
        .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    let (_, described) = codex
        .call(
            &world.app,
            tool(
                "collection_describe",
                json!({"projectId": "WORK", "collection": "Orders"}),
            ),
        )
        .await;
    let shown = described["value"]["definition"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["name"] == "Decide")
        .cloned()
        .expect("Decide");
    assert_eq!(
        shown["button"]["steps"][0]["arguments"]["title"],
        "Shipped {{record.title}} from {{Phase}} (#{{record.number}})",
        "{shown}"
    );
    let (status, pressed) = op(&world, lena, "button_press", Some("press-2"), press).await;
    assert_eq!(status, StatusCode::OK, "{pressed}");
    assert_eq!(decisions_titled(world.pool(), title).await.len(), 3);

    // Mistakes are refused with what to fix.
    for (button, part) in [
        (
            json!({"steps": [{"operation": "nope_create"}]}),
            "no operation called",
        ),
        (
            json!({"steps": [{"operation": "decision_list"}]}),
            "only reads",
        ),
        (
            json!({"steps": [{"operation": "decision_record", "arguments": {"title": "{{Missing}}"}}]}),
            "nothing called",
        ),
        (json!({"steps": []}), "1 to 3"),
    ] {
        let (_, refused) = codex
            .call(
                &world.app,
                tool(
                    "collection_write",
                    json!({"projectId": "WORK", "collection": "Orders", "fields": [{"name": "Bad", "type": "button", "button": button}]}),
                ),
            )
            .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
        assert!(refused.to_string().contains(part), "{part}: {refused}");
    }
    world.finish().await;
}

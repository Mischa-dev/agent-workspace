//! T1.17: formula fields. The silent, costly failures: a formula that stops
//! working when a field it reads is renamed; results that don't filter or
//! sort as their type; an error for one record that hides it from "is
//! empty", or breaks the page; a value written into a formula field; and
//! results left stale after the formula changes.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

/// The titles of a page's records, in order.
fn titles(value: &Value) -> Vec<String> {
    value["value"]["records"]
        .as_array()
        .unwrap_or_else(|| panic!("no records: {value}"))
        .iter()
        .map(|record| record["title"].as_str().expect("title").to_owned())
        .collect()
}

/// A record's value for `field`, by title.
fn value_of(page: &Value, title: &str, field: &str) -> Value {
    page["value"]["records"]
        .as_array()
        .expect("records")
        .iter()
        .find(|record| record["title"] == title)
        .unwrap_or_else(|| panic!("no {title}: {page}"))["fields"][field]
        .clone()
}

#[tokio::test]
async fn formulas_filter_sort_survive_renames_and_show_errors() {
    let Some(setup) = setup("t1_formulas").await else {
        return;
    };
    let app = &setup.app;
    let project_id = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let created = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": "Orders",
            "titleName": "Order",
            "fields": [
                {"name": "Price", "type": "number"},
                {"name": "Qty", "type": "number"},
                {"name": "Stage", "type": "single_choice", "options": [{"name": "Open"}, {"name": "Closed"}]},
                {"name": "Total", "type": "formula", "formula": "{Price} * {Qty}"},
                {"name": "Each", "type": "formula", "formula": "{Price} / {Qty}"},
                {"name": "Label", "type": "formula", "formula": "{Order} & \" (\" & {Stage} & \")\""},
                {"name": "Big", "type": "formula", "formula": "{Total} >= 10"},
            ],
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let collection = created["value"]["collection"]["id"]
        .as_str()
        .expect("collection")
        .to_owned();

    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Orders",
            "records": [
                {"title": "Apples", "fields": {"Price": 10, "Qty": 2, "Stage": "Open"}},
                {"title": "Pears", "fields": {"Price": 5, "Qty": 0, "Stage": "Closed"}},
                {"title": "Plums", "fields": {"Price": 1, "Qty": 7}},
            ],
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");

    let query = |extra: Value| {
        let mut arguments = json!({"projectId": "WORK", "collection": "Orders", "limit": 50});
        for (key, value) in extra.as_object().expect("object") {
            arguments[key] = value.clone();
        }
        call("record_query", arguments)
    };
    let all = query(json!({})).await;
    assert_eq!(all["outcome"], "ok", "{all}");
    assert_eq!(value_of(&all, "Apples", "Total"), json!(20));
    assert_eq!(value_of(&all, "Apples", "Each"), json!(5));
    assert_eq!(value_of(&all, "Apples", "Label"), json!("Apples (Open)"));
    assert_eq!(value_of(&all, "Apples", "Big"), json!(true));
    assert_eq!(value_of(&all, "Plums", "Label"), json!("Plums ()"));
    // An error is that record's value, and the others still read.
    assert_eq!(
        value_of(&all, "Pears", "Each"),
        json!({"error": "Divide by zero"})
    );
    assert_eq!(value_of(&all, "Pears", "Total"), json!(0));

    // Filter and sort by results, as numbers.
    let big = query(json!({
        "filters": [{"field": "Total", "op": "gt", "values": ["5"]}],
        "sort": [{"field": "Total", "direction": "desc"}],
    }))
    .await;
    assert_eq!(big["outcome"], "ok", "{big}");
    assert_eq!(titles(&big), ["Apples", "Plums"]);
    let ascending = query(json!({"sort": [{"field": "Total"}]})).await;
    assert_eq!(titles(&ascending), ["Pears", "Plums", "Apples"]);
    // An error filters as empty.
    let empty = query(json!({"filters": [{"field": "Each", "op": "empty"}]})).await;
    assert_eq!(titles(&empty), ["Pears"], "{empty}");
    let not_empty = query(json!({"filters": [{"field": "Each", "op": "notEmpty"}]})).await;
    assert_eq!(titles(&not_empty), ["Apples", "Plums"]);
    // A true-or-false result filters as a checkbox.
    let checked =
        query(json!({"filters": [{"field": "Big", "op": "is", "values": ["true"]}]})).await;
    assert_eq!(titles(&checked), ["Apples"], "{checked}");
    // Grouped by its result.
    let grouped = query(json!({"groupBy": "Big"})).await;
    assert_eq!(grouped["outcome"], "ok", "{grouped}");

    // The editor's preview checks a formula and works it out on a record,
    // saving nothing.
    let preview = call(
        "formula_preview",
        json!({"collection": collection, "formula": "{Price} * {Qty} + 1", "record": 1}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    assert_eq!(preview["value"]["value"], json!(21), "{preview}");
    assert_eq!(preview["value"]["result"], "number");
    assert_eq!(preview["value"]["record"]["title"], "Apples");
    let problem = call(
        "formula_preview",
        json!({"collection": collection, "formula": "{Price} *"}),
    )
    .await;
    assert!(
        problem["value"]["problem"]
            .as_str()
            .is_some_and(|text| text.contains("ends too soon")),
        "{problem}"
    );

    // Nobody writes a formula's value.
    let refused = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Orders", "records": [{"title": "Figs", "fields": {"Total": 3}}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    // Renaming a field it reads keeps it working, and shows the new name.
    let renamed = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Orders", "fields": [{"field": "Price", "name": "Cost"}]}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    let changed = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Orders", "records": [{"record": "#1", "fields": {"Cost": 11}}]}),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    let after = query(json!({})).await;
    assert_eq!(value_of(&after, "Apples", "Total"), json!(22));
    let described = call(
        "collection_describe",
        json!({"projectId": "WORK", "collection": "Orders"}),
    )
    .await;
    let total = described["value"]["definition"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["name"] == "Total")
        .cloned()
        .expect("Total");
    assert_eq!(total["formula"], "{Cost} * {Qty}", "{described}");

    // Mistakes are refused with what to fix.
    for (formula, part) in [
        ("{Missing} + 1", "no field called"),
        ("SUMM(1)", "no function called SUMM"),
        ("{Total} + 1 +", "ends too soon"),
    ] {
        let refused = call(
            "collection_write",
            json!({"projectId": "WORK", "collection": "Orders", "fields": [{"name": "Broken", "type": "formula", "formula": formula}]}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
        assert!(refused.to_string().contains(part), "{formula}: {refused}");
    }
    // A loop through other formulas is refused.
    let looped = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Orders", "fields": [{"field": "Total", "formula": "IF({Big}, 1, 0)"}]}),
    )
    .await;
    assert_eq!(looped["outcome"], "invalid_request", "{looped}");
    assert!(looped.to_string().contains("use itself"), "{looped}");

    // A changed formula queues the recalculation of every record, which
    // brings the stored results up to date.
    let rewritten = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Orders", "fields": [{"field": "Total", "formula": "{Cost} + {Qty}"}]}),
    )
    .await;
    assert_eq!(rewritten["outcome"], "ok", "{rewritten}");
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind = 'collections.recompute_formulas' \
         AND state = 'queued' AND payload->>'collectionId' = $1",
    )
    .bind(&collection)
    .fetch_one(&setup.pool)
    .await
    .expect("jobs");
    assert_eq!(queued, 1, "one queued run covers every change");
    crate::support::as_job(
        setup.organization_id,
        agent_workspace_server::collections::computed::recompute_collection(
            &setup.pool,
            project_id.parse::<Uuid>().expect("project"),
            collection.parse::<Uuid>().expect("collection"),
        ),
    )
    .await
    .expect("recompute");
    let recomputed = query(json!({})).await;
    assert_eq!(value_of(&recomputed, "Apples", "Total"), json!(13));
    assert_eq!(value_of(&recomputed, "Plums", "Total"), json!(8));
    // And what reads it follows: 13 >= 10, 8 isn't.
    assert_eq!(value_of(&recomputed, "Apples", "Big"), json!(true));
    assert_eq!(value_of(&recomputed, "Plums", "Big"), Value::Null);

    // The web app makes one too, and sees results with the record.
    let (status, field) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project_id}/collections/{collection}/fields"),
            Some("t1-formula-web"),
            json!({"name": "Double", "type": "formula", "formula": "{Qty} * 2"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{field}");
    assert_eq!(field["formula"]["result"], "number", "{field}");
    let double = field["id"].as_str().expect("field").to_owned();
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project_id}/collections/{collection}/fields"),
            Some("t1-formula-web-bad"),
            json!({"name": "Bad", "type": "formula", "formula": "{Qty} *"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    // A new record gets it at once, with the write.
    let written = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Orders", "records": [{"title": "Kiwis", "fields": {"Qty": 4}}]}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let (status, page) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{project_id}/collections/{collection}/records?limit=50"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let kiwis = page["records"]
        .as_array()
        .expect("records")
        .iter()
        .find(|record| record["title"] == "Kiwis")
        .cloned()
        .expect("Kiwis");
    assert_eq!(kiwis["values"][&double], json!(8), "{kiwis}");
}

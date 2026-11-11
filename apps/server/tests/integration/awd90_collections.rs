//! AWD-90: collections, the record types a project makes for itself. The
//! silent, costly failures: a query that reads every record to find a page,
//! or pages that skip or repeat records; group counts that don't add up; a
//! definition that comes out different in another project; a batch of
//! records half written; a viewer writing records or someone without
//! `collections.build` changing fields and views; an ID from another project
//! matching (a collection, field, view or record, or a person, task or page
//! a value names); and open screens not hearing about a change.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::add_person,
};

/// The numbers of a page's records, in order.
fn numbers(value: &Value) -> Vec<i64> {
    value["value"]["records"]
        .as_array()
        .unwrap_or_else(|| panic!("no records: {value}"))
        .iter()
        .map(|record| record["number"].as_i64().expect("number"))
        .collect()
}

async fn get(setup: &Setup, browser: &Browser, uri: &str) -> (StatusCode, Value) {
    send(&setup.app, browser.request("GET", uri, None, Value::Null)).await
}

async fn write(
    setup: &Setup,
    browser: &Browser,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let key = format!("awd90-{}", Uuid::now_v7().simple());
    send(&setup.app, browser.request(method, uri, Some(&key), body)).await
}

async fn collection_events(setup: &Setup, collection: &str) -> Vec<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM outbox_events WHERE event_type = 'collection.updated' AND payload->>'collectionId' = $1 ORDER BY event_cursor",
    )
    .bind(collection)
    .fetch_all(&setup.pool)
    .await
    .expect("collection events")
}

#[tokio::test]
async fn a_thousand_records_page_filter_sort_and_group_on_the_server() {
    let Some(setup) = setup("awd90_query").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "WORK", "Work").await;
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
            "name": "Playtests",
            "titleName": "Finding",
            "fields": [
                {"name": "Severity", "type": "single_choice", "options": [{"name": "Low"}, {"name": "High", "color": "#dc2626"}]},
                {"name": "Score", "type": "number"},
                {"name": "Fixed", "type": "checkbox"},
            ],
            "clientRequestId": "playtests",
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    assert_eq!(created["value"]["created"], true);
    let collection = created["value"]["collection"]["id"]
        .as_str()
        .expect("collection")
        .to_owned();

    // 1,000 records, 100 a call. Every third is High; Score is the index.
    for batch in 0..10 {
        let records = (0..100)
            .map(|offset| {
                let index = batch * 100 + offset;
                json!({
                    "title": format!("Level {index} crash"),
                    "fields": {
                        "Severity": if index % 3 == 0 { "High" } else { "Low" },
                        "Score": index,
                        "Fixed": index % 2 == 0,
                    },
                })
            })
            .collect::<Vec<_>>();
        let written = call(
            "record_write",
            json!({
                "projectId": "WORK",
                "collection": "Playtests",
                "records": records,
                "clientRequestId": format!("batch-{batch}"),
            }),
        )
        .await;
        assert_eq!(written["outcome"], "ok", "batch {batch}: {written}");
        assert_eq!(written["value"]["created"], 100);
    }
    // A retry with the same clientRequestId adds nothing.
    let replay = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtests",
            "records": (0..100).map(|offset| json!({
                "title": format!("Level {offset} crash"),
                "fields": {"Severity": if offset % 3 == 0 { "High" } else { "Low" }, "Score": offset, "Fixed": offset % 2 == 0},
            })).collect::<Vec<_>>(),
            "clientRequestId": "batch-0",
        }),
    )
    .await;
    assert_eq!(replay["value"]["idempotentReplay"], true, "{replay}");

    // The High ones, highest score first, 150 a page: every one once, in
    // order, and never more than a page at a time.
    let query = json!({
        "projectId": "WORK",
        "collection": "Playtests",
        "filters": [{"field": "Severity", "op": "is", "values": ["High"]}],
        "sort": [{"field": "Score", "direction": "desc"}],
        "fields": ["Score"],
        "limit": 150,
    });
    let mut seen = Vec::new();
    let mut cursor = None::<String>;
    let mut pages = 0;
    loop {
        let mut arguments = query.clone();
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = call("record_query", arguments).await;
        assert_eq!(page["outcome"], "ok", "{page}");
        assert_eq!(page["value"]["total"], 334, "{page}");
        let records = page["value"]["records"].as_array().expect("records");
        assert!(records.len() <= 150);
        for record in records {
            assert_eq!(
                record["fields"].as_object().expect("fields").len(),
                1,
                "only Score: {record}"
            );
            seen.push(record["fields"]["Score"].as_i64().expect("score"));
        }
        pages += 1;
        match page["value"]["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    let expected = (0..1000)
        .rev()
        .filter(|index| index % 3 == 0)
        .collect::<Vec<i64>>();
    assert_eq!(seen, expected);

    // A cursor reads back only with its own query.
    let first = call("record_query", query.clone()).await;
    let next = first["value"]["nextCursor"].as_str().expect("cursor");
    let mut other = query.clone();
    other["sort"] = json!([{"field": "Score", "direction": "asc"}]);
    other["cursor"] = json!(next);
    let refused = call("record_query", other).await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    // Group counts cover every record, in the options' order.
    let grouped = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Playtests", "groupBy": "Severity", "limit": 5}),
    )
    .await;
    assert_eq!(grouped["outcome"], "ok", "{grouped}");
    assert_eq!(
        grouped["value"]["groups"],
        json!([{"name": "Low", "count": 666}, {"name": "High", "count": 334}])
    );
    assert_eq!(grouped["value"]["total"], 1000);
    assert_eq!(grouped["value"]["records"][0]["group"], "Low");
    let fixed = call(
        "record_query",
        json!({
            "projectId": "WORK",
            "collection": "Playtests",
            "filters": [
                {"field": "Fixed", "op": "is", "values": ["true"]},
                {"field": "Score", "op": "gte", "values": ["990"]},
            ],
            "groupBy": "Fixed",
        }),
    )
    .await;
    assert_eq!(fixed["value"]["total"], 5, "{fixed}");
    assert_eq!(numbers(&fixed), vec![991, 993, 995, 997, 999]);

    // Every word must be in the title (or a value); a number also finds
    // its record.
    let found = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Playtests", "search": "level 42 crash", "limit": 200}),
    )
    .await;
    let records = found["value"]["records"].as_array().expect("records");
    assert_eq!(records.len(), 21, "{found}");
    assert!(numbers(&found).contains(&43) && numbers(&found).contains(&42));
    for record in records {
        assert!(
            record["title"].as_str().expect("title").contains("42") || record["number"] == 42,
            "{record}"
        );
    }
    let found = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Playtests", "search": "level 4 nothing"}),
    )
    .await;
    assert_eq!(found["value"]["total"], 0, "{found}");
    let found = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Playtests", "search": "#7"}),
    )
    .await;
    assert!(numbers(&found).contains(&7), "{found}");

    // All or nothing: one bad record leaves the other unwritten.
    let refused = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtests",
            "records": [
                {"title": "Fine"},
                {"title": "Not fine", "fields": {"Severity": "Catastrophic"}},
            ],
        }),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    assert_eq!(refused["field"], "records[1].fields.Severity", "{refused}");
    let total = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Playtests", "limit": 1}),
    )
    .await;
    assert_eq!(total["value"]["total"], 1000);
    let too_many = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtests",
            "records": (0..101).map(|_| json!({"title": "More"})).collect::<Vec<_>>(),
        }),
    )
    .await;
    assert_eq!(too_many["outcome"], "invalid_request", "{too_many}");

    // A stale version is a conflict; history keeps before and after.
    let changed = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtests",
            "records": [{"record": "#1", "fields": {"Severity": "Low", "Score": null}, "expectedVersion": 1}],
        }),
    )
    .await;
    assert_eq!(changed["value"]["changed"], 1, "{changed}");
    let stale = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtests",
            "records": [{"record": "#1", "title": "Again", "expectedVersion": 1}],
        }),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let same = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtests",
            "records": [{"record": "#1", "fields": {"Severity": "Low"}}],
        }),
    )
    .await;
    assert_eq!(same["value"]["unchanged"], 1, "{same}");
    let read = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Playtests", "records": ["#1"], "includeHistory": true}),
    )
    .await;
    assert_eq!(read["value"]["records"][0]["fields"]["Severity"], "Low");
    assert!(read["value"]["records"][0]["fields"].get("Score").is_none());
    let history = read["value"]["history"][0]["history"]
        .as_array()
        .expect("history");
    assert_eq!(history.len(), 2, "{read}");
    assert_eq!(history[0]["action"], "updated");
    assert_eq!(history[1]["action"], "created");
    let changes = history[0]["changes"].as_array().expect("changes");
    assert!(
        changes.iter().any(|change| change["name"] == "Severity"
            && change["before"] == "High"
            && change["after"] == "Low"),
        "{changes:?}"
    );
    assert!(
        changes
            .iter()
            .any(|change| change["name"] == "Score" && change["before"] == 0),
        "{changes:?}"
    );

    // Open screens hear about each change, by record, and not about one
    // that changed nothing.
    let events = collection_events(&setup, &collection).await;
    // Created, ten batches of 100, and the one change.
    assert_eq!(events.len(), 12, "{events:?}");
    let record_one = read["value"]["records"][0]["id"].as_str().expect("id");
    assert_eq!(events[11]["recordIds"], json!([record_one]));
    assert_eq!(events[1]["recordIds"].as_array().expect("ids").len(), 100);
}

/// A page that ends on a record with long values still has a next page:
/// sorts compare the first characters of text, so the cursor stays small.
#[tokio::test]
async fn records_with_long_values_page_through_when_sorted_by_them() {
    let Some(setup) = setup("awd90_long_sorts").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "WORK", "Work").await;
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
            "name": "Notes",
            "fields": [{"name": "Body", "type": "text"}, {"name": "Steps", "type": "text"}],
            "clientRequestId": "notes",
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    // The longest values a record takes, in the widest characters, alike
    // but for the last one.
    let long = |last: char| format!("{}{last}", "😁".repeat(4_999));
    let records = ['c', 'a', 'b']
        .into_iter()
        .enumerate()
        .map(|(index, last)| {
            json!({
                "title": format!("{}{index}", "😁".repeat(299)),
                "fields": {"Body": long(last), "Steps": long(last)},
            })
        })
        .collect::<Vec<_>>();
    let written = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Notes", "records": records, "clientRequestId": "long"}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    for sort in [
        json!([{"field": "Body"}]),
        json!([{"field": "Body"}, {"field": "Steps", "direction": "desc"}, {"field": "title"}]),
    ] {
        let mut seen = Vec::new();
        let mut cursor = None::<String>;
        loop {
            let mut arguments = json!({
                "projectId": "WORK",
                "collection": "Notes",
                "sort": sort,
                "fields": ["Body"],
                "limit": 1,
            });
            if let Some(cursor) = &cursor {
                arguments["cursor"] = json!(cursor);
            }
            let page = call("record_query", arguments).await;
            assert_eq!(page["outcome"], "ok", "{sort}: {page}");
            seen.extend(numbers(&page));
            assert!(seen.len() <= 3, "{sort}: {seen:?}");
            match page["value"]["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2, 3], "{sort}");
    }
}

/// collection_write with the version collection_describe gave is refused,
/// with nothing kept, when the collection changed since, even when all it
/// changes is fields or views. A write that changes nothing still passes.
#[tokio::test]
async fn a_definition_from_an_old_version_conflicts_instead_of_overwriting() {
    let Some(setup) = setup("awd90_stale_definition").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "WORK", "Work").await;
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
            "name": "Builds",
            "fields": [{"name": "Platform", "type": "text"}],
            "clientRequestId": "builds",
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let read = created["value"]["collection"]["version"]
        .as_i64()
        .expect("version");
    // Someone else adds a field in between.
    let other = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Builds",
            "fields": [{"name": "Branch", "type": "text"}],
            "clientRequestId": "branch",
        }),
    )
    .await;
    assert_eq!(other["outcome"], "ok", "{other}");
    let current = other["value"]["collection"]["version"]
        .as_i64()
        .expect("version");
    assert!(current > read, "{other}");

    for (index, change) in [
        json!({"fields": [{"name": "Notes", "type": "text"}]}),
        json!({"views": [{"name": "Mine"}]}),
        json!({"description": "What we ship", "fields": [{"name": "Notes", "type": "text"}]}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut arguments = json!({
            "projectId": "WORK",
            "collection": "Builds",
            "expectedVersion": read,
            "clientRequestId": format!("stale-{index}"),
        });
        for (key, value) in change.as_object().expect("change") {
            arguments[key] = value.clone();
        }
        let stale = call("collection_write", arguments).await;
        assert_eq!(stale["outcome"], "conflict", "{change}: {stale}");
    }
    let described = call(
        "collection_describe",
        json!({"projectId": "WORK", "collection": "Builds"}),
    )
    .await;
    assert_eq!(described["outcome"], "ok", "{described}");
    let names = |list: &str| {
        described["value"][list]
            .as_array()
            .expect("list")
            .iter()
            .map(|entry| entry["name"].as_str().expect("name").to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("fields"), vec!["Platform", "Branch"], "{described}");
    assert!(!names("views").contains(&"Mine".to_owned()), "{described}");
    assert_eq!(described["value"]["collection"]["version"], current);

    // Writing what's there already changes nothing, so the old version passes.
    let same = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Builds",
            "expectedVersion": read,
            "fields": [{"name": "Platform", "type": "text"}],
            "clientRequestId": "same",
        }),
    )
    .await;
    assert_eq!(same["outcome"], "ok", "{same}");
    assert_eq!(same["value"]["changes"], json!([]), "{same}");
    // With the version it has now, the change lands.
    let fresh = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Builds",
            "expectedVersion": current,
            "fields": [{"name": "Notes", "type": "text"}],
            "clientRequestId": "fresh",
        }),
    )
    .await;
    assert_eq!(fresh["outcome"], "ok", "{fresh}");
    assert_eq!(
        fresh["value"]["changes"],
        json!(["field \"Notes\" added"]),
        "{fresh}"
    );
}

#[tokio::test]
async fn a_definition_made_from_another_projects_description_comes_out_the_same() {
    let Some(setup) = setup("awd90_definition").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "WORK", "Work").await;
    create_project(&setup, "SIDE", "Side").await;
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
            "name": "Builds",
            "description": "Every build we hand to testers",
            "titleName": "Version",
            "fields": [
                {"name": "Channel", "type": "single_choice", "options": [{"name": "Beta", "color": "#2563eb"}, {"name": "Stable"}]},
                {"name": "Platforms", "type": "multiple_choice", "options": [{"name": "Windows"}, {"name": "Mac"}]},
                {"name": "Size", "type": "number", "description": "In megabytes"},
                {"name": "Shipped", "type": "date"},
                {"name": "Owner", "type": "person"},
                {"name": "Ticket", "type": "link", "target": "task"},
                {"name": "Notes", "type": "link", "target": "page"},
                {"name": "Store page", "type": "url"},
                {"name": "Signed", "type": "checkbox"},
            ],
            "views": [
                {"name": "All builds"},
                {
                    "name": "Mine in beta",
                    "filters": [
                        {"field": "Channel", "op": "is", "values": ["Beta"]},
                        {"field": "Owner", "op": "is", "values": ["me"]},
                        {"field": "Size", "op": "lt", "values": ["500"]},
                    ],
                    "sort": [{"field": "Shipped", "direction": "desc"}, {"field": "title"}],
                    "groupBy": "Channel",
                    "columns": [{"field": "Size", "width": 120}, {"field": "Notes", "hidden": true}],
                    "titleWidth": 240,
                },
            ],
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let described = call(
        "collection_describe",
        json!({"projectId": "WORK", "collection": "Builds"}),
    )
    .await;
    assert_eq!(described["outcome"], "ok", "{described}");
    let definition = described["value"]["definition"].clone();
    assert_eq!(definition["fields"].as_array().expect("fields").len(), 9);
    assert_eq!(
        definition["views"][1]["filters"][1],
        json!({"field": "Owner", "op": "is", "values": ["me"]})
    );

    // The description, as is, makes the same collection in SIDE.
    let mut arguments = definition.clone();
    arguments["projectId"] = json!("SIDE");
    let copied = call("collection_write", arguments.clone()).await;
    assert_eq!(copied["outcome"], "ok", "{copied}");
    let side = call(
        "collection_describe",
        json!({"projectId": "SIDE", "collection": "Builds"}),
    )
    .await;
    assert_eq!(side["value"]["definition"], definition);

    // Writing it again changes nothing; a change by name changes just that.
    arguments["collection"] = json!("Builds");
    arguments.as_object_mut().expect("arguments").remove("name");
    let again = call("collection_write", arguments.clone()).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["changes"], json!([]), "{again}");
    let renamed = call(
        "collection_write",
        json!({
            "projectId": "SIDE",
            "collection": "Builds",
            "fields": [{"field": "Size", "name": "Size in MB"}, {"name": "Crashes", "type": "number"}],
            "views": [{"name": "All builds", "remove": true}],
        }),
    )
    .await;
    assert_eq!(
        renamed["value"]["changes"],
        json!([
            "field \"Size\" renamed to \"Size in MB\"",
            "field \"Crashes\" added",
            "view \"All builds\" removed",
        ]),
        "{renamed}"
    );
    // A view that named Size still does, by its new name.
    let side = call(
        "collection_describe",
        json!({"projectId": "SIDE", "collection": "Builds"}),
    )
    .await;
    assert_eq!(
        side["value"]["definition"]["views"][0]["filters"][2]["field"],
        "Size in MB"
    );
    // A field's type never changes.
    let retyped = call(
        "collection_write",
        json!({"projectId": "SIDE", "collection": "Builds", "fields": [{"name": "Crashes", "type": "text"}]}),
    )
    .await;
    assert_eq!(retyped["outcome"], "invalid_request", "{retyped}");
}

#[tokio::test]
async fn viewers_read_and_only_builders_change_fields_and_views() {
    let Some(setup) = setup("awd90_roles").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let base = format!("/api/v1/projects/{work}/collections");
    let (status, created) = write(
        &setup,
        &setup.browser,
        "POST",
        &base,
        json!({
            "name": "Bugs",
            "fields": [{"name": "Severity", "type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]}],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["views"][0]["name"], "All records");
    let collection = created["collection"]["id"].as_str().expect("id").to_owned();
    let view = created["views"][0]["id"].as_str().expect("view").to_owned();
    let field = created["fields"][0]["id"]
        .as_str()
        .expect("field")
        .to_owned();
    let uri = format!("{base}/{collection}");

    // A viewer reads everything and changes nothing.
    let (vera, vera_browser) = add_person(&setup, &[&work], "vera").await;
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(vera)
    .execute(&setup.pool)
    .await
    .expect("vera views");
    let (status, read) = get(&setup, &vera_browser, &uri).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["canEditRecords"], false);
    assert_eq!(read["canBuild"], false);
    for (method, path, body) in [
        ("POST", format!("{uri}/records"), json!({"title": "Mine"})),
        (
            "POST",
            format!("{uri}/fields"),
            json!({"name": "Mine", "type": "text"}),
        ),
        ("POST", base.clone(), json!({"name": "Mine"})),
    ] {
        let (status, body) = write(&setup, &vera_browser, method, &path, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "viewer {method} {path}: {body}"
        );
    }

    // A role with "Edit records" but not "Build collections" writes records,
    // and can't touch fields, views or collections.
    let keepers = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO workspace_roles (id, organization_id, key, base_role, name, permissions)
        VALUES (gen_random_uuid(), $1, 'record-keepers', 'member', 'Record keepers',
                ARRAY['projects.view', 'tasks.view', 'channels.post', 'collections.edit'])
        RETURNING id
        "#,
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("record keepers");
    let (kim, kim_browser) = add_person(&setup, &[&work], "kim").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    for actor in [kim, mara] {
        sqlx::query(
            "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(project)
        .bind(actor)
        .bind(keepers)
        .execute(&setup.pool)
        .await
        .expect("keeps records");
    }
    let (status, read) = get(&setup, &kim_browser, &uri).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["canEditRecords"], true);
    assert_eq!(read["canBuild"], false);
    let (status, record) = write(
        &setup,
        &kim_browser,
        "POST",
        &format!("{uri}/records"),
        json!({"title": "Crash", "values": {"Severity": "High"}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{record}");
    let record_id = record["record"]["id"].as_str().expect("record").to_owned();
    let (status, changed) = write(
        &setup,
        &kim_browser,
        "PATCH",
        &format!("{uri}/records/{record_id}"),
        json!({"expectedVersion": 1, "archived": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["record"]["archived"], true);
    for (method, path, body) in [
        (
            "POST",
            format!("{uri}/fields"),
            json!({"name": "Mine", "type": "text"}),
        ),
        (
            "PATCH",
            format!("{uri}/fields/{field}"),
            json!({"expectedVersion": 1, "name": "Mine"}),
        ),
        ("POST", format!("{uri}/views"), json!({"name": "Mine"})),
        (
            "PATCH",
            format!("{uri}/views/{view}"),
            json!({"expectedVersion": 1, "name": "Mine"}),
        ),
        ("DELETE", format!("{uri}/views/{view}"), Value::Null),
        (
            "PATCH",
            uri.clone(),
            json!({"expectedVersion": 1, "name": "Mine"}),
        ),
        ("POST", base.clone(), json!({"name": "Mine"})),
    ] {
        let (status, body) = write(&setup, &kim_browser, method, &path, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "keeper {method} {path}: {body}"
        );
    }

    // The same for an agent holding the role.
    let session = initialize(app, &mara_token).await.expect("Mara MCP");
    let listed = tool(
        app,
        &mara_token,
        &session,
        "collection_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["canBuild"], false);
    let written = tool(
        app,
        &mara_token,
        &session,
        "record_write",
        json!({"projectId": "WORK", "collection": "Bugs", "records": [{"title": "Found by Mara", "fields": {"Severity": "Low"}}]}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let refused = tool(
        app,
        &mara_token,
        &session,
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "fields": [{"name": "Mine", "type": "text"}]}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let fields = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM field_definitions WHERE collection_id = $1::uuid",
    )
    .bind(&collection)
    .fetch_one(&setup.pool)
    .await
    .expect("fields");
    assert_eq!(fields, 1, "nobody without collections.build added a field");
}

#[tokio::test]
async fn ids_from_another_project_never_match() {
    let Some(setup) = setup("awd90_projects").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let side = create_project(&setup, "SIDE", "Side").await;
    // Sam is only in SIDE.
    add_person(&setup, &[&side], "sam").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let definition = json!({
        "name": "Bugs",
        "fields": [
            {"name": "Tester", "type": "person"},
            {"name": "Build", "type": "link", "target": "task"},
            {"name": "Spec", "type": "link", "target": "page"},
        ],
        "views": [{"name": "Everything"}],
    });
    let mut ids = Vec::new();
    for key in ["WORK", "SIDE"] {
        let mut arguments = definition.clone();
        arguments["projectId"] = json!(key);
        let created = call("collection_write", arguments).await;
        assert_eq!(created["outcome"], "ok", "{created}");
        let task = call(
            "task_create",
            json!({"projectId": key, "title": "Ship it", "clientRequestId": format!("task-{key}")}),
        )
        .await;
        assert_eq!(task["outcome"], "applied", "{task}");
        let page = call(
            "page_write",
            json!({"projectId": key, "title": "Spec", "body": "What it does"}),
        )
        .await;
        let page = page["value"]["id"].as_str().expect("page").to_owned();
        let written = call(
            "record_write",
            json!({
                "projectId": key,
                "collection": "Bugs",
                "records": [{"title": "Crash", "fields": {"Tester": "me", "Build": format!("{key}-1"), "Spec": page}}],
            }),
        )
        .await;
        assert_eq!(written["outcome"], "ok", "{written}");
        let described = call(
            "collection_describe",
            json!({"projectId": key, "collection": "Bugs"}),
        )
        .await;
        ids.push(json!({
            "collection": described["value"]["collection"]["id"],
            "field": described["value"]["fields"][0]["id"],
            "view": described["value"]["views"][0]["id"],
            "record": written["value"]["records"][0]["id"],
            "task": sqlx::query_scalar::<_, Uuid>(
                "SELECT item.id FROM work_items item JOIN projects project ON project.id = item.project_id WHERE project.project_key = $1",
            )
            .bind(key)
            .fetch_one(&setup.pool)
            .await
            .expect("task"),
            "page": page,
        }));
    }
    let (here, there) = (&ids[0], &ids[1]);
    let s = |value: &Value| value.as_str().expect("id").to_owned();

    // SIDE's collection, field, view and record, named in WORK.
    let base = format!("/api/v1/projects/{work}/collections");
    let mine = format!("{base}/{}", s(&here["collection"]));
    for uri in [
        format!("{base}/{}", s(&there["collection"])),
        format!("{base}/{}/records", s(&there["collection"])),
        format!("{mine}/records/{}", s(&there["record"])),
        format!("{mine}/records?view={}", s(&there["view"])),
    ] {
        let (status, body) = get(&setup, &setup.browser, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}: {body}");
    }
    let (status, body) = get(
        &setup,
        &setup.browser,
        &format!("{mine}/records?groupBy={}", s(&there["field"])),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    for (method, uri, body) in [
        (
            "PATCH",
            format!("{mine}/fields/{}", s(&there["field"])),
            json!({"expectedVersion": 1, "name": "Mine"}),
        ),
        (
            "PATCH",
            format!("{mine}/views/{}", s(&there["view"])),
            json!({"expectedVersion": 1, "name": "Mine"}),
        ),
        (
            "DELETE",
            format!("{mine}/views/{}", s(&there["view"])),
            Value::Null,
        ),
        (
            "PATCH",
            format!("{mine}/records/{}", s(&there["record"])),
            json!({"expectedVersion": 1, "title": "Mine"}),
        ),
    ] {
        let (status, body) = write(&setup, &setup.browser, method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {body}");
    }

    // Values and filters naming SIDE's person, task or page are refused.
    for fields in [
        json!({"Tester": "@sam"}),
        json!({"Build": "SIDE-1"}),
        json!({"Build": there["task"]}),
        json!({"Spec": there["page"]}),
    ] {
        let refused = call(
            "record_write",
            json!({"projectId": "WORK", "collection": "Bugs", "records": [{"title": "Elsewhere", "fields": fields}]}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{fields}: {refused}");
    }
    for (filter, outcome) in [
        (
            json!({"field": "Spec", "op": "is", "values": [there["page"]]}),
            "invalid_request",
        ),
        (
            json!({"field": "Tester", "op": "is", "values": ["@sam"]}),
            "invalid_request",
        ),
        (
            json!({"field": there["field"], "op": "notEmpty"}),
            "invalid_request",
        ),
    ] {
        let refused = call(
            "record_query",
            json!({"projectId": "WORK", "collection": "Bugs", "filters": [filter]}),
        )
        .await;
        assert_eq!(refused["outcome"], outcome, "{filter}: {refused}");
    }
    for (arguments, outcome) in [
        (json!({"collection": there["collection"]}), "not_found"),
        (
            json!({"collection": "Bugs", "view": there["view"]}),
            "not_found",
        ),
        (
            json!({"collection": "Bugs", "records": [there["record"]]}),
            "not_found",
        ),
    ] {
        let mut arguments = arguments;
        arguments["projectId"] = json!("WORK");
        let refused = call("record_query", arguments.clone()).await;
        assert_eq!(refused["outcome"], outcome, "{arguments}: {refused}");
    }
    let refused = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Bugs", "records": [{"record": there["record"], "title": "Mine"}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    let refused = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "views": [{"view": there["view"], "name": "Mine"}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");

    // WORK's own still match, and nothing of SIDE's changed.
    let found = call(
        "record_query",
        json!({
            "projectId": "WORK",
            "collection": "Bugs",
            "filters": [
                {"field": "Build", "op": "is", "values": ["WORK-1"]},
                {"field": "Spec", "op": "is", "values": [here["page"]]},
                {"field": "Tester", "op": "is", "values": ["me"]},
            ],
        }),
    )
    .await;
    assert_eq!(found["value"]["total"], 1, "{found}");
    let side_record =
        sqlx::query_scalar::<_, String>("SELECT title FROM collection_records WHERE id = $1::uuid")
            .bind(s(&there["record"]))
            .fetch_one(&setup.pool)
            .await
            .expect("side record");
    assert_eq!(side_record, "Crash");
}

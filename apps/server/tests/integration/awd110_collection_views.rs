//! AWD-110: boards, galleries and calendars of a collection. The silent,
//! costly failures: a board column that shows records from another column
//! (or misses some, or pages wrongly); a calendar month that leaks records
//! from outside it; a board or calendar saved without the field it needs;
//! and a definition that loses a view's layout on the way to another project.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
};

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
    let key = format!("awd110-{}", Uuid::now_v7().simple());
    send(&setup.app, browser.request(method, uri, Some(&key), body)).await
}

fn titles(page: &Value) -> Vec<String> {
    page["records"]
        .as_array()
        .unwrap_or_else(|| panic!("no records: {page}"))
        .iter()
        .map(|record| record["title"].as_str().expect("title").to_owned())
        .collect()
}

#[tokio::test]
async fn board_columns_and_calendar_months_read_only_their_own_records() {
    let Some(setup) = setup("awd110_views").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    create_project(&setup, "COPY", "Copy").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    // A board and a calendar written without their fields take the first
    // that fits; a gallery needs none.
    let created = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": "Builds",
            "fields": [
                {"name": "Notes", "type": "text"},
                {"name": "Stage", "type": "single_choice", "options": [{"name": "Queued"}, {"name": "Testing"}, {"name": "Shipped"}]},
                {"name": "Ships", "type": "date"},
                {"name": "Approved", "type": "checkbox"},
            ],
            "views": [
                {"name": "Pipeline", "type": "board"},
                {"name": "Schedule", "type": "calendar"},
                {"name": "Cards", "type": "gallery"},
                {"name": "Sign-off", "type": "board", "groupBy": "Approved"},
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
    let views = described["value"]["definition"]["views"].clone();
    let view = |name: &str| {
        views
            .as_array()
            .expect("views")
            .iter()
            .find(|view| view["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {views}"))
            .clone()
    };
    assert_eq!(view("Pipeline")["type"], "board");
    assert_eq!(view("Pipeline")["groupBy"], "Stage");
    assert_eq!(view("Schedule")["type"], "calendar");
    assert_eq!(view("Schedule")["dateField"], "Ships");
    assert_eq!(view("Cards")["type"], "gallery");
    assert_eq!(view("Sign-off")["groupBy"], "Approved");

    // The definition makes the same views in another project.
    let mut definition = described["value"]["definition"].clone();
    definition["projectId"] = json!("COPY");
    let copied = call("collection_write", definition).await;
    assert_eq!(copied["outcome"], "ok", "{copied}");
    let copy = call(
        "collection_describe",
        json!({"projectId": "COPY", "collection": "Builds"}),
    )
    .await;
    assert_eq!(copy["value"]["definition"]["views"], views, "{copy}");

    // A calendar needs a date field; a board a field that groups.
    let refused = call(
        "collection_write",
        json!({"projectId": "WORK", "name": "Plain", "fields": [{"name": "Notes", "type": "text"}], "views": [{"name": "When", "type": "calendar"}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let refused = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Builds", "views": [{"view": "Schedule", "dateField": "Notes"}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let refused = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Builds", "views": [{"view": "Pipeline", "type": "table"}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    // 120 builds: 60 queued, 40 testing, 20 with no stage. Build n ships
    // on day n of 2026-11 to 2027-01 (days 1 to 28 of each month).
    let records = (0..120)
        .map(|index| {
            let mut fields = json!({
                "Ships": format!("{}-{:02}", ["2026-11", "2026-12", "2027-01"][index % 3], index % 28 + 1),
                "Approved": index % 4 == 0,
            });
            if index < 60 {
                fields["Stage"] = json!("Queued");
            } else if index < 100 {
                fields["Stage"] = json!("Testing");
            }
            json!({"title": format!("Build {index:03}"), "fields": fields})
        })
        .collect::<Vec<_>>();
    for (batch, chunk) in records.chunks(100).enumerate() {
        let written = call(
            "record_write",
            json!({"projectId": "WORK", "collection": "Builds", "records": chunk, "clientRequestId": format!("builds-{batch}")}),
        )
        .await;
        assert_eq!(written["outcome"], "ok", "{written}");
    }

    let base = format!("/api/v1/projects/{work}/collections");
    let (status, list) = get(&setup, &setup.browser, &base).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let collection = list["collections"]
        .as_array()
        .expect("collections")
        .iter()
        .find(|entry| entry["name"] == "Builds")
        .expect("Builds")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let uri = format!("{base}/{collection}");
    let (status, body) = get(&setup, &setup.browser, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let field = |name: &str| {
        body["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|field| field["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))
            .clone()
    };
    let stage = field("Stage");
    let option = |name: &str| {
        stage["options"]
            .as_array()
            .expect("options")
            .iter()
            .find(|option| option["name"] == name)
            .expect("option")["id"]
            .as_str()
            .expect("id")
            .to_owned()
    };
    let saved = |name: &str| {
        body["views"]
            .as_array()
            .expect("views")
            .iter()
            .find(|view| view["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))
            .clone()
    };
    let pipeline = saved("Pipeline");
    assert_eq!(pipeline["type"], "board");
    let pipeline = pipeline["id"].as_str().expect("view").to_owned();
    let ships = field("Ships")["id"].as_str().expect("id").to_owned();

    // The view's own groups count every record, empty stages left out.
    let (status, counts) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?view={pipeline}&limit=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{counts}");
    assert_eq!(counts["total"], 120);
    let groups = counts["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .map(|group| {
            (
                group["key"].as_str().expect("key").to_owned(),
                group["count"].as_i64().expect("count"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        groups,
        vec![
            (option("Queued"), 60),
            (option("Testing"), 40),
            ("none".to_owned(), 20)
        ]
    );

    // Each column pages through exactly its own records, every one once.
    for (key, expected) in [
        (option("Queued"), 0..60),
        (option("Testing"), 60..100),
        ("none".to_owned(), 100..120),
    ] {
        let mut seen = Vec::new();
        let mut cursor = None::<String>;
        loop {
            let mut path = format!(
                "{uri}/records?view={pipeline}&group={key}&limit=15&sort=%5B%7B%22field%22%3A%22title%22%2C%22direction%22%3A%22asc%22%7D%5D"
            );
            if let Some(cursor) = &cursor {
                path.push_str(&format!("&cursor={cursor}"));
            }
            let (status, page) = get(&setup, &setup.browser, &path).await;
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!(
                page["total"].as_i64(),
                Some(expected.end - expected.start),
                "{page}"
            );
            for record in page["records"].as_array().expect("records") {
                assert_eq!(record["group"]["key"], json!(key), "{record}");
            }
            seen.extend(titles(&page));
            match page["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        let wanted = expected
            .map(|index| format!("Build {index:03}"))
            .collect::<Vec<_>>();
        assert_eq!(seen, wanted);
    }
    // A cursor from one column doesn't read another.
    let (_, first) = get(
        &setup,
        &setup.browser,
        &format!(
            "{uri}/records?view={pipeline}&group={}&limit=5",
            option("Queued")
        ),
    )
    .await;
    let cursor = first["nextCursor"].as_str().expect("cursor");
    let (status, refused) = get(
        &setup,
        &setup.browser,
        &format!(
            "{uri}/records?view={pipeline}&group={}&limit=5&cursor={cursor}",
            option("Testing")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    // A checkbox board: checked and not.
    let sign_off = saved("Sign-off")["id"].as_str().expect("view").to_owned();
    let (_, checked) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?view={sign_off}&group=true&limit=200"),
    )
    .await;
    assert_eq!(checked["total"], 30, "{checked}");
    // A group needs a grouping.
    let all = saved("Cards")["id"].as_str().expect("view").to_owned();
    let (status, refused) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?view={all}&group=none"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");

    // A calendar month: from the first, up to but not including the next.
    let schedule = saved("Schedule")["id"].as_str().expect("view").to_owned();
    let (status, month) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?view={schedule}&groupBy=&dateField={ships}&from=2026-12-01&to=2027-01-01&limit=200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{month}");
    assert_eq!(month["total"], 40, "{month}");
    for record in month["records"].as_array().expect("records") {
        let day = record["values"][&ships].as_str().expect("date");
        assert!(day.starts_with("2026-12-"), "{record}");
    }
    let (_, first_week) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?view={schedule}&dateField={ships}&from=2026-11-01&to=2026-11-03&limit=200"),
    )
    .await;
    // Days 1 and 2 of November: builds 0, 3 (day 1 and 4)... only index % 3 == 0 with index % 28 in 0..2.
    let wanted = (0..120)
        .filter(|index| index % 3 == 0 && index % 28 < 2)
        .count();
    assert_eq!(
        first_week["total"].as_u64(),
        Some(wanted as u64),
        "{first_week}"
    );
    // The view's own filters still hold within the month.
    let (_, queued) = get(
        &setup,
        &setup.browser,
        &format!(
            "{uri}/records?view={schedule}&dateField={ships}&from=2026-12-01&to=2027-01-01&filters={}",
            urlencode(&json!([{"field": stage["id"], "op": "is", "values": [option("Queued")]}]).to_string())
        ),
    )
    .await;
    assert_eq!(
        queued["total"].as_u64(),
        Some((0..60).filter(|index| index % 3 == 1).count() as u64),
        "{queued}"
    );
    // A range that's too long, backwards, on a field that isn't a date, or
    // half given is refused.
    let notes = field("Notes")["id"].as_str().expect("id").to_owned();
    for query in [
        format!("dateField={ships}&from=2026-01-01&to=2026-06-01"),
        format!("dateField={ships}&from=2026-12-01&to=2026-11-01"),
        format!("dateField={notes}&from=2026-12-01&to=2027-01-01"),
        format!("dateField={ships}&from=2026-12-01"),
        format!("dateField={}&from=2026-12-01&to=2027-01-01", Uuid::now_v7()),
    ] {
        let (status, refused) =
            get(&setup, &setup.browser, &format!("{uri}/records?{query}")).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query}: {refused}"
        );
    }

    // The app makes a board from the page, on the field that fits.
    let ops = format!("/api/v2/w/{}/ops/view_create", setup.organization_id);
    let make = |body: Value| write(&setup, &setup.browser, "POST", &ops, body);
    let (status, made) =
        make(json!({"collection": collection, "name": "Stages", "type": "board", "shared": true}))
            .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    assert_eq!(made["value"]["type"], "board");
    assert_eq!(made["value"]["config"]["groupBy"], stage["id"]);
    let (status, made) = make(json!({"collection": collection, "name": "Months", "type": "calendar", "config": {"dateField": ships}})).await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    assert_eq!(made["value"]["config"]["dateField"], json!(ships));
    let (status, refused) = make(json!({"collection": collection, "name": "Wrong", "type": "calendar", "config": {"dateField": notes}})).await;
    assert_ne!(refused["outcome"], "ok", "{status} {refused}");
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// T1.14: a timeline reads the records whose bar, from the start date to the
/// end date (one day without an end), touches the weeks asked for, and no
/// others; a list and a timeline are views any collection can have.
#[tokio::test]
async fn timeline_ranges_read_only_bars_that_touch_them() {
    let Some(setup) = setup("awd110_timeline").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
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
            "name": "Trips",
            "fields": [
                {"name": "Leaves", "type": "date"},
                {"name": "Returns", "type": "date"},
            ],
            "views": [
                {"name": "Plan", "type": "timeline"},
                {"name": "Lines", "type": "list"},
            ],
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let described = call(
        "collection_describe",
        json!({"projectId": "WORK", "collection": "Trips"}),
    )
    .await;
    let views = described["value"]["definition"]["views"].clone();
    let plan = views
        .as_array()
        .expect("views")
        .iter()
        .find(|view| view["name"] == "Plan")
        .expect("Plan")
        .clone();
    assert_eq!(plan["type"], "timeline", "{views}");
    assert_eq!(plan["dateField"], "Leaves", "{views}");
    assert_eq!(plan["endField"], "Returns", "{views}");

    // Before: ends before the range. Across: starts before and ends after.
    // Inside, Late (starts on its last day), Day (no end, inside), After
    // (starts on the day after), Undated (no start).
    let trips = [
        ("Before", Some("2026-10-01"), Some("2026-10-31")),
        ("Across", Some("2026-10-20"), Some("2026-12-20")),
        ("Inside", Some("2026-11-05"), Some("2026-11-08")),
        ("Late", Some("2026-11-30"), Some("2027-01-10")),
        ("Day", Some("2026-11-15"), None),
        ("After", Some("2026-12-01"), Some("2026-12-03")),
        ("Undated", None, Some("2026-11-10")),
    ];
    let records = trips
        .iter()
        .map(|(title, leaves, returns)| {
            let mut fields = json!({});
            if let Some(leaves) = leaves {
                fields["Leaves"] = json!(leaves);
            }
            if let Some(returns) = returns {
                fields["Returns"] = json!(returns);
            }
            json!({"title": title, "fields": fields})
        })
        .collect::<Vec<_>>();
    let written = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Trips", "records": records, "clientRequestId": "trips"}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");

    let base = format!("/api/v1/projects/{work}/collections");
    let (_, list) = get(&setup, &setup.browser, &base).await;
    let collection = list["collections"]
        .as_array()
        .expect("collections")
        .iter()
        .find(|entry| entry["name"] == "Trips")
        .expect("Trips")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let uri = format!("{base}/{collection}");
    let (_, body) = get(&setup, &setup.browser, &uri).await;
    let field = |name: &str| {
        body["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|field| field["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))["id"]
            .as_str()
            .expect("id")
            .to_owned()
    };
    let (leaves, returns) = (field("Leaves"), field("Returns"));
    let (status, november) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?dateField={leaves}&endField={returns}&from=2026-11-01&to=2026-12-01&sort=%5B%7B%22field%22%3A%22title%22%2C%22direction%22%3A%22asc%22%7D%5D"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{november}");
    assert_eq!(
        titles(&november),
        ["Across", "Day", "Inside", "Late"],
        "{november}"
    );
    // A timeline's range may run longer than a calendar's, but not past a
    // year and a bit, and its two fields differ.
    let (status, year) = get(
        &setup,
        &setup.browser,
        &format!(
            "{uri}/records?dateField={leaves}&endField={returns}&from=2026-01-01&to=2027-01-01"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{year}");
    for query in [
        format!("dateField={leaves}&endField={returns}&from=2026-01-01&to=2028-01-01"),
        format!("dateField={leaves}&endField={leaves}&from=2026-11-01&to=2026-12-01"),
        format!("endField={returns}&from=2026-11-01&to=2026-12-01"),
    ] {
        let (status, refused) =
            get(&setup, &setup.browser, &format!("{uri}/records?{query}")).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query}: {refused}"
        );
    }
}

/// T1.31: a chart's groups add up a number field (sum, average, least,
/// most) beside their counts, and the whole adds up each record once even
/// when a record is in several groups; a count reads no field, and a sum
/// of a field that isn't a number is refused.
#[tokio::test]
async fn charts_add_up_a_number_field_by_group() {
    let Some(setup) = setup("awd110_charts").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
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
            "name": "Deals",
            "fields": [
                {"name": "Stage", "type": "single_choice", "options": [{"name": "Open"}, {"name": "Won"}]},
                {"name": "Tags", "type": "multiple_choice", "options": [{"name": "Big"}, {"name": "New"}]},
                {"name": "Amount", "type": "number"},
                {"name": "Note", "type": "text"},
            ],
            "views": [
                {"name": "By stage", "type": "chart", "chart": {"measure": "sum", "field": "Amount"}},
            ],
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let records = [
        ("A", "Open", vec!["Big", "New"], Some(100)),
        ("B", "Open", vec!["Big"], Some(50)),
        ("C", "Won", vec!["New"], Some(30)),
        ("D", "Won", vec![], None),
    ]
    .iter()
    .map(|(title, stage, tags, amount)| {
        let mut fields = json!({"Stage": stage, "Tags": tags});
        if let Some(amount) = amount {
            fields["Amount"] = json!(amount);
        }
        json!({"title": title, "fields": fields})
    })
    .collect::<Vec<_>>();
    let written = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Deals", "records": records, "clientRequestId": "deals"}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");

    // The view groups by its first choice field and sums Amount.
    let described = call(
        "collection_describe",
        json!({"projectId": "WORK", "collection": "Deals"}),
    )
    .await;
    let view = described["value"]["definition"]["views"][0].clone();
    assert_eq!(view["groupBy"], "Stage", "{view}");
    assert_eq!(view["chart"]["measure"], "sum", "{view}");
    assert_eq!(view["chart"]["field"], "Amount", "{view}");
    let by_stage = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Deals", "view": "By stage", "limit": 1}),
    )
    .await;
    let groups = by_stage["value"]["groups"].clone();
    assert_eq!(
        groups,
        json!([
            {"name": "Open", "count": 2, "value": 150.0},
            {"name": "Won", "count": 2, "value": 30.0},
        ]),
        "{by_stage}"
    );
    assert_eq!(by_stage["value"]["value"], 180.0, "{by_stage}");

    // By a many-valued field each record adds to each of its groups, but
    // to the whole once.
    let base = format!("/api/v1/projects/{work}/collections");
    let (_, list) = get(&setup, &setup.browser, &base).await;
    let collection = list["collections"]
        .as_array()
        .expect("collections")
        .iter()
        .find(|entry| entry["name"] == "Deals")
        .expect("Deals")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let uri = format!("{base}/{collection}");
    let (_, body) = get(&setup, &setup.browser, &uri).await;
    let field = |name: &str| {
        body["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|field| field["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))["id"]
            .as_str()
            .expect("id")
            .to_owned()
    };
    let (tags, amount, note) = (field("Tags"), field("Amount"), field("Note"));
    let (status, by_tag) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?groupBy={tags}&measure=average&measureField={amount}&limit=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_tag}");
    let values = by_tag["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .map(|group| {
            (
                group["label"].as_str().unwrap_or("").to_owned(),
                group["count"].clone(),
                group["value"].clone(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        values.contains(&("Big".into(), json!(2), json!(75.0))),
        "{by_tag}"
    );
    assert!(
        values.contains(&("New".into(), json!(2), json!(65.0))),
        "{by_tag}"
    );
    assert_eq!(by_tag["value"], json!(60.0), "{by_tag}");
    let (status, most) = get(
        &setup,
        &setup.browser,
        &format!("{uri}/records?groupBy=&measure=max&measureField={amount}&limit=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{most}");
    assert_eq!(most["value"], json!(100.0), "{most}");
    assert_eq!(most["total"], json!(4), "{most}");

    for query in [
        format!("measure=sum&measureField={note}"),
        format!("measure=count&measureField={amount}"),
        "measure=sum".to_owned(),
        "measure=median".to_owned(),
        format!("measureField={amount}"),
    ] {
        let (status, refused) =
            get(&setup, &setup.browser, &format!("{uri}/records?{query}")).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query}: {refused}"
        );
    }
}

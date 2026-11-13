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
                "{uri}/records?view={pipeline}&group={key}&limit=15&sort=%5B%7B%22field%22%3A%22title%22%7D%5D"
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
    let (status, made) = write(
        &setup,
        &setup.browser,
        "POST",
        &format!("{uri}/views"),
        json!({"name": "Stages", "type": "board"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["type"], "board");
    assert_eq!(made["groupBy"], stage["id"]);
    let (status, made) = write(
        &setup,
        &setup.browser,
        "POST",
        &format!("{uri}/views"),
        json!({"name": "Months", "type": "calendar", "config": {"dateField": ships}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["dateField"], json!(ships));
    let (status, refused) = write(
        &setup,
        &setup.browser,
        "POST",
        &format!("{uri}/views"),
        json!({"name": "Wrong", "type": "calendar", "config": {"dateField": notes}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, refused) = write(
        &setup,
        &setup.browser,
        "POST",
        &format!("{uri}/views"),
        json!({"name": "Odd", "type": "timeline"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
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

//! AWD-110: forms that add a record, grown from bug report links. The
//! silent, costly failures: a public form that shows more than the form
//! (records, people, other fields); an answer that writes a field the form
//! doesn't ask for; a form for a collection in another project; a viewer
//! making one; a paused form still taking answers; and a form that keeps
//! acting for someone who can no longer add records.

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::add_person,
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

/// A request from anyone with the link: no session, any origin.
async fn public(
    setup: &Setup,
    method: &str,
    uri: &str,
    content_type: &str,
    body: &str,
) -> (StatusCode, Value) {
    send(
        &setup.app,
        Request::builder()
            .method(method)
            .uri(uri)
            .header("origin", "https://playtest.example.com")
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body.to_owned()))
            .expect("request"),
    )
    .await
}

/// The token in a form's URL.
fn token_of(url: &Value) -> String {
    url.as_str()
        .expect("url")
        .rsplit('/')
        .next()
        .expect("token")
        .to_owned()
}

#[tokio::test]
async fn a_form_shows_only_itself_and_each_answer_is_one_record_of_its_fields() {
    let Some(setup) = setup("awd110_forms").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let base = format!("/api/v1/projects/{work}/collections");
    let (status, created) = write(
        &setup,
        &setup.browser,
        "POST",
        &base,
        json!({
            "name": "Playtests",
            "titleName": "Finding",
            "fields": [
                {"name": "Severity", "type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]},
                {"name": "Score", "type": "number"},
                {"name": "Platforms", "type": "multiple_choice", "options": [{"name": "PC"}, {"name": "Deck"}]},
                {"name": "Tester", "type": "person"},
                {"name": "Internal notes", "type": "text"},
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let collection = created["collection"]["id"].as_str().expect("id").to_owned();
    let field = |name: &str| {
        created["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|field| field["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))["id"]
            .as_str()
            .expect("id")
            .to_owned()
    };
    let links = format!("/api/v1/projects/{work}/bug-report-links");
    let form = json!({
        "intro": "Tell us what broke.",
        "thanks": "Thanks, we read every one.",
        "fields": [
            {"field": field("Severity"), "required": true},
            {"field": field("Score")},
            {"field": field("Platforms")},
        ],
    });

    // A person can't be asked on a public form.
    let (status, refused) = write(
        &setup,
        &setup.browser,
        "POST",
        &links,
        json!({"name": "Playtest form", "collectionId": collection, "form": {"fields": [{"field": field("Tester")}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Tester"),
        "{refused}"
    );
    // A collection from another project is never found.
    let (status, made_elsewhere) = write(
        &setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{other}/bug-report-links"),
        json!({"name": "Sneaky", "collectionId": collection, "form": {"fields": []}}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{made_elsewhere}");

    let (status, made) = write(
        &setup,
        &setup.browser,
        "POST",
        &links,
        json!({"name": "Playtest form", "collectionId": collection, "form": form}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["link"]["collectionName"], "Playtests");
    let token = token_of(&made["formUrl"]);
    assert!(
        made["formUrl"]
            .as_str()
            .expect("form url")
            .ends_with(&format!("/forms/{token}"))
    );
    let link_id = made["link"]["id"].as_str().expect("link").to_owned();

    // The page reads the form and nothing else: no records, no people, no
    // field the form doesn't ask for.
    let (status, page) = public(
        &setup,
        "GET",
        &format!("/api/v1/forms/{token}"),
        "text/plain",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["collection"], "Playtests");
    assert_eq!(page["titleName"], "Finding");
    assert_eq!(page["intro"], "Tell us what broke.");
    let names = page["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .map(|field| field["name"].as_str().expect("name").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["Severity", "Score", "Platforms"]);
    assert_eq!(page["fields"][0]["required"], true);
    let text = page.to_string();
    for leak in ["Tester", "Internal notes", "records"] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    let (status, _) = public(
        &setup,
        "GET",
        "/api/v1/forms/awbr_nothing",
        "text/plain",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // An answer is one record of the asked fields, whatever else is sent.
    let answer = "finding=Falls+through+the+floor&Severity=High&Score=4&Platforms=PC&Platforms=Deck&Internal+notes=sneaky";
    let report = format!("/api/v1/bug-reports/{token}");
    let (status, sent) = public(
        &setup,
        "POST",
        &report,
        "application/x-www-form-urlencoded",
        answer,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(sent["record"], 1);
    assert_eq!(sent["thanks"], "Thanks, we read every one.");
    let (status, again) = public(
        &setup,
        "POST",
        &report,
        "application/x-www-form-urlencoded",
        answer,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["duplicate"], true);
    let (_, records) = get(
        &setup,
        &setup.browser,
        &format!("{base}/{collection}/records"),
    )
    .await;
    assert_eq!(records["total"], 1, "{records}");
    let record = &records["records"][0];
    assert_eq!(record["title"], "Falls through the floor");
    assert_eq!(record["values"][field("Score")], 4.0);
    assert_eq!(
        record["values"][field("Platforms")]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(
        record["values"].get(field("Internal notes")).is_none(),
        "{record}"
    );

    // A required field left out, or a value the collection refuses, says why.
    let (status, refused) = public(
        &setup,
        "POST",
        &report,
        "application/json",
        r#"{"title": "No severity"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"], "Fill in Severity.");
    let (status, refused) = public(
        &setup,
        "POST",
        &report,
        "application/json",
        r#"{"title": "Odd", "Severity": "Catastrophic"}"#,
    )
    .await;
    assert!(status.is_client_error(), "{status} {refused}");
    let (_, records) = get(
        &setup,
        &setup.browser,
        &format!("{base}/{collection}/records"),
    )
    .await;
    assert_eq!(records["total"], 1, "{records}");

    // Paused, it shows and takes nothing.
    let (status, paused) = write(
        &setup,
        &setup.browser,
        "PATCH",
        &format!("/api/v1/bug-report-links/{link_id}"),
        json!({"state": "paused"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    let (status, _) = public(
        &setup,
        "GET",
        &format!("/api/v1/forms/{token}"),
        "text/plain",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = public(
        &setup,
        "POST",
        &report,
        "application/json",
        r#"{"title": "Later", "Severity": "Low"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A task link's report URL isn't a form.
    let (_, task_link) = write(
        &setup,
        &setup.browser,
        "POST",
        &links,
        json!({"name": "Crashes"}),
    )
    .await;
    let (status, _) = public(
        &setup,
        "GET",
        &format!("/api/v1/forms/{}", token_of(&task_link["url"])),
        "text/plain",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A viewer can't make a form.
    let project = work.parse::<Uuid>().expect("project");
    let (vera, vera_browser) = add_person(&setup, &[&work], "vera").await;
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(vera)
    .execute(&setup.pool)
    .await
    .expect("viewer");
    let (status, refused) = write(
        &setup,
        &vera_browser,
        "POST",
        &links,
        json!({"name": "Mine", "collectionId": collection, "form": {"fields": []}}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    // A member makes one, then becomes a viewer: the form turns itself off
    // instead of adding records for them.
    let (mel, mel_browser) = add_person(&setup, &[&work], "mel").await;
    let (status, mels) = write(
        &setup,
        &mel_browser,
        "POST",
        &links,
        json!({"name": "Mel's form", "collectionId": collection, "form": {"fields": [{"field": field("Severity")}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mels}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(mel)
    .execute(&setup.pool)
    .await
    .expect("viewer");
    let (status, _) = public(
        &setup,
        "POST",
        &format!("/api/v1/bug-reports/{}", token_of(&mels["formUrl"])),
        "application/json",
        r#"{"title": "From Mel's form"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let state = sqlx::query_scalar::<_, String>("SELECT state FROM bug_report_links WHERE id = $1")
        .bind(
            mels["link"]["id"]
                .as_str()
                .expect("id")
                .parse::<Uuid>()
                .expect("uuid"),
        )
        .fetch_one(&setup.pool)
        .await
        .expect("state");
    assert_eq!(state, "disabled");
    // Still its maker, but without record edit: the questions can't change.
    let (status, refused) = write(
        &setup,
        &mel_browser,
        "PATCH",
        &format!("/api/v1/bug-report-links/{}", mels["link"]["id"].as_str().expect("id")),
        json!({"form": {"fields": []}}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    // An app connected as a person makes one by field names; a person field
    // is refused there too.
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let made = tool(
        app,
        &codex,
        &session,
        "bug_report_link_write",
        json!({
            "projectId": "WORK",
            "name": "Agent form",
            "collection": "Playtests",
            "form": {"fields": [{"field": "Severity", "required": true}, {"field": "Score"}]},
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert!(made["value"]["formUrl"].as_str().is_some(), "{made}");
    let refused = tool(
        app,
        &codex,
        &session,
        "bug_report_link_write",
        json!({
            "projectId": "WORK",
            "name": "People form",
            "collection": "Playtests",
            "form": {"fields": [{"field": "Tester"}]},
        }),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let changed = tool(
        app,
        &codex,
        &session,
        "bug_report_link_write",
        json!({
            "projectId": "WORK",
            "linkId": made["value"]["link"]["id"],
            "form": {"intro": "Shorter", "fields": [{"field": "Score", "required": true}]},
        }),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    assert_eq!(changed["value"]["form"]["intro"], "Shorter");
}

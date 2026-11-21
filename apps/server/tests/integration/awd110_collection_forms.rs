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
    modules::switch,
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

/// Makes a form link, or changes one, through its operation; answers the
/// status and the operation's value, or the refusal.
async fn link_op(
    setup: &Setup,
    browser: &Browser,
    link: Option<&str>,
    project: &str,
    mut body: Value,
) -> (StatusCode, Value) {
    if let Some(collection) = body
        .as_object_mut()
        .and_then(|body| body.remove("collectionId"))
    {
        body["collection"] = collection;
    }
    body["projectId"] = json!(project);
    let name = match link {
        None => "bug_report_link_create",
        Some(link) => {
            body["linkId"] = json!(link);
            "bug_report_link_update"
        }
    };
    let (status, mut answer) =
        write(setup, browser, "POST", &format!("/api/v2/ops/{name}"), body).await;
    if status.is_success() {
        (status, answer["value"].take())
    } else {
        (status, answer)
    }
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

/// An answer with the page's render token in it (T1.29), as JSON or a form.
fn signed(body: &str, rendered: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(Value::Object(mut answer)) => {
            answer.insert("_aw_rendered".to_owned(), json!(rendered));
            Value::Object(answer).to_string()
        }
        _ => format!("{body}&_aw_rendered={rendered}"),
    }
}

/// Reads a form's page and waits until an answer from it may be sent.
async fn render(setup: &Setup, token: &str) -> String {
    let (status, page) = public(
        setup,
        "GET",
        &format!("/api/v1/forms/{token}"),
        "text/plain",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    tokio::time::sleep(std::time::Duration::from_millis(3_100)).await;
    page["renderToken"]
        .as_str()
        .expect("render token")
        .to_owned()
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
    let (status, refused) = link_op(&setup, &setup.browser, None, &work,
        json!({"name": "Playtest form", "collectionId": collection, "form": {"fields": [{"field": field("Tester")}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Tester"),
        "{refused}"
    );
    // A collection from another project is never found.
    let (status, made_elsewhere) = link_op(
        &setup,
        &setup.browser,
        None,
        &other,
        json!({"name": "Sneaky", "collectionId": collection, "form": {"fields": []}}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{made_elsewhere}");

    let (status, made) = link_op(
        &setup,
        &setup.browser,
        None,
        &work,
        json!({"name": "Playtest form", "collectionId": collection, "form": form}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
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
    let rendered = render(&setup, &token).await;
    let answer = &signed(answer, &rendered);
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
        &signed(r#"{"title": "No severity"}"#, &rendered),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"], "Fill in Severity.");
    let (status, refused) = public(
        &setup,
        "POST",
        &report,
        "application/json",
        &signed(r#"{"title": "Odd", "Severity": "Catastrophic"}"#, &rendered),
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
    let (status, paused) = link_op(
        &setup,
        &setup.browser,
        Some(&link_id),
        &work,
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
        &signed(r#"{"title": "Later", "Severity": "Low"}"#, &rendered),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A task link's report URL isn't a form.
    let (_, task_link) = link_op(
        &setup,
        &setup.browser,
        None,
        &work,
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
    let (status, refused) = link_op(
        &setup,
        &vera_browser,
        None,
        &work,
        json!({"name": "Mine", "collectionId": collection, "form": {"fields": []}}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    // A member makes one, then becomes a viewer: the form turns itself off
    // instead of adding records for them.
    let (mel, mel_browser) = add_person(&setup, &[&work], "mel").await;
    let (status, mels) = link_op(&setup, &mel_browser, None, &work,
        json!({"name": "Mel's form", "collectionId": collection, "form": {"fields": [{"field": field("Severity")}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mels}");
    let mels_rendered = render(&setup, &token_of(&mels["formUrl"])).await;
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
        &signed(r#"{"title": "From Mel's form"}"#, &mels_rendered),
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
    let (status, refused) = link_op(
        &setup,
        &mel_browser,
        Some(mels["link"]["id"].as_str().expect("id")),
        &work,
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

    // With Tasks off and Collections on, the link tools still serve forms
    // and leave task links out of reach.
    let (status, switched) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"tasks": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{switched}");
    let listed = tool(
        app,
        &codex,
        &session,
        "bug_report_link_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let links = listed["value"]["links"].as_array().expect("links");
    assert!(!links.is_empty(), "{listed}");
    assert!(
        links.iter().all(|link| link["collectionId"].is_string()),
        "{listed}"
    );
    let task_made = tool(
        app,
        &codex,
        &session,
        "bug_report_link_write",
        json!({"projectId": "WORK", "name": "More crashes"}),
    )
    .await;
    assert_eq!(task_made["outcome"], "conflict", "{task_made}");
    let task_paused = tool(
        app,
        &codex,
        &session,
        "bug_report_link_write",
        json!({"projectId": "WORK", "linkId": task_link["link"]["id"], "state": "paused"}),
    )
    .await;
    assert_eq!(task_paused["outcome"], "conflict", "{task_paused}");
    let form_renamed = tool(
        app,
        &codex,
        &session,
        "bug_report_link_write",
        json!({"projectId": "WORK", "linkId": made["value"]["link"]["id"], "name": "Agent form, renamed"}),
    )
    .await;
    assert_eq!(form_renamed["outcome"], "ok", "{form_renamed}");
}

/// T1.29: a public form holds back what isn't a person: an answer sent
/// sooner after the page loaded than anyone could type, one without the
/// page at all, one that filled in the field people never see, more than
/// twenty an hour from one address, and more than its daily limit, which
/// pauses it and tells its maker.
#[tokio::test]
async fn public_forms_hold_back_scripts_floods_and_a_busy_day() {
    let Some(setup) = setup("t1_form_limits").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let (status, created) = write(
        &setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{work}/collections"),
        json!({"name": "Signups", "titleName": "Name", "fields": []}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let collection = created["collection"]["id"].as_str().expect("id").to_owned();
    let make = |name: &'static str, limit: Option<u32>| {
        let mut form = json!({"fields": []});
        if let Some(limit) = limit {
            form["dailyLimit"] = json!(limit);
        }
        json!({"name": name, "collectionId": collection, "form": form})
    };
    let (status, refused) = link_op(
        &setup,
        &setup.browser,
        None,
        &work,
        make("Too many", Some(10_001)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, small) =
        link_op(&setup, &setup.browser, None, &work, make("Small", Some(3))).await;
    assert_eq!(status, StatusCode::OK, "{small}");
    assert_eq!(small["link"]["form"]["dailyLimit"], 3, "{small}");
    let small_token = token_of(&small["formUrl"]);
    let small_report = format!("/api/v1/bug-reports/{small_token}");
    let small_id = small["link"]["id"].as_str().expect("id").to_owned();
    let records = || async {
        get(
            &setup,
            &setup.browser,
            &format!("/api/v1/projects/{work}/collections/{collection}/records"),
        )
        .await
        .1["total"]
            .as_i64()
            .unwrap_or_default()
    };

    // No page, a forged page, and a page loaded a moment ago are refused.
    let (status, page) = public(
        &setup,
        "GET",
        &format!("/api/v1/forms/{small_token}"),
        "text/plain",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["honeypot"], "_aw_homepage");
    let fresh = page["renderToken"]
        .as_str()
        .expect("render token")
        .to_owned();
    let post = |body: String| {
        let report = small_report.clone();
        let setup = &setup;
        async move { public(setup, "POST", &report, "application/json", &body).await }
    };
    let (status, refused) = post(json!({"title": "Quick"}).to_string()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, refused) =
        post(json!({"title": "Quick", "_aw_rendered": "1.00"}).to_string()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, refused) =
        post(json!({"title": "Quick", "_aw_rendered": fresh}).to_string()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(records().await, 0);

    // A filled-in hidden field is thanked, and nothing is kept.
    let rendered = render(&setup, &small_token).await;
    let answer = |title: &str| json!({"title": title, "_aw_rendered": rendered}).to_string();
    let (status, thanked) = post(
        json!({"title": "Bot", "_aw_homepage": "https://spam.example", "_aw_rendered": rendered})
            .to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{thanked}");
    assert_eq!(records().await, 0);

    // Three a day: the third pauses the form and tells its maker once; a
    // repeat doesn't count.
    for title in ["Ada", "Ada", "Brook", "Cyd"] {
        let (status, sent) = post(answer(title)).await;
        assert!(status.is_success(), "{title}: {status} {sent}");
    }
    assert_eq!(records().await, 3);
    let (status, refused) = post(answer("Dee")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let link = small_id.parse::<Uuid>().expect("link");
    let (state, reason) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, disabled_reason FROM bug_report_links WHERE id = $1",
    )
    .bind(link)
    .fetch_one(&setup.pool)
    .await
    .expect("link");
    assert_eq!(state, "paused");
    assert!(reason.unwrap_or_default().contains("daily limit"));
    let told = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE kind = 'form_paused' AND subject_id = $1",
    )
    .bind(link)
    .fetch_one(&setup.pool)
    .await
    .expect("notifications");
    assert_eq!(told, 1);

    // Turned on again, it takes answers again.
    let (status, resumed) = link_op(
        &setup,
        &setup.browser,
        Some(&small_id),
        &work,
        json!({"state": "active"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let (status, sent) = post(answer("Dee")).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");

    // Twenty an hour from one address; another address still gets in.
    let (status, open) = link_op(&setup, &setup.browser, None, &work, make("Open", None)).await;
    assert_eq!(status, StatusCode::OK, "{open}");
    let open_token = token_of(&open["formUrl"]);
    let open_rendered = render(&setup, &open_token).await;
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database url");
    let addressed = super::support::app::test_app_with(
        setup.pool.clone(),
        &database_url,
        &[(
            "AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER",
            "x-client-address".to_owned(),
        )],
    );
    let from = |address: &'static str, title: String| {
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/bug-reports/{open_token}"))
            .header("x-client-address", address)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"title": title, "_aw_rendered": open_rendered}).to_string(),
            ))
            .expect("request");
        send(&addressed, request)
    };
    for n in 0..20 {
        let (status, sent) = from("203.0.113.9", format!("Signup {n}")).await;
        assert_eq!(status, StatusCode::CREATED, "{n}: {sent}");
    }
    let (status, refused) = from("203.0.113.9", "One more".to_owned()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    let (status, sent) = from("198.51.100.4", "Someone else".to_owned()).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
}

/// T1.11: the Tasks collection is edited like any other. A field added on
/// its collection screen is a task field, an agent writes its records (by
/// field and option names) as task changes, and a form on it makes tasks.
#[tokio::test]
async fn tasks_take_fields_records_and_forms_like_any_collection() {
    let Some(setup) = setup("t1_11_tasks_like_any").await else {
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
    let base = format!("/api/v1/projects/{work}/collections");
    let (status, listed) = get(&setup, &setup.browser, &base).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let tasks_id = listed["collections"]
        .as_array()
        .expect("collections")
        .iter()
        .find(|collection| collection["builtin"] == "tasks")
        .expect("Tasks")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let (status, effort) = write(
        &setup,
        &setup.browser,
        "POST",
        &format!("{base}/{tasks_id}/fields"),
        json!({"name": "Effort", "type": "number"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{effort}");

    // An agent writes a record of Tasks by names: it's a task, with the
    // built-in fields and the new one set.
    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Tasks",
            "records": [{"title": "Oil the hinges", "fields": {"Priority": "High", "Effort": 3}}],
            "clientRequestId": "hinges",
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    assert_eq!(written["value"]["created"], 1, "{written}");
    let task = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM task_rows WHERE project_id = $1::uuid AND title = 'Oil the hinges' \
         AND priority = 'high'",
    )
    .bind(&work)
    .fetch_one(&setup.pool)
    .await
    .expect("the task, high priority");
    let opened = call("task_open", json!({"taskId": task.to_string()})).await;
    let fields = opened["value"]["task"]["fields"]
        .as_array()
        .or_else(|| opened["value"]["fields"].as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        fields
            .iter()
            .any(|field| field["name"] == "Effort" && field["value"] == 3),
        "{opened}"
    );
    // A second write of the same values changes nothing.
    let again = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Tasks",
            "records": [{"record": task.to_string(), "fields": {"Effort": 3}}],
            "clientRequestId": "hinges-again",
        }),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    // A wrong option is refused with its record's place.
    let wrong = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Tasks",
            "records": [{"title": "Fine"}, {"title": "Odd", "fields": {"Priority": "Whenever"}}],
            "clientRequestId": "wrong-priority",
        }),
    )
    .await;
    assert_eq!(wrong["outcome"], "invalid_request", "{wrong}");
    let fine = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_rows WHERE project_id = $1::uuid AND title = 'Fine'",
    )
    .bind(&work)
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(fine, 0, "all or nothing");

    // A form on Tasks makes a task of its answers.
    let made = call(
        "bug_report_link_write",
        json!({"projectId": "WORK", "name": "Chores form", "collection": "Tasks",
               "form": {"fields": [{"field": "Priority"}, {"field": "Effort", "required": true}]}}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let token = token_of(&made["value"]["formUrl"]);
    let rendered = render(&setup, &token).await;
    let (status, sent) = public(
        &setup,
        "POST",
        &format!("/api/v1/bug-reports/{token}"),
        "application/json",
        &signed(
            &json!({"title": "Sweep the deck", "Priority": "Low", "Effort": 2}).to_string(),
            &rendered,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let (priority, values) = sqlx::query_as::<_, (String, Value)>(
        "SELECT priority, field_values FROM task_rows \
         WHERE project_id = $1::uuid AND title = 'Sweep the deck'",
    )
    .bind(&work)
    .fetch_one(&setup.pool)
    .await
    .expect("the answer's task");
    assert_eq!(priority, "low");
    let effort_id = effort["field"]["id"]
        .as_str()
        .or_else(|| effort["id"].as_str())
        .expect("Effort's id");
    assert_eq!(values[effort_id], 2, "{values}");
}

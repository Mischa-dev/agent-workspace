//! A1.8: requests are raised, answered and withdrawn by role. The silent
//! failures: someone the request isn't for answering it (or reading it), an
//! answer changed after the raiser acted on it, the raiser never hearing,
//! and a run left waiting on a request that was answered or ran out.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::add_person,
    notifications::{drain, inbox},
};

/// The request card of `kind` about `request` in an inbox page.
fn card(page: &Value, kind: &str, request: &str) -> Option<Value> {
    page["notifications"]
        .as_array()
        .expect("notifications")
        .iter()
        .find(|notification| {
            notification["kind"] == kind && notification["subject"]["id"] == request
        })
        .cloned()
}

#[tokio::test]
async fn an_agent_asks_a_person_and_hears_their_answer() {
    let Some(setup) = setup("a1_requests_ask").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "ASK", "Asking").await;
    let (_mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let elsewhere = create_project(&setup, "OUT", "Elsewhere").await;
    let (outsider, _) = add_person(&setup, &[&elsewhere], "otto").await;
    let space = space(&setup).await;
    let (scout, scout_token) = create_agent(app, &setup.browser, &space, "Scout", "scout").await;
    add_member(&setup, &project, scout, "member").await;
    let scout_session = initialize(app, &scout_token).await.expect("Scout MCP");
    let (other, other_token) = create_agent(app, &setup.browser, &space, "Other", "other").await;
    add_member(&setup, &project, other, "member").await;
    let other_session = initialize(app, &other_token).await.expect("Other MCP");
    let scout_call = |name: &'static str, arguments: Value| {
        let token = scout_token.clone();
        let session = scout_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let other_call = |name: &'static str, arguments: Value| {
        let token = other_token.clone();
        let session = other_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let ask = json!({
        "spaceId": project,
        "title": "Ship the colour change today?",
        "options": [{"id": "ship", "label": "Ship it"}, {"id": "wait", "label": "Wait for review"}],
        "audience": {"accounts": ["@mara"]},
        "clientRequestId": "ask-mara-1",
    });
    let asked = scout_call("request_create", ask.clone()).await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    assert_eq!(asked["value"]["status"], "open");
    let request = asked["value"]["id"].as_str().expect("request").to_owned();
    // A retry asks once.
    let again = scout_call("request_create", ask).await;
    assert_eq!(again["value"]["id"], request.as_str(), "{again}");

    // Only accounts in the project, and not yourself; at most 8 options.
    for account in [outsider.to_string(), scout.to_string()] {
        let refused = scout_call(
            "request_create",
            json!({"spaceId": project, "title": "Hello?", "audience": {"accounts": [account]}}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    }
    let options = (0..9)
        .map(|n| json!({"id": format!("o{n}"), "label": format!("Option {n}")}))
        .collect::<Vec<_>>();
    let too_many = scout_call(
        "request_create",
        json!({"spaceId": project, "title": "Pick", "options": options,
               "audience": {"accounts": ["@mara"]}}),
    )
    .await;
    assert_eq!(too_many["outcome"], "invalid_request", "{too_many}");
    let nobody = scout_call(
        "request_create",
        json!({"spaceId": project, "title": "Anyone?", "audience": {}}),
    )
    .await;
    assert_eq!(nobody["outcome"], "invalid_request", "{nobody}");

    // The card is in Mara's inbox, with its options.
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    let raised = card(&page, "request_raised", &request)
        .unwrap_or_else(|| panic!("no request card: {page}"));
    assert!(
        raised["title"]
            .as_str()
            .is_some_and(|title| title.contains("has a question for you")),
        "{raised}"
    );
    assert_eq!(raised["request"]["status"], "open");
    assert_eq!(raised["request"]["options"][1]["label"], "Wait for review");
    // Nobody else got one.
    let owner_page = inbox(&setup, &setup.browser, "").await;
    assert!(
        card(&owner_page, "request_raised", &request).is_none(),
        "{owner_page}"
    );

    // Nobody else can answer it, and the agent that asked can't either.
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("owner-answer"),
            json!({"requestId": request, "optionId": "ship"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    for answered in [
        scout_call(
            "request_respond",
            json!({"requestId": request, "optionId": "ship"}),
        )
        .await,
        other_call(
            "request_respond",
            json!({"requestId": request, "optionId": "ship"}),
        )
        .await,
    ] {
        assert_eq!(answered["outcome"], "invalid_request", "{answered}");
    }
    // An option it doesn't offer, or nothing at all, isn't an answer.
    for (key, body) in [
        (
            "bad-option",
            json!({"requestId": request, "optionId": "nine"}),
        ),
        ("empty", json!({"requestId": request, "text": "   "})),
    ] {
        let (status, refused) = send(
            app,
            mara_browser.request("POST", "/api/v2/ops/request_respond", Some(key), body),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    }
    let answer = json!({"requestId": request, "optionId": "ship", "text": "After lunch"});
    let (status, answered) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("0190b6a4-1c2d-7e3f-8a9b-0c1d2e3f4a5b"),
            answer.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["value"]["status"], "answered", "{answered}");
    assert_eq!(answered["value"]["answer"]["label"], "Ship it");
    // An answer is final; the same answer sent again is the same answer.
    let (status, changed) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("0190b6a4-1c2d-7e3f-8a9b-0c1d2e3f4a5c"),
            json!({"requestId": request, "optionId": "wait"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{changed}");
    let (status, replayed) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("0190b6a4-1c2d-7e3f-8a9b-0c1d2e3f4a5b"),
            answer,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["value"]["answer"]["optionId"], "ship");

    // The answer goes back to Scout: an event naming the request, and the
    // request itself.
    agent_workspace_server::events::catch_up(&setup.pool)
        .await
        .expect("sequence events");
    let events = scout_call(
        "external_event_poll",
        json!({"limit": 50, "waitSeconds": 0}),
    )
    .await;
    let event = events["events"]
        .as_array()
        .unwrap_or_else(|| panic!("events: {events}"))
        .iter()
        .find(|event| event["kind"] == "request.answered")
        .unwrap_or_else(|| panic!("no answered event: {events}"))
        .clone();
    assert_eq!(event["resourceId"], request.as_str());
    assert!(!event.to_string().contains("After lunch"), "{event}");
    let read = scout_call("request_get", json!({"requestId": request})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["request"]["answer"]["text"], "After lunch");
    assert_eq!(read["value"]["mayAnswer"], false);
    let mine = scout_call("request_list", json!({"role": "raised"})).await;
    assert!(mine.to_string().contains(&request), "{mine}");

    // Another agent neither hears of it nor is asked it.
    let others = other_call("request_list", json!({})).await;
    assert_eq!(others["outcome"], "ok", "{others}");
    assert!(!others.to_string().contains(&request), "{others}");
    let other_events = other_call(
        "external_event_poll",
        json!({"limit": 50, "waitSeconds": 0}),
    )
    .await;
    assert!(
        !other_events.to_string().contains("request.answered"),
        "{other_events}"
    );

    // The card shows the answer and is read.
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    let raised = card(&page, "request_raised", &request).expect("request card");
    assert_eq!(raised["read"], true, "{raised}");
    assert_eq!(raised["request"]["status"], "answered");
    assert_eq!(raised["request"]["answer"]["text"], "After lunch");
}

#[tokio::test]
async fn a_request_to_a_role_goes_to_its_holders_only() {
    let Some(setup) = setup("a1_requests_role").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "ROLE", "Roles").await;
    let (_mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let space = space(&setup).await;
    let (scout, scout_token) = create_agent(app, &setup.browser, &space, "Scout", "scout").await;
    add_member(&setup, &project, scout, "member").await;
    let scout_session = initialize(app, &scout_token).await.expect("Scout MCP");

    let asked = tool(
        app,
        &scout_token,
        &scout_session,
        "request_create",
        json!({"spaceId": project, "kind": "approval", "title": "Spend the budget?",
               "options": [{"id": "yes", "label": "Yes", "style": "approve"},
                           {"id": "no", "label": "No", "style": "deny"}],
               "audience": {"roles": ["owner"]}}),
    )
    .await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    let request = asked["value"]["id"].as_str().expect("request").to_owned();
    assert_eq!(asked["value"]["allowText"], false);

    drain(&setup).await;
    let owner_page = inbox(&setup, &setup.browser, "").await;
    let raised = card(&owner_page, "request_raised", &request)
        .unwrap_or_else(|| panic!("the owner wasn't asked: {owner_page}"));
    assert!(
        raised["title"]
            .as_str()
            .is_some_and(|title| title.contains("needs your approval")),
        "{raised}"
    );
    let mara_page = inbox(&setup, &mara_browser, "").await;
    assert!(
        card(&mara_page, "request_raised", &request).is_none(),
        "{mara_page}"
    );

    // A member who doesn't hold the role can't answer; a typed answer isn't
    // taken; the owner answers.
    let (status, refused) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("mara-yes"),
            json!({"requestId": request, "optionId": "yes"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("owner-typed"),
            json!({"requestId": request, "text": "Sure"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, answered) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("owner-yes"),
            json!({"requestId": request, "optionId": "yes"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["value"]["status"], "answered");

    // The raiser withdraws only what is still open, and only its own.
    let withdrawn = tool(
        app,
        &scout_token,
        &scout_session,
        "request_create",
        json!({"spaceId": project, "title": "Still needed?", "audience": {"accounts": ["@mara"]}}),
    )
    .await;
    let withdrawn = withdrawn["value"]["id"]
        .as_str()
        .expect("request")
        .to_owned();
    let (status, refused) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v2/ops/request_cancel",
            Some("mara-cancel"),
            json!({"requestId": withdrawn}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let cancelled = tool(
        app,
        &scout_token,
        &scout_session,
        "request_cancel",
        json!({"requestId": withdrawn}),
    )
    .await;
    assert_eq!(cancelled["value"]["status"], "cancelled", "{cancelled}");
}

#[tokio::test]
async fn a_run_waiting_on_a_request_goes_on_when_it_is_answered_or_runs_out() {
    let Some(setup) = setup("a1_requests_run").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "RUN", "Runs").await;
    let (mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let (_fixer, fixer_token) = create_agent(app, &setup.browser, &project, "Fixer", "fixer").await;
    let fixer = initialize(app, &fixer_token).await.expect("Fixer MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = fixer_token.clone();
        let session = fixer.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let opened = call(
        "run_create",
        json!({"spaceId": project, "title": "Tidy the backlog", "cause": {"kind": "manual"}}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    let run = run_id.parse::<Uuid>().expect("run UUID");
    let asked = call(
        "run_activity_report",
        json!({"runId": run_id, "activities": [
            {"id": "q1", "kind": "interrupt", "payload": {"request": {
                "title": "Close the stale ones?", "audience": {"accounts": [mara]}}}},
            {"id": "q2", "kind": "interrupt", "payload": {"request": {
                "kind": "approval", "title": "Archive the board?",
                "options": [{"id": "go", "label": "Go", "style": "approve"},
                            {"id": "stop", "label": "Stop", "style": "deny"}],
                "audience": {"accounts": [mara]},
                "expiresInSeconds": 60, "onExpiry": "deny"}}}]}),
    )
    .await;
    assert_eq!(asked["value"]["state"], "waiting", "{asked}");
    let first = asked["value"]["requests"][0]["requestId"]
        .as_str()
        .expect("first request")
        .to_owned();
    let second = asked["value"]["requests"][1]["requestId"]
        .as_str()
        .expect("second request")
        .parse::<Uuid>()
        .expect("UUID");
    let state = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, String>("SELECT state FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .expect("state")
    };

    // Answering one: the run still waits on the other.
    let (status, answered) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("mara-close"),
            json!({"requestId": first, "text": "Yes, close them"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(state(setup.pool.clone()).await, "waiting");

    // The other runs out and is denied: the run goes on.
    sqlx::query("UPDATE requests SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(second)
        .execute(&setup.pool)
        .await
        .expect("age the request");
    let settled =
        agent_workspace_server::requests::expire_due(&crate::support::app::state_on(&setup.pool))
            .await
            .expect("expire");
    assert_eq!(settled, 1);
    let (status, answer) = sqlx::query_as::<_, (String, Option<Value>)>(
        "SELECT status, answer FROM requests WHERE id = $1",
    )
    .bind(second)
    .fetch_one(&setup.pool)
    .await
    .expect("request");
    assert_eq!(status, "answered");
    assert_eq!(answer.expect("answer")["optionId"], "stop");
    assert_eq!(state(setup.pool.clone()).await, "working");
    let resolved = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM run_activities WHERE run_id = $1 AND kind = 'interrupt_resolved'",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("activities");
    assert_eq!(resolved, 2, "the run's log says each was settled");

    // The run hears of both: what it asked shows the answers.
    let read = call("request_get", json!({"requestId": first})).await;
    assert_eq!(
        read["value"]["request"]["answer"]["text"], "Yes, close them",
        "{read}"
    );
}

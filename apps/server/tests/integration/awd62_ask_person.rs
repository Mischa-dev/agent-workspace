//! AWD-62: an agent asks a person something on an inbox card, and the answer
//! goes back to it. The silent failures: someone other than the person asked
//! answering (or reading the answer), an answer changed after the agent
//! acted on it, and the agent never hearing that it was answered.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::add_person,
    notifications::{drain, inbox},
};

#[tokio::test]
async fn an_agent_asks_a_person_and_hears_their_answer() {
    let Some(setup) = setup("awd62_ask").await else {
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
        "projectId": project,
        "person": "@mara",
        "question": "Ship the colour change today?",
        "options": ["Ship it", "Wait for review"],
        "clientRequestId": "ask-mara-1",
    });
    let asked = scout_call("ask_person", ask.clone()).await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    assert_eq!(asked["value"]["status"], "pending");
    let question = asked["value"]["questionId"]
        .as_str()
        .expect("question")
        .to_owned();
    // A retry asks once.
    let again = scout_call("ask_person", ask).await;
    assert_eq!(again["value"]["questionId"], question.as_str(), "{again}");
    assert_eq!(again["value"]["idempotentReplay"], true);

    // Only a person in the project, and not yourself.
    for person in [outsider.to_string(), scout.to_string(), "@other".to_owned()] {
        let refused = scout_call(
            "ask_person",
            json!({"projectId": project, "person": person, "question": "Hello?"}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
        assert_eq!(refused["value"], Value::Null);
    }
    let too_many = scout_call(
        "ask_person",
        json!({"projectId": project, "person": "@mara", "question": "Pick", "options": ["a", "b", "c", "d", "e"]}),
    )
    .await;
    assert_eq!(too_many["outcome"], "invalid_request", "{too_many}");

    // The card is in Mara's inbox, with its options.
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    let card = page["notifications"]
        .as_array()
        .expect("notifications")
        .iter()
        .find(|notification| notification["kind"] == "question_asked")
        .unwrap_or_else(|| panic!("no question card: {page}"))
        .clone();
    assert!(
        card["title"]
            .as_str()
            .is_some_and(|title| title.contains("has a question for you")),
        "{card}"
    );
    assert_eq!(card["question"]["id"], question.as_str());
    assert_eq!(card["question"]["status"], "pending");
    assert_eq!(card["question"]["options"][1]["label"], "Wait for review");

    let answer_uri = format!("/api/v1/questions/{question}/answer");
    // Nobody else can answer it, and the agent that asked can't either.
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &answer_uri,
            Some("owner-answer"),
            json!({"optionId": "1"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    for answered in [
        scout_call(
            "question_answer",
            json!({"questionId": question, "optionId": "1"}),
        )
        .await,
        other_call(
            "question_answer",
            json!({"questionId": question, "optionId": "1"}),
        )
        .await,
    ] {
        assert_eq!(answered["outcome"], "not_found", "{answered}");
    }
    // An option it doesn't offer, or nothing at all, isn't an answer.
    for (key, body) in [
        ("bad-option", json!({"optionId": "9"})),
        ("empty", json!({"text": "   "})),
    ] {
        let (status, refused) = send(
            app,
            mara_browser.request("POST", &answer_uri, Some(key), body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    }
    let (status, answered) = send(
        app,
        mara_browser.request(
            "POST",
            &answer_uri,
            Some("mara-answer"),
            json!({"optionId": "1", "text": "After lunch"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["question"]["status"], "answered");
    assert_eq!(answered["question"]["answer"]["label"], "Ship it");
    // An answer is final.
    let (status, changed) = send(
        app,
        mara_browser.request(
            "POST",
            &answer_uri,
            Some("mara-change"),
            json!({"optionId": "2"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{changed}");
    let (status, replayed) = send(
        app,
        mara_browser.request(
            "POST",
            &answer_uri,
            Some("mara-answer"),
            json!({"optionId": "1", "text": "After lunch"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["question"]["answer"]["optionId"], "1");

    // The answer goes back to Scout: an event, and question_list.
    let events = scout_call(
        "external_event_poll",
        json!({"limit": 50, "waitSeconds": 0}),
    )
    .await;
    let event = events["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["kind"] == "question.answered")
        .unwrap_or_else(|| panic!("no answered event: {events}"))
        .clone();
    assert_eq!(event["resourceId"], question.as_str());
    assert!(!event.to_string().contains("After lunch"), "{event}");
    let listed = scout_call("question_list", json!({"questionId": question})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let line = &listed["value"]["questions"][0];
    assert_eq!(line["status"], "answered");
    assert_eq!(line["answer"]["label"], "Ship it");
    assert_eq!(line["answer"]["text"], "After lunch");
    assert_eq!(line["person"], "mara");

    // Another agent neither hears of it nor reads it.
    let others = other_call("question_list", json!({})).await;
    assert!(!others.to_string().contains(&question), "{others}");
    let others = other_call("question_list", json!({"role": "for_me"})).await;
    assert!(!others.to_string().contains(&question), "{others}");
    let other_events = other_call(
        "external_event_poll",
        json!({"limit": 50, "waitSeconds": 0}),
    )
    .await;
    assert!(
        !other_events.to_string().contains("question.answered"),
        "{other_events}"
    );

    // The card shows the answer and is read.
    let page = inbox(&setup, &mara_browser, "").await;
    let card = page["notifications"]
        .as_array()
        .expect("notifications")
        .iter()
        .find(|notification| notification["kind"] == "question_asked")
        .expect("question card")
        .clone();
    assert_eq!(card["read"], true, "{card}");
    assert_eq!(card["question"]["status"], "answered");
    assert_eq!(card["question"]["answer"]["text"], "After lunch");
}

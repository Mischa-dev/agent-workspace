//! A1.6: the run tools read every run through the run service, so a run its
//! own agent reports shows in `run_get` and `run_list` with its stage in
//! words, and stays out of sight for anyone who can't see it.

use serde_json::{Value, json};

use super::{
    agent_interface::{create_agent, initialize, tool},
    mcp_admin::{create_project, setup},
};

#[tokio::test]
async fn run_get_and_run_list_show_a_self_reported_run_with_its_stage() {
    let Some(setup) = setup("a1_run_reads").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "ELSE", "Elsewhere").await;
    let (_fixer, fixer_token) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let fixer = initialize(app, &fixer_token).await.expect("Fixer MCP");
    let (_stranger, stranger_token) =
        create_agent(app, &setup.browser, &other, "Stranger", "stranger").await;
    let stranger = initialize(app, &stranger_token)
        .await
        .expect("Stranger MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = fixer_token.clone();
        let session = fixer.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let opened = call(
        "run_create",
        json!({"projectId": "WORK", "title": "Triage new feedback",
               "cause": {"kind": "manual"}}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    let updated = call(
        "run_update",
        json!({"runId": run_id, "statusLine": "Reading this week's feedback"}),
    )
    .await;
    assert_eq!(updated["outcome"], "ok", "{updated}");

    let got = call("run_get", json!({"runId": run_id})).await;
    assert_eq!(got["outcome"], "ok", "{got}");
    let run = &got["value"];
    assert_eq!(run["state"], "working", "{run}");
    assert_eq!(run["stage"], "working", "{run}");
    assert_eq!(
        run["next"], "The run is working: Reading this week's feedback",
        "{run}"
    );
    assert_eq!(run["prompt"], "Triage new feedback", "{run}");
    assert_eq!(run["statusLine"], "Reading this week's feedback", "{run}");
    assert!(run.get("threadId").is_none(), "{run}");
    assert!(
        run["url"]
            .as_str()
            .is_some_and(|url| url.ends_with(&format!("/runs/{run_id}"))),
        "{run}"
    );

    let listed = call("run_list", json!({"projectId": "WORK"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let runs = listed["value"]["runs"].as_array().expect("runs");
    assert!(
        runs.iter()
            .any(|listed| listed["id"] == run_id.as_str() && listed["stage"] == "working"),
        "{listed}"
    );

    // Finished, it says so in the words its agent left.
    let done = call(
        "run_update",
        json!({"runId": run_id, "state": "done", "outcomeSummary": "Filed three tasks"}),
    )
    .await;
    assert_eq!(done["outcome"], "ok", "{done}");
    let got = call("run_get", json!({"runId": run_id})).await;
    assert_eq!(got["value"]["stage"], "finished", "{got}");
    assert_eq!(
        got["value"]["next"], "The run finished: Filed three tasks",
        "{got}"
    );
    let active = call("run_list", json!({"projectId": "WORK"})).await;
    assert!(
        !active["value"]["runs"]
            .as_array()
            .expect("runs")
            .iter()
            .any(|listed| listed["id"] == run_id.as_str()),
        "a finished run isn't active: {active}"
    );
    let finished = call(
        "run_list",
        json!({"projectId": "WORK", "state": "finished"}),
    )
    .await;
    assert!(
        finished["value"]["runs"]
            .as_array()
            .expect("runs")
            .iter()
            .any(|listed| listed["id"] == run_id.as_str()),
        "{finished}"
    );

    // An agent outside the project neither gets it nor lists it.
    let hidden = tool(
        app,
        &stranger_token,
        &stranger,
        "run_get",
        json!({"runId": run_id}),
    )
    .await;
    assert_eq!(hidden["outcome"], "not_found", "{hidden}");
    let none = tool(
        app,
        &stranger_token,
        &stranger,
        "run_list",
        json!({"state": "all"}),
    )
    .await;
    assert!(
        !none["value"]["runs"]
            .as_array()
            .expect("runs")
            .iter()
            .any(|listed| listed["id"] == run_id.as_str()),
        "{none}"
    );
}

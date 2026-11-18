//! A1.23: the protocol is enough. The conformance suite, acting as a host,
//! passes against the server; the example host, written only on the public
//! protocol crate, runs an agent's run from start to done.

use agent_workspace_host_conformance::check_server;
use agent_workspace_minimal_host::{AGENT, MinimalHost, enrol};
use axum::http::StatusCode;
use serde_json::json;
use tokio::net::TcpListener;
use uuid::Uuid;

use super::{
    a1_host_protocol::{dispatch, host_op, run_state},
    agent_interface::create_agent,
    mcp_admin::{create_project, setup},
};

/// The app on a local port, for clients that speak HTTP.
async fn serve(app: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    base
}

async fn code(setup: &super::support::world::Setup) -> String {
    let (status, made) = host_op(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "host_enrolment_create",
        json!({"owner": "workspace", "label": "Example host"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    made["code"].as_str().expect("code").to_owned()
}

#[tokio::test]
async fn the_conformance_suite_passes_against_the_server() {
    let Some(setup) = setup("a1_conformance").await else {
        return;
    };
    let base = serve(setup.app.clone()).await;
    let report = check_server(&base, &code(&setup).await).await;
    assert!(report.passed(), "{report}");
}

#[tokio::test]
async fn the_example_host_runs_a_run_to_done() {
    let Some(setup) = setup("a1_example_host").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let space = work.parse::<Uuid>().expect("space");
    let (echo, _) = create_agent(&setup.app, &setup.browser, &work, "Echo", "echo").await;
    let base = serve(setup.app.clone()).await;
    let client = enrol(&base, &code(&setup).await, "Example host")
        .await
        .expect("enrol");
    let mut host = MinimalHost::new(client);
    host.describe().await.expect("describe");
    let host_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM hosts WHERE owner_workspace_id = $1 AND NOT builtin",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("host");
    let (status, bound) = host_op(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "host_agent_bind",
        json!({"hostId": host_id, "accountId": echo, "hostAgentRef": AGENT}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");

    let run = dispatch(&setup.pool, space, echo, "Say hello").await;
    for _ in 0..5 {
        host.step(1).await.expect("commands");
        if run_state(&setup.pool, run).await == "done" {
            break;
        }
    }
    assert_eq!(run_state(&setup.pool, run).await, "done");
    let said = sqlx::query_scalar::<_, String>(
        "SELECT payload->>'text' FROM run_activities WHERE run_id = $1 AND kind = 'text_message'",
    )
    .bind(run)
    .fetch_all(&setup.pool)
    .await
    .expect("activities");
    assert_eq!(said, ["Echo: Say hello"]);
    let used = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(input_tokens), 0)::bigint FROM run_usage WHERE run_id = $1",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("usage");
    assert_eq!(used, 2, "two words in, reported once");
    host.heartbeat().await.expect("heartbeat");
}

//! H1.1: every limit belongs to someone. Junk sign-ins from one address
//! don't stop a correct sign-in from another, and one workspace's agents
//! using up their calls don't refuse another workspace's agent.

use std::sync::Arc;

use crate::support::legacy::router;
use agent_workspace_server::{AppState, rate_limit::RateLimiters};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::json;
use tower::ServiceExt;

use crate::{
    agent_interface::initialize,
    support::{app::PASSWORD, two_workspaces::two_workspaces},
    workspace_isolation::{ORIGIN, call_tool, structured},
};

const ADDRESS_HEADER: &str = "x-test-client-address";

async fn sign_in(
    app: &axum::Router,
    address: &str,
    login_name: &str,
    password: &str,
) -> StatusCode {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", ORIGIN)
                .header(ADDRESS_HEADER, address)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "loginName": login_name, "password": password }))
                        .expect("login body"),
                ))
                .expect("login request"),
        )
        .await
        .expect("login response");
    let status = response.status();
    let _ = to_bytes(response.into_body(), 100_000).await;
    status
}

#[tokio::test]
async fn limits_belong_to_an_address_or_a_workspace_not_the_server() {
    let Some(t) = two_workspaces("hosting_limits").await else {
        return;
    };
    let mut config = (*t.state.config).clone();
    config.client_address_header = Some(ADDRESS_HEADER.to_owned());
    let state = AppState {
        config: Arc::new(config),
        rate_limits: RateLimiters::with_workspace_calls_per_window(3),
        ..t.state.clone()
    };
    let app = router(state);

    // 100 junk sign-ins from one address: none gets in, and the address runs
    // out of its own budget long before the server's.
    for attempt in 0..100 {
        let status = sign_in(
            &app,
            "198.51.100.7",
            &format!("junk-{attempt}"),
            "wrong-password",
        )
        .await;
        assert!(
            matches!(
                status,
                StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS
            ),
            "junk sign-in {attempt}: {status}"
        );
    }
    assert_eq!(
        sign_in(&app, "198.51.100.7", "owner", PASSWORD).await,
        StatusCode::TOO_MANY_REQUESTS,
        "the junk address is out of budget"
    );
    // Someone who hasn't signed in on this server lately, from elsewhere.
    assert_eq!(
        sign_in(&app, "203.0.113.20", "owner", PASSWORD).await,
        StatusCode::OK
    );

    // A's agents get three calls this minute; then A is refused, B isn't.
    let a_session = initialize(&app, &t.a.agent_token).await.expect("A's agent");
    let b_session = initialize(&app, &t.b.agent_token).await.expect("B's agent");
    // The counters are shared (K5.6), so the setup's own calls count too:
    // start both workspaces' minutes afresh.
    sqlx::query("DELETE FROM rate_limit_counters WHERE bucket = 'workspace.calls'")
        .execute(&t.state.pool)
        .await
        .expect("fresh workspace budgets");
    for call in 0..3 {
        let (status, body) =
            call_tool(&app, &t.a.agent_token, &a_session, "task_list", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let result = structured(&body);
        assert_ne!(result["code"], "rate_limited", "call {call}: {body}");
        assert_ne!(result["outcome"], "rate_limited", "call {call}: {body}");
    }
    let (_, body) = call_tool(&app, &t.a.agent_token, &a_session, "task_list", json!({})).await;
    let result = structured(&body);
    assert!(
        result["code"] == "rate_limited" || result["outcome"] == "rate_limited",
        "A's fourth call: {body}"
    );
    let (status, body) =
        call_tool(&app, &t.b.agent_token, &b_session, "task_list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = structured(&body);
    assert_ne!(result["code"], "rate_limited", "{body}");
    assert_ne!(result["outcome"], "rate_limited", "{body}");

    drop(app);
    t.finish().await;
}

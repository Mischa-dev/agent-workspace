//! AWD-63: a one-time link that sets an agent up in the app it runs in. The
//! silent failures: a link that works twice, after it lapsed, or after the
//! agent's token changed; a token left behind in the secrets store; and
//! someone who can't manage agents making one.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    agent_interface::{initialize, send},
    mcp_admin::{create_project, setup},
    message_edits::add_person,
};

async fn fetch(app: &axum::Router, url: &str) -> (StatusCode, String) {
    let path = &url[url.find("/connect/agent/").expect("link path")..];
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let bytes = to_bytes(response.into_body(), 100_000).await.expect("body");
    (status, String::from_utf8(bytes.to_vec()).expect("utf-8"))
}

fn token_in(steps: &str) -> String {
    steps
        .split("Bearer ")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("token in steps")
        .to_owned()
}

async fn waiting_secrets(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM secrets WHERE name LIKE 'agent_setup:%'")
        .fetch_one(pool)
        .await
        .expect("secrets")
}

#[tokio::test]
async fn a_setup_link_hands_the_agent_its_token_once() {
    let Some(setup) = setup("awd63_links").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "SET", "Setting up").await;
    let agents = format!("/api/v1/projects/{project}/external-agents");

    // Adding an agent with an app gives a link, not the token.
    let (status, created) = send(
        app,
        setup.browser.request(
            "POST",
            &agents,
            None,
            json!({
                "connectionName": "Scout",
                "displayName": "Scout",
                "handle": "scout",
                "role": "member",
                "scopes": ["read", "write", "events"],
                "setupApp": "claude_code",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["oneTimeCredential"], Value::Null, "{created}");
    assert_eq!(created["setupLink"]["app"], "claude_code");
    let url = created["setupLink"]["url"]
        .as_str()
        .expect("link")
        .to_owned();
    let connection = created["connection"]["id"].as_str().expect("id").to_owned();
    assert_eq!(waiting_secrets(&setup.pool).await, 1);

    // A HEAD first (a link checker) leaves the link unspent.
    let path = &url[url.find("/connect/agent/").expect("link path")..];
    let peeked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(peeked.status(), StatusCode::OK);
    assert_eq!(waiting_secrets(&setup.pool).await, 1);

    // The first fetch gets the steps with a working token; the second, nothing.
    let (status, steps) = fetch(app, &url).await;
    assert_eq!(status, StatusCode::OK, "{steps}");
    assert!(steps.contains("claude mcp add"), "{steps}");
    assert!(steps.contains("@scout"), "{steps}");
    let first_token = token_in(&steps);
    assert!(initialize(app, &first_token).await.is_some());
    assert_eq!(waiting_secrets(&setup.pool).await, 0);
    let (status, spent) = fetch(app, &url).await;
    assert_eq!(status, StatusCode::GONE);
    assert!(!spent.contains(&first_token));
    let (status, _) = fetch(app, "/connect/agent/not-a-real-code").await;
    assert_eq!(status, StatusCode::GONE);

    // A link for an existing agent is a rotation: the old token stops.
    let version = |value: &Value| value["connection"]["version"].as_i64().expect("version");
    let link_for = |expected: i64, app_name: &'static str| {
        setup.browser.request(
            "POST",
            &format!("{agents}/{connection}/setup-link"),
            None,
            json!({"expectedVersion": expected, "app": app_name}),
        )
    };
    let (status, codex) = send(app, link_for(version(&created), "codex")).await;
    assert_eq!(status, StatusCode::OK, "{codex}");
    assert_eq!(codex["oneTimeCredential"], Value::Null);
    assert!(initialize(app, &first_token).await.is_none());
    let codex_url = codex["setupLink"]["url"].as_str().expect("link").to_owned();

    // Rotating again closes the open link and deletes its token.
    let (status, rotated) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("{agents}/{connection}/credentials/rotate"),
            None,
            json!({"expectedVersion": version(&codex)}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    assert_eq!(waiting_secrets(&setup.pool).await, 0);
    let (status, _) = fetch(app, &codex_url).await;
    assert_eq!(status, StatusCode::GONE);

    // A lapsed link doesn't answer, and its token is purged.
    let (status, other) = send(app, link_for(version(&rotated), "other")).await;
    assert_eq!(status, StatusCode::OK, "{other}");
    sqlx::query(
        "UPDATE agent_setup_links SET expires_at = now() - interval '1 second' WHERE claimed_at IS NULL",
    )
    .execute(&setup.pool)
    .await
    .expect("lapse");
    let (status, _) = fetch(app, other["setupLink"]["url"].as_str().expect("link")).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(waiting_secrets(&setup.pool).await, 0);

    // Only an app the link knows, and only someone who manages agents.
    let (status, unknown) = send(app, link_for(version(&other), "notepad")).await;
    assert!(status.is_client_error(), "{status} {unknown}");
    let (_, member) = add_person(&setup, &[&project], "mara").await;
    let (status, refused) = send(
        app,
        member.request(
            "POST",
            &format!("{agents}/{connection}/setup-link"),
            None,
            json!({"expectedVersion": version(&other), "app": "codex"}),
        ),
    )
    .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status} {refused}"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_setup_links")
            .fetch_one(&setup.pool)
            .await
            .expect("links"),
        3
    );
}

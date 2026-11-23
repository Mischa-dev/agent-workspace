//! Two tool calls on one MCP session at the same time both answer.

use axum::body::to_bytes;
use serde_json::{Value, json};

use crate::support::{
    app::{initialize, mcp},
    two_workspaces::two_workspaces,
};

async fn call(app: &axum::Router, token: &str, session: &str, id: u64) -> Value {
    let response = mcp(
        app,
        token,
        Some(session),
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "briefing_get", "arguments": {} }
        }),
    )
    .await;
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    let line = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text)
        .to_owned();
    serde_json::from_str(&line).unwrap_or_else(|_| panic!("MCP JSON: {text}"))
}

#[tokio::test]
async fn overlapping_calls_on_one_session_both_answer() {
    let Some(t) = two_workspaces("mcp_overlap").await else {
        return;
    };
    let token = t.a.agent_token.clone();
    let session = initialize(&t.app, &token).await.expect("session");
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        tokio::join!(
            call(&t.app, &token, &session, 7),
            call(&t.app, &token, &session, 8)
        )
    })
    .await
    .expect("both calls answer");
    assert_eq!(first["id"], 7, "{first}");
    assert_eq!(second["id"], 8, "{second}");
    assert!(first["result"].is_object(), "{first}");
    assert!(second["result"].is_object(), "{second}");
    t.finish().await;
}

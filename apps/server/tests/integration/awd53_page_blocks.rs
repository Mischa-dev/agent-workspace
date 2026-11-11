//! AWD-53: agents read pages as blocks and change them block by block. The
//! silent, costly failures: an edit built on a stale read landing anyway
//! (overwriting someone's newer text), an edit changing text it didn't name,
//! someone who may only read pages editing one, and blocks of a page in a
//! project the agent isn't in reaching it.

use serde_json::{Value, json};

use super::{
    agent_interface::{connect_app, create_agent, initialize, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
};

const BODY: &str =
    "# Plan\n\nFirst we sketch.\n\n## Risks\n\n- slow CI\n- flaky test\n\n## Later\n\nShip it.\n";

fn hash_at(read: &Value, position: usize) -> String {
    read["value"]["blocks"][position]["hash"]
        .as_str()
        .expect("hash")
        .to_owned()
}

#[tokio::test]
async fn block_edits_change_only_what_was_read_and_only_for_page_editors() {
    let Some(setup) = setup("awd53_blocks").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    create_project(&setup, "SIDE", "Side").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let written = call(
        "page_write",
        json!({"projectId": "WORK", "title": "Plan", "body": BODY}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let page = written["value"]["id"].as_str().expect("page").to_owned();

    // The outline and blocks of version 1, with previews only.
    let read = call("page_blocks", json!({"pageId": page})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["version"], 1);
    let kinds: Vec<&str> = read["value"]["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .map(|block| block["type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        [
            "heading",
            "paragraph",
            "heading",
            "list",
            "heading",
            "paragraph"
        ]
    );
    assert_eq!(read["value"]["outline"][1]["text"], "Risks");
    assert_eq!(read["value"]["outline"][1]["sectionEnd"], 3);
    assert!(read["value"]["blocks"][3]["markdown"].is_null(), "{read}");

    // A section, exactly as stored; positions need the version they came from.
    let section = call(
        "page_blocks",
        json!({"pageId": page, "version": 1, "section": 2}),
    )
    .await;
    assert_eq!(
        section["value"]["markdown"],
        "## Risks\n\n- slow CI\n- flaky test"
    );
    let unversioned = call("page_blocks", json!({"pageId": page, "section": 2})).await;
    assert_eq!(unversioned["outcome"], "invalid_request", "{unversioned}");

    // Replace the list and append a block; the rest stays byte for byte.
    let edited = call(
        "page_edit",
        json!({
            "pageId": page,
            "expectedVersion": 1,
            "note": "Risks updated",
            "edits": [
                {"op": "replace", "position": 3, "hash": hash_at(&read, 3), "markdown": "- slow CI (fixed)\n- flaky test"},
                {"op": "append", "markdown": "## Done\n\nNothing yet."}
            ]
        }),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    assert_eq!(edited["value"]["version"], 2);
    let page_now = call("page_get", json!({"pageId": page})).await;
    assert_eq!(
        page_now["value"]["body"],
        "# Plan\n\nFirst we sketch.\n\n## Risks\n\n- slow CI (fixed)\n- flaky test\n\n## Later\n\nShip it.\n\n## Done\n\nNothing yet.\n"
    );
    assert_eq!(page_now["value"]["history"][0]["note"], "Risks updated");

    // Someone saved since: an edit built on version 1 is refused, and so is
    // one whose hash names text that's no longer there.
    let stale = call(
        "page_edit",
        json!({"pageId": page, "expectedVersion": 1, "edits": [
            {"op": "delete", "position": 1, "hash": hash_at(&read, 1)}
        ]}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let moved = call(
        "page_edit",
        json!({"pageId": page, "expectedVersion": 2, "edits": [
            {"op": "delete", "position": 3, "hash": hash_at(&read, 3)}
        ]}),
    )
    .await;
    assert_eq!(moved["outcome"], "conflict", "{moved}");
    let unchanged = call("page_get", json!({"pageId": page})).await;
    assert_eq!(unchanged["value"]["version"], 2);

    // A viewer may read the blocks but not change them; an agent outside
    // WORK can't even see them.
    let (vic, vic_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Vic", "vic").await;
    add_member(&setup, &work, vic, "viewer").await;
    let vic_session = initialize(app, &vic_token).await.expect("Vic MCP");
    let seen = tool(
        app,
        &vic_token,
        &vic_session,
        "page_blocks",
        json!({"pageId": page}),
    )
    .await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    let refused = tool(
        app,
        &vic_token,
        &vic_session,
        "page_edit",
        json!({"pageId": page, "expectedVersion": 2, "edits": [{"op": "append", "markdown": "Vic was here"}]}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let (_, out_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Out", "out").await;
    let out_session = initialize(app, &out_token).await.expect("Out MCP");
    let hidden = tool(
        app,
        &out_token,
        &out_session,
        "page_blocks",
        json!({"pageId": page}),
    )
    .await;
    assert_ne!(hidden["outcome"], "ok", "{hidden}");
    assert!(!hidden.to_string().contains("slow CI"), "{hidden}");
    let after = call("page_get", json!({"pageId": page})).await;
    assert_eq!(after["value"]["version"], 2, "{after}");
}

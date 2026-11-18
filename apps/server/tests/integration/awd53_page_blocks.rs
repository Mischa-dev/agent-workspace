//! AWD-53: agents read pages as blocks and change them block by block. The
//! silent, costly failures: an edit built on a stale read landing anyway
//! (overwriting someone's newer text), an edit changing text it didn't name,
//! someone who may only read pages editing one, and blocks of a page in a
//! project the agent isn't in reaching it.

use agent_workspace_server::mcp::{IssueCredentialCommand, issue_credential};
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

/// C1.25: a page made from three picked messages quotes them in the order
/// they were sent, with a link back and a made-from link; one made by
/// moving blocks out of a page leaves a link to it in their place, and
/// undoing that change puts the blocks back. Messages in a conversation
/// the maker can't read don't reach the page.
#[tokio::test]
async fn a_page_made_from_messages_or_moved_blocks() {
    let Some(setup) = setup("c1_page_make").await else {
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

    let channel = call(
        "channel_create",
        json!({"projectId": "WORK", "name": "planning"}),
    )
    .await;
    assert_eq!(channel["outcome"], "ok", "{channel}");
    let channel = channel["value"]["id"].as_str().expect("channel").to_owned();
    let mut sent = Vec::new();
    for (index, text) in [
        "First we sketch",
        "Skip this one",
        "Then we *build*",
        "Then we ship",
    ]
    .iter()
    .enumerate()
    {
        let message = call(
            "message_send",
            json!({"conversationId": channel, "text": text, "clientRequestId": format!("make-{index}")}),
        )
        .await;
        assert_eq!(message["outcome"], "ok", "{message}");
        sent.push(
            message["value"]["messageId"]
                .as_str()
                .expect("message")
                .to_owned(),
        );
    }
    // Picked out of order; quoted in the order they were sent.
    let made = call(
        "page_make",
        json!({"messages": [sent[3], sent[0], sent[2]], "clientRequestId": "make-page-1"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert_eq!(made["value"]["title"], "First we sketch", "{made}");
    let page = made["value"]["id"].as_str().expect("page").to_owned();
    let read = call("page_get", json!({"pageId": page})).await;
    let body = read["value"]["body"].as_str().expect("body").to_owned();
    let at = |text: &str| {
        body.find(text)
            .unwrap_or_else(|| panic!("{text} in {body}"))
    };
    assert!(at("First we sketch") < at("Then we *build*"), "{body}");
    assert!(at("Then we *build*") < at("Then we ship"), "{body}");
    assert!(!body.contains("Skip this one"), "{body}");
    assert!(body.starts_with("From [planning]("), "{body}");
    let made_from = sqlx::query_scalar::<_, String>(
        "SELECT kind FROM links WHERE from_thing_id = $1 AND to_thing_id = $2",
    )
    .bind(uuid::Uuid::parse_str(&page).expect("page id"))
    .bind(uuid::Uuid::parse_str(&sent[0]).expect("message id"))
    .fetch_all(&setup.pool)
    .await
    .expect("links");
    assert_eq!(made_from, ["made_from"]);
    // A retry makes no second page.
    let again = call(
        "page_make",
        json!({"messages": [sent[3], sent[0], sent[2]], "clientRequestId": "make-page-1"}),
    )
    .await;
    assert_eq!(again["value"]["id"], made["value"]["id"], "{again}");

    // Blocks moved out of a page leave a link to the new one.
    let written = call(
        "page_write",
        json!({"projectId": "WORK", "title": "Plan", "body": BODY}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let plan = written["value"]["id"].as_str().expect("plan").to_owned();
    let blocks = call("page_blocks", json!({"pageId": plan})).await;
    let id_at = |position: usize| {
        blocks["value"]["blocks"][position]["id"]
            .as_str()
            .expect("block id")
            .to_owned()
    };
    let moved = call(
        "page_make",
        json!({"pageId": plan, "blocks": [id_at(2), id_at(3)], "move": true, "spaceId": work,
               "clientRequestId": "make-page-2"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["title"], "Risks", "{moved}");
    assert_eq!(moved["value"]["sourceVersion"], 2, "{moved}");
    let risks = moved["value"]["id"].as_str().expect("risks").to_owned();
    let new_body = call("page_get", json!({"pageId": risks})).await["value"]["body"].clone();
    assert_eq!(
        new_body, "## Risks\n\n- slow CI\n- flaky test",
        "{new_body}"
    );
    let left = call("page_get", json!({"pageId": plan})).await["value"]["body"]
        .as_str()
        .expect("plan body")
        .to_owned();
    assert!(!left.contains("slow CI"), "{left}");
    assert!(left.contains(&format!("{risks} \"embed\")")), "{left}");

    let changes = call("change_list", json!({"thingKind": "page", "thingId": plan})).await;
    assert_eq!(changes["outcome"], "ok", "{changes}");
    let undo = call(
        "change_undo",
        json!({"changeSetId": changes["value"]["changes"][0]["id"], "clientRequestId": "make-undo"}),
    )
    .await;
    assert_eq!(undo["outcome"], "ok", "{undo}");
    let back = call("page_get", json!({"pageId": plan})).await["value"]["body"]
        .as_str()
        .expect("plan body")
        .to_owned();
    assert_eq!(back, BODY, "{back}");

    // Someone else's DM can't be quoted into a page.
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let not_hers = tool(
        app,
        &mara_token,
        &mara_session,
        "page_make",
        json!({"messages": [sent[0]], "spaceId": work, "clientRequestId": "mara-make"}),
    )
    .await;
    assert_eq!(
        not_hers["outcome"], "ok",
        "a channel she can read: {not_hers}"
    );
    let dm = call(
        "conversation_start",
        json!({"people": ["@mara"], "text": "Just between us"}),
    )
    .await;
    assert_eq!(dm["outcome"], "ok", "{dm}");
    let dm_id = dm["value"]["conversation"]["id"]
        .as_str()
        .expect("dm")
        .to_owned();
    let dm_message =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM messages WHERE conversation_id = $1")
            .bind(uuid::Uuid::parse_str(&dm_id).expect("dm id"))
            .fetch_one(&setup.pool)
            .await
            .expect("dm message");
    let (other, other_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Nate", "nate").await;
    add_member(&setup, &work, other, "member").await;
    let other_session = initialize(app, &other_token).await.expect("Nate MCP");
    let refused = tool(
        app,
        &other_token,
        &other_session,
        "page_make",
        json!({"messages": [dm_message], "spaceId": work, "clientRequestId": "nate-make"}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");

    // A credential kept to WORK can't copy a page from another space its
    // account belongs to, though the page goes in WORK.
    let other_space = create_project(&setup, "ELSE", "Elsewhere").await;
    let elsewhere = call(
        "page_write",
        json!({"projectId": "ELSE", "title": "Secret plan", "body": BODY}),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "ok", "{elsewhere}");
    let elsewhere = elsewhere["value"]["id"].as_str().expect("page").to_owned();
    let first_block = call("page_blocks", json!({"pageId": elsewhere})).await["value"]["blocks"][0]
        ["id"]
        .clone();
    let owner = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' LIMIT 1",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    let confined = issue_credential(
        &setup.pool,
        IssueCredentialCommand {
            actor_id: owner,
            project_id: work.parse().expect("work"),
            client_name: "Confined".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("credential");
    let confined_session = initialize(app, &confined.token).await.expect("session");
    let copied = tool(
        app,
        &confined.token,
        &confined_session,
        "page_make",
        json!({"pageId": elsewhere, "blocks": [first_block], "spaceId": work,
               "clientRequestId": "confined-make"}),
    )
    .await;
    assert_eq!(copied["outcome"], "not_found", "{copied}");
    let from_work = tool(
        app,
        &confined.token,
        &confined_session,
        "page_make",
        json!({"pageId": plan, "blocks": [id_at(0)], "spaceId": work,
               "clientRequestId": "confined-make-work"}),
    )
    .await;
    assert_eq!(from_work["outcome"], "ok", "{from_work}");
    assert!(!other_space.is_empty());
}

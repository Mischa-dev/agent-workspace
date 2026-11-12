//! AWD-53 slice 3: pages are live documents that people and agents edit at
//! the same time. The silent, costly failures: someone's typing lost when an
//! agent edits the page (or the other way round), an agent undoing a change
//! it never saw, block ids changing under an agent, a page looking different
//! after it's converted, a browser slipping in content the editor can't make,
//! and someone who may only read a page (or can't see it) changing it.

use agent_workspace_server::page_docs::{
    tree::{from_markdown, strip_ids},
    ydoc,
};
use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;
use yrs::{
    ClientID, Doc, GetString, OffsetKind, Options, ReadTxn, StateVector, Text, Transact, Update,
    XmlElementPrelim, XmlFragment, XmlOut,
    updates::{decoder::Decode, encoder::Encode},
};

use super::{
    agent_interface::{Browser, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::add_person,
};

const BODY: &str =
    "# Plan\n\nFirst we sketch.\n\n## Risks\n\n- slow CI\n- flaky test\n\nShip it.\n";

/// A person's open editor: their copy of the page's document.
struct Editor {
    doc: Doc,
    generation: String,
}

impl Editor {
    async fn open(setup: &Setup, browser: &Browser, page: &str, client: u64) -> (Self, Value) {
        let (status, document) = send(
            &setup.app,
            browser.request(
                "GET",
                &format!("/api/v1/pages/{page}/document"),
                None,
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{document}");
        let doc = Doc::with_options(Options {
            client_id: ClientID::new(client),
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        apply(&doc, document["state"].as_str().expect("state"));
        let generation = document["generation"]
            .as_str()
            .expect("generation")
            .to_owned();
        (Self { doc, generation }, document)
    }

    fn state_vector(&self) -> String {
        STANDARD.encode(self.doc.transact().state_vector().encode_v1())
    }

    /// Types `text` at `at` in top-level block `block` (a paragraph or heading).
    fn type_text(&self, block: u32, at: u32, text: &str) {
        let fragment = self.doc.get_or_insert_xml_fragment("default");
        let mut txn = self.doc.transact_mut();
        let Some(XmlOut::Element(element)) = fragment.get(&txn, block) else {
            panic!("no block {block}");
        };
        let Some(XmlOut::Text(content)) = element.get(&txn, 0) else {
            panic!("block {block} has no text");
        };
        content.insert(&mut txn, at, text);
    }

    fn text_of(&self, block: u32) -> String {
        let fragment = self.doc.get_or_insert_xml_fragment("default");
        let txn = self.doc.transact();
        let Some(XmlOut::Element(element)) = fragment.get(&txn, block) else {
            panic!("no block {block}");
        };
        let Some(XmlOut::Text(content)) = element.get(&txn, 0) else {
            panic!("block {block} has no text");
        };
        content.get_string(&txn)
    }

    /// Sends everything the server lacks, given its state vector `known`
    /// (base64), and applies what comes back.
    async fn send_changes(
        &self,
        setup: &Setup,
        browser: &Browser,
        page: &str,
        known: &str,
    ) -> (StatusCode, Value) {
        let known =
            StateVector::decode_v1(&STANDARD.decode(known).expect("base64")).expect("state vector");
        let update = self.doc.transact().encode_diff_v1(&known);
        let (status, answer) = send(
            &setup.app,
            browser.request(
                "POST",
                &format!("/api/v1/pages/{page}/document/updates"),
                None,
                json!({
                    "generation": self.generation,
                    "update": STANDARD.encode(update),
                    "stateVector": self.state_vector(),
                }),
            ),
        )
        .await;
        if status == StatusCode::OK && answer["status"] == "synced" {
            apply(&self.doc, answer["update"].as_str().expect("update"));
        }
        (status, answer)
    }

    fn blocks(&self) -> Vec<agent_workspace_server::page_docs::tree::Block> {
        ydoc::read_doc(&self.doc).expect("the editor's copy reads")
    }
}

fn apply(doc: &Doc, update: &str) {
    let update = Update::decode_v1(&STANDARD.decode(update).expect("base64")).expect("update");
    doc.transact_mut().apply_update(update).expect("applies");
}

async fn page_row(setup: &Setup, page: &str) -> (String, i64, i64) {
    sqlx::query_as::<_, (String, i64, i64)>(
        r#"
        SELECT page.body, page.version,
               (SELECT count(*) FROM project_page_revisions revision WHERE revision.page_id = page.id)
        FROM project_pages page WHERE page.id = $1
        "#,
    )
    .bind(page.parse::<Uuid>().expect("page"))
    .fetch_one(&setup.pool)
    .await
    .expect("page row")
}

async fn server_blocks(
    setup: &Setup,
    page: &str,
) -> Vec<agent_workspace_server::page_docs::tree::Block> {
    let state = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT state FROM project_page_documents WHERE page_id = $1",
    )
    .bind(page.parse::<Uuid>().expect("page"))
    .fetch_one(&setup.pool)
    .await
    .expect("stored document");
    ydoc::read_doc(&ydoc::load(&state).expect("loads")).expect("reads")
}

fn ids(read: &Value) -> Vec<String> {
    read["value"]["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .map(|block| block["id"].as_str().expect("id").to_owned())
        .collect()
}

async fn create_page(setup: &Setup, project: &str, body: &str) -> String {
    let (status, page) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/pages"),
            None,
            json!({"title": "Plan", "body": body}),
        ),
    )
    .await;
    assert!(status.is_success(), "{page}");
    page["id"].as_str().expect("page").to_owned()
}

#[tokio::test]
async fn two_people_and_an_agent_edit_one_page_at_once_without_losing_text() {
    let Some(setup) = setup("awd53_live").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let page = create_page(&setup, &work, BODY).await;
    let (_, lena) = add_person(&setup, &[&work], "lena").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    // The page is Mara's, so her edits change it directly; on a page someone
    // else made they'd be suggestions (awd53_discussion).
    sqlx::query("UPDATE project_pages SET created_by_actor_id = $2 WHERE id = $1::uuid")
        .bind(&page)
        .bind(mara)
        .execute(&setup.pool)
        .await
        .expect("Mara's page");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    // Opening the page converts it, and nothing anyone can see changes.
    let (owner_editor, opened) = Editor::open(&setup, &setup.browser, &page, 101).await;
    assert_eq!(opened["version"], 1, "{opened}");
    assert_eq!(opened["canEdit"], true, "{opened}");
    assert_eq!(page_row(&setup, &page).await, (BODY.to_owned(), 1, 1));
    assert_eq!(
        strip_ids(&owner_editor.blocks()),
        from_markdown(BODY),
        "the document is the page as it was drawn"
    );
    let (lena_editor, _) = Editor::open(&setup, &lena, &page, 202).await;
    let lena_known = lena_editor.state_vector();
    let owner_known = owner_editor.state_vector();

    // The agent reads the blocks: every one has an id.
    let read = call("page_blocks", json!({"pageId": page})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let before = ids(&read);
    assert_eq!(before.len(), 5, "{read}");
    assert!(before.iter().all(|id| ydoc::valid_block_id(id)), "{read}");

    // Lena types in the first paragraph and the owner in the last one, both
    // unsent, while the agent rewrites the list by its id, with no version.
    lena_editor.type_text(1, 15, " together");
    owner_editor.type_text(4, 8, " Soon");
    let edited = call(
        "page_edit",
        json!({"pageId": page, "edits": [{
            "op": "replace",
            "blockId": before[3],
            "hash": read["value"]["blocks"][3]["hash"],
            "markdown": "- slow CI (fixed)\n- flaky test"
        }]}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    assert_eq!(edited["value"]["version"], 2, "{edited}");

    // Both people's changes reach the server, and each gets the others'.
    let (status, synced) = lena_editor
        .send_changes(&setup, &lena, &page, &lena_known)
        .await;
    assert_eq!(status, StatusCode::OK, "{synced}");
    assert_eq!(synced["status"], "synced", "{synced}");
    assert_eq!(synced["version"], 3, "{synced}");
    let (status, synced) = owner_editor
        .send_changes(&setup, &setup.browser, &page, &owner_known)
        .await;
    assert_eq!(status, StatusCode::OK, "{synced}");
    assert_eq!(synced["version"], 4, "{synced}");
    let (status, caught_up) = send(
        app,
        lena.request(
            "POST",
            &format!("/api/v1/pages/{page}/document/sync"),
            None,
            json!({"generation": lena_editor.generation, "stateVector": lena_editor.state_vector()}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{caught_up}");
    apply(
        &lena_editor.doc,
        caught_up["update"].as_str().expect("update"),
    );

    let expected = "# Plan\n\nFirst we sketch together.\n\n## Risks\n\n- slow CI (fixed)\n- flaky test\n\nShip it. Soon\n";
    let (body, version, _) = page_row(&setup, &page).await;
    assert_eq!(body, expected);
    assert_eq!(version, 4);
    let on_server = server_blocks(&setup, &page).await;
    assert_eq!(lena_editor.blocks(), on_server, "Lena's copy matches");
    assert_eq!(owner_editor.blocks(), on_server, "the owner's copy matches");
    assert_eq!(lena_editor.text_of(4), "Ship it. Soon");

    // Ids lasted through all of it; the replaced list kept its own.
    let read_again = call("page_blocks", json!({"pageId": page})).await;
    assert_eq!(ids(&read_again), before, "{read_again}");

    // History: the agent's edit and each person's typing are their own
    // versions, and more typing by the same person merges into theirs.
    let history = call("page_get", json!({"pageId": page})).await;
    let authors: Vec<&str> = history["value"]["history"]
        .as_array()
        .expect("history")
        .iter()
        .map(|line| line["author"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(authors, ["Owner", "lena", "Mara", "Owner"], "{history}");
    let owner_known = owner_editor.state_vector();
    owner_editor.type_text(4, 13, "!");
    let (_, synced) = owner_editor
        .send_changes(&setup, &setup.browser, &page, &owner_known)
        .await;
    assert_eq!(synced["version"], 5, "{synced}");
    let (body, version, revisions) = page_row(&setup, &page).await;
    assert!(body.contains("Ship it. Soon!"), "{body}");
    assert_eq!(
        (version, revisions),
        (5, 4),
        "merged into the owner's version"
    );

    // Every change told open pages about it.
    let events = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events WHERE event_type = 'page.updated' AND payload->>'pageId' = $1",
    )
    .bind(&page)
    .fetch_one(&setup.pool)
    .await
    .expect("events");
    assert!(events >= 5, "{events} page.updated events");

    // The page's Markdown round-trips: written back as it is, nothing changes,
    // and it reads back as the document.
    let (body, _, _) = page_row(&setup, &page).await;
    assert_eq!(
        from_markdown(&body),
        strip_ids(&server_blocks(&setup, &page).await)
    );
    let rewritten = call(
        "page_write",
        json!({"pageId": page, "body": body, "expectedVersion": 5}),
    )
    .await;
    assert_eq!(rewritten["outcome"], "ok", "{rewritten}");
    assert_eq!(rewritten["value"]["version"], 5, "{rewritten}");

    // Rewriting the whole page keeps the ids of blocks that stay, and of a
    // block edited in place.
    let rewritten = call(
        "page_write",
        json!({"pageId": page, "body": body.replace("First we sketch together.", "We sketch first."), "expectedVersion": 5}),
    )
    .await;
    assert_eq!(rewritten["outcome"], "ok", "{rewritten}");
    let read_again = call("page_blocks", json!({"pageId": page})).await;
    assert_eq!(ids(&read_again), before, "{read_again}");
}

#[tokio::test]
async fn edits_that_would_lose_text_or_the_editor_cant_make_are_refused() {
    let Some(setup) = setup("awd53_refused").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let page = create_page(&setup, &work, BODY).await;
    let (_, lena) = add_person(&setup, &[&work], "lena").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    // The agent reads; then Lena types in the paragraph it means to replace.
    let read = call("page_blocks", json!({"pageId": page})).await;
    let before = ids(&read);
    let (editor, _) = Editor::open(&setup, &lena, &page, 303).await;
    let known = editor.state_vector();
    editor.type_text(1, 0, "Lena: ");
    let (status, synced) = editor.send_changes(&setup, &lena, &page, &known).await;
    assert_eq!(
        (status, synced["status"].as_str()),
        (StatusCode::OK, Some("synced"))
    );
    let stale = call(
        "page_edit",
        json!({"pageId": page, "edits": [{
            "op": "replace", "blockId": before[1],
            "hash": read["value"]["blocks"][1]["hash"], "markdown": "Agent text."
        }]}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let (body, version, _) = page_row(&setup, &page).await;
    assert!(body.contains("Lena: First we sketch."), "{body}");
    assert_eq!(version, 2);
    // Without a hash, replacing or deleting isn't allowed at all.
    let blind = call(
        "page_edit",
        json!({"pageId": page, "edits": [{"op": "delete", "blockId": before[1]}]}),
    )
    .await;
    assert_eq!(blind["outcome"], "invalid_request", "{blind}");

    // A browser can't slip in what the editor can't make.
    let known = editor.state_vector();
    {
        let fragment = editor.doc.get_or_insert_xml_fragment("default");
        let mut txn = editor.doc.transact_mut();
        fragment.insert(&mut txn, 0, XmlElementPrelim::empty("script"));
    }
    let (status, refused) = editor.send_changes(&setup, &lena, &page, &known).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(page_row(&setup, &page).await.1, 2, "nothing saved");

    // A copy from another generation is told to load again, and an update
    // built on changes the server never had is asked to resync.
    let (status, replaced) = send(
        app,
        lena.request(
            "POST",
            &format!("/api/v1/pages/{page}/document/updates"),
            None,
            json!({"generation": Uuid::now_v7(), "update": STANDARD.encode([0u8, 0]), "stateVector": STANDARD.encode([0u8])}),
        ),
    )
    .await;
    assert_eq!(
        (status, replaced["status"].as_str()),
        (StatusCode::OK, Some("replaced")),
        "{replaced}"
    );
    let elsewhere = Doc::with_options(Options {
        client_id: ClientID::new(404),
        ..Options::default()
    });
    {
        let text = elsewhere.get_or_insert_text("scratch");
        let mut txn = elsewhere.transact_mut();
        text.insert(&mut txn, 0, "a");
    }
    let base = elsewhere.transact().state_vector();
    {
        let text = elsewhere.get_or_insert_text("scratch");
        let mut txn = elsewhere.transact_mut();
        text.insert(&mut txn, 1, "b");
    }
    let dependent = elsewhere.transact().encode_diff_v1(&base);
    let (status, resync) = send(
        app,
        lena.request(
            "POST",
            &format!("/api/v1/pages/{page}/document/updates"),
            None,
            json!({"generation": editor.generation, "update": STANDARD.encode(dependent), "stateVector": editor.state_vector()}),
        ),
    )
    .await;
    assert_eq!(
        (status, resync["status"].as_str()),
        (StatusCode::OK, Some("resync")),
        "{resync}"
    );
    assert!(resync["stateVector"].is_string(), "{resync}");

    // Someone who may only read the page can open and follow it, not change it.
    let (viewer, viewer_browser) = add_person(&setup, &[&work], "vic").await;
    sqlx::query("UPDATE project_memberships SET role = 'viewer' WHERE actor_id = $1")
        .bind(viewer)
        .execute(&setup.pool)
        .await
        .expect("viewer");
    let (viewer_editor, opened) = Editor::open(&setup, &viewer_browser, &page, 505).await;
    assert_eq!(opened["canEdit"], false, "{opened}");
    let known = viewer_editor.state_vector();
    viewer_editor.type_text(0, 0, "Vic ");
    let (status, _) = viewer_editor
        .send_changes(&setup, &viewer_browser, &page, &known)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        app,
        viewer_browser.request(
            "POST",
            &format!("/api/v1/pages/{page}/document/sync"),
            None,
            json!({"generation": viewer_editor.generation, "stateVector": known}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Someone outside the project can't even see it.
    let side = create_project(&setup, "SIDE", "Side").await;
    let (_, outsider) = add_person(&setup, &[&side], "otto").await;
    let (status, hidden) = send(
        app,
        outsider.request(
            "GET",
            &format!("/api/v1/pages/{page}/document"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");
    assert!(!hidden.to_string().contains("sketch"), "{hidden}");
    assert_eq!(page_row(&setup, &page).await.1, 2, "still nothing saved");
}

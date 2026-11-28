//! AWD-223: the Docs tree. A space's pages, sheets, whiteboards,
//! collections and dashboards sit in one tree of folders, and a document or
//! a folder can be restricted to named people. A restricted document
//! doesn't exist for anyone else in the space, a person or an agent alike:
//! not by id, not in its kind's list, not in the tree, not in search.

use agent_workspace_server::realtime::documents::{DocumentKind, DocumentRef, can_view};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{send, tool},
    world::{Workspace, WorldBuilder},
};

/// Calls `name` as `who`: a person through the v2 route, an agent through
/// its MCP tool. Both answer `{outcome, value}`.
async fn op(world: &Workspace, who: &str, name: &str, input: Value) -> Value {
    if let Some(agent) = world.agents.get(who) {
        let mut input = input;
        if name.ends_with("_create") {
            input["clientRequestId"] = json!(Uuid::now_v7().to_string());
        }
        return tool(&world.app, &agent.token, &agent.session, name, input).await;
    }
    let browser = if who == "owner" {
        &world.owner.browser
    } else {
        &world.people[who].browser
    };
    let route = format!("/api/v2/w/{}/ops/{name}", world.organization);
    let key = Uuid::now_v7().to_string();
    send(
        &world.app,
        browser.request("POST", &route, Some(&key), input),
    )
    .await
    .1
}

fn ok(answer: &Value) -> &Value {
    assert_eq!(answer["outcome"], "ok", "{answer}");
    &answer["value"]
}

fn id_at(answer: &Value, pointer: &str) -> String {
    ok(answer)
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no {pointer} in {answer}"))
        .to_owned()
}

/// What a public link answers someone who isn't signed in.
async fn link_opens(world: &Workspace, path: &str) -> StatusCode {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .header(axum::http::header::HOST, "127.0.0.1")
        .body(axum::body::Body::empty())
        .expect("request");
    send(&world.app, request).await.0
}

/// Whether a live connection lets `who` watch the page, the sheet and the
/// whiteboard. A socket is served for no account in particular, as here.
async fn watches(world: &Workspace, who: &str, docs: &Docs) -> bool {
    let account = world.people[who].actor;
    let mut seen = Vec::new();
    for (kind, id) in [
        (DocumentKind::Page, &docs.page),
        (DocumentKind::Sheet, &docs.sheet),
        (DocumentKind::Whiteboard, &docs.board),
    ] {
        let document = DocumentRef {
            kind,
            id: id.parse().expect("id"),
        };
        seen.push(
            agent_workspace_server::tenancy::system(
                "realtime.connections",
                can_view(world.pool(), account, document),
            )
            .await
            .expect("can view"),
        );
    }
    assert!(
        seen.iter().all(|one| *one == seen[0]),
        "{who} watches some and not others: {seen:?}"
    );
    seen[0]
}

/// One document of each kind, made by the owner.
struct Docs {
    page: String,
    sheet: String,
    board: String,
    collection: String,
    dashboard: String,
    /// The word every title carries, for searching.
    word: String,
}

impl Docs {
    fn all(&self) -> [(&'static str, &str); 5] {
        [
            ("page", &self.page),
            ("sheet", &self.sheet),
            ("whiteboard", &self.board),
            ("collection", &self.collection),
            ("dashboard", &self.dashboard),
        ]
    }
}

async fn make_docs(world: &Workspace, space: &str, word: &str) -> Docs {
    let page = op(
        world,
        "owner",
        "page_create",
        json!({"projectId": space, "title": format!("{word} page"), "body": format!("About {word}.")}),
    )
    .await;
    let sheet = op(
        world,
        "owner",
        "sheet_create",
        json!({"projectId": space, "name": format!("{word} sheet")}),
    )
    .await;
    let board = op(
        world,
        "owner",
        "whiteboard_create",
        json!({"projectId": space, "title": format!("{word} board")}),
    )
    .await;
    let collection = op(
        world,
        "owner",
        "collection_create",
        json!({"projectId": space, "name": format!("{word} collection")}),
    )
    .await;
    let dashboard = op(
        world,
        "owner",
        "dashboard_create",
        json!({"spaceId": space, "name": format!("{word} dashboard"), "shared": true}),
    )
    .await;
    Docs {
        page: id_at(&page, "/id"),
        sheet: id_at(&sheet, "/id"),
        board: id_at(&board, "/whiteboard/id"),
        collection: id_at(&collection, "/collection/id"),
        dashboard: id_at(&dashboard, "/id"),
        word: word.to_owned(),
    }
}

/// The tree as `who` sees it, the whole way down.
async fn tree(world: &Workspace, who: &str, space: &str) -> Vec<Value> {
    let answer = op(
        world,
        who,
        "doc_list",
        json!({"spaceId": space, "deep": true}),
    )
    .await;
    ok(&answer)["entries"].as_array().expect("entries").clone()
}

fn entry<'a>(entries: &'a [Value], id: &str) -> Option<&'a Value> {
    entries.iter().find(|entry| entry["id"] == id)
}

/// Whether `who` can open the document by id with its kind's own tool.
async fn opens(world: &Workspace, who: &str, space: &str, kind: &str, id: &str) -> bool {
    let answer = match kind {
        "page" => op(world, who, "page_get", json!({"pageId": id})).await,
        "sheet" => {
            op(
                world,
                who,
                "sheet_get",
                json!({"projectId": space, "sheet": id}),
            )
            .await
        }
        "whiteboard" => {
            if world.agents.contains_key(who) {
                op(world, who, "whiteboard_read", json!({"whiteboardId": id})).await
            } else {
                // A person reads a board through its own route.
                let browser = if who == "owner" {
                    &world.owner.browser
                } else {
                    &world.people[who].browser
                };
                let (status, _) = send(
                    &world.app,
                    browser.request(
                        "GET",
                        &format!("/api/v1/whiteboards/{id}"),
                        None,
                        json!(null),
                    ),
                )
                .await;
                return status.is_success();
            }
        }
        "collection" => {
            op(
                world,
                who,
                "collection_get",
                json!({"projectId": space, "collection": id}),
            )
            .await
        }
        "dashboard" => op(world, who, "dashboard_get", json!({"dashboard": id})).await,
        other => panic!("no kind {other}"),
    };
    answer["outcome"] == "ok"
}

/// Whether the document shows in its kind's own list for `who`.
async fn listed(world: &Workspace, who: &str, space: &str, kind: &str, id: &str) -> bool {
    let answer = match kind {
        "page" => op(world, who, "page_list", json!({"projectId": space})).await,
        "sheet" => op(world, who, "sheet_list", json!({"projectId": space})).await,
        "whiteboard" => {
            if !world.agents.contains_key(who) {
                let browser = if who == "owner" {
                    &world.owner.browser
                } else {
                    &world.people[who].browser
                };
                let (status, body) = send(
                    &world.app,
                    browser.request(
                        "GET",
                        &format!("/api/v1/whiteboards?projectId={space}"),
                        None,
                        json!(null),
                    ),
                )
                .await;
                assert!(status.is_success(), "{body}");
                return body.to_string().contains(id);
            }
            op(world, who, "whiteboard_list", json!({"projectId": space})).await
        }
        "collection" => op(world, who, "collection_list", json!({"projectId": space})).await,
        "dashboard" => op(world, who, "dashboard_list", json!({"spaceId": space})).await,
        other => panic!("no kind {other}"),
    };
    assert_eq!(answer["outcome"], "ok", "{answer}");
    answer.to_string().contains(id)
}

/// Checks that `who` finds every document everywhere (`sees`) or nowhere.
async fn check(world: &Workspace, who: &str, space: &str, docs: &Docs, sees: bool) {
    let entries = tree(world, who, space).await;
    for (kind, id) in docs.all() {
        assert_eq!(
            entry(&entries, id).is_some(),
            sees,
            "{who} and the {kind} in the tree"
        );
        assert_eq!(
            opens(world, who, space, kind, id).await,
            sees,
            "{who} opening the {kind}"
        );
        assert_eq!(
            listed(world, who, space, kind, id).await,
            sees,
            "{who} listing the {kind}"
        );
        let access = op(world, who, "doc_access_get", json!({"id": id})).await;
        assert_eq!(access["outcome"] == "ok", sees, "{who}: {access}");
    }
    // Search finds a title only for someone who may open the document.
    let found = op(
        world,
        who,
        "workspace_search",
        json!({"query": docs.word, "projectId": space}),
    )
    .await;
    assert_eq!(found["outcome"], "ok", "{found}");
    let found = found.to_string();
    assert_eq!(found.contains(&docs.page), sees, "{who} searching: {found}");
    if !sees {
        for (kind, id) in docs.all() {
            assert!(!found.contains(id), "{who} found the {kind}: {found}");
        }
    }
}

#[tokio::test]
async fn folders_hold_every_kind_and_move_like_one_tree() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("vera", "viewer")
        .agent("rook", "member")
        .build("awd223_tree")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let docs = make_docs(&world, &work, "Atlas").await;

    // Everything starts at the top, open to the space.
    let entries = tree(&world, "lena", &work).await;
    for (kind, id) in docs.all() {
        let found = entry(&entries, id).unwrap_or_else(|| panic!("no {kind} in {entries:?}"));
        assert_eq!(found["kind"], kind);
        assert!(found.get("parentId").is_none(), "{found}");
        assert_eq!(found["restricted"], false);
    }

    // A folder inside a folder; an agent makes one the same way.
    let plans = id_at(
        &op(
            &world,
            "lena",
            "doc_folder_create",
            json!({"spaceId": work, "title": "Plans"}),
        )
        .await,
        "/id",
    );
    let drafts = id_at(
        &op(
            &world,
            "rook",
            "doc_folder_create",
            json!({"spaceId": work, "title": "Drafts", "parentFolderId": plans}),
        )
        .await,
        "/id",
    );

    // Every kind goes into a folder with the one tool, by a person or an
    // agent.
    for (index, (kind, id)) in docs.all().into_iter().enumerate() {
        let who = if index % 2 == 0 { "lena" } else { "rook" };
        let moved = op(
            &world,
            who,
            "doc_move",
            json!({"id": id, "folderId": drafts}),
        )
        .await;
        assert_eq!(ok(&moved)["parentId"], drafts, "{kind}: {moved}");
    }
    let entries = tree(&world, "rook", &work).await;
    assert_eq!(entry(&entries, &drafts).expect("drafts")["parentId"], plans);
    for (_, id) in docs.all() {
        assert_eq!(entry(&entries, id).expect("doc")["parentId"], drafts);
    }

    // One folder's own contents, and the top of the tree.
    let inside = op(
        &world,
        "lena",
        "doc_list",
        json!({"spaceId": work, "folderId": plans}),
    )
    .await;
    let inside = ok(&inside)["entries"].as_array().expect("entries");
    assert_eq!(inside.len(), 1, "{inside:?}");
    assert_eq!(inside[0]["id"], drafts);
    let top = op(&world, "lena", "doc_list", json!({"spaceId": work})).await;
    assert!(
        ok(&top)["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .all(|entry| entry.get("parentId").is_none()),
        "{top}"
    );

    // A folder can't go inside itself or what it holds, and only an empty
    // one is removed.
    let looped = op(
        &world,
        "lena",
        "doc_move",
        json!({"id": plans, "folderId": drafts}),
    )
    .await;
    assert_eq!(looped["outcome"], "invalid_request", "{looped}");
    let kept = op(
        &world,
        "lena",
        "doc_folder_update",
        json!({"folderId": drafts, "remove": true}),
    )
    .await;
    assert_eq!(kept["outcome"], "invalid_request", "{kept}");

    // Someone who only reads can look, and can't move or file.
    assert!(entry(&tree(&world, "vera", &work).await, &docs.sheet).is_some());
    let refused = op(&world, "vera", "doc_move", json!({"id": docs.sheet})).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let refused = op(
        &world,
        "vera",
        "doc_folder_create",
        json!({"spaceId": work, "title": "Mine"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // Back to the top, then the empty folders go.
    for (_, id) in docs.all() {
        ok(&op(&world, "lena", "doc_move", json!({"id": id})).await);
    }
    let renamed = op(
        &world,
        "lena",
        "doc_folder_update",
        json!({"folderId": drafts, "title": "Old drafts"}),
    )
    .await;
    assert_eq!(ok(&renamed)["title"], "Old drafts");
    for folder in [&drafts, &plans] {
        let removed = op(
            &world,
            "lena",
            "doc_folder_update",
            json!({"folderId": folder, "remove": true}),
        )
        .await;
        assert_eq!(ok(&removed)["removed"], true);
    }
    let entries = tree(&world, "lena", &work).await;
    assert!(entry(&entries, &plans).is_none() && entry(&entries, &drafts).is_none());
}

#[tokio::test]
async fn a_restricted_folder_hides_what_it_holds_from_everyone_not_named() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .agent("wren", "member")
        .build("awd223_restricted")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let lena = world.people["lena"].actor;
    let rook = world.agents["rook"].actor;
    let docs = make_docs(&world, &work, "Quasar").await;
    let open = make_docs(&world, &work, "Harbor").await;

    let vault = id_at(
        &op(
            &world,
            "owner",
            "doc_folder_create",
            json!({"spaceId": work, "title": "Vault"}),
        )
        .await,
        "/id",
    );
    for (_, id) in docs.all() {
        ok(&op(
            &world,
            "owner",
            "doc_move",
            json!({"id": id, "folderId": vault}),
        )
        .await);
    }
    // In a folder, still open to the space.
    for who in ["lena", "mara", "rook", "wren"] {
        check(&world, who, &work, &docs, true).await;
    }
    // A record template on the collection, which anyone who builds
    // collections can rename while the collection is open.
    let template = id_at(
        &op(
            &world,
            "owner",
            "record_template_create",
            json!({"collection": docs.collection, "name": "Weekly", "title": "Quasar check-in"}),
        )
        .await,
        "/id",
    );
    ok(&op(
        &world,
        "mara",
        "record_template_update",
        json!({"template": template, "name": "Each week"}),
    )
    .await);

    // A link to the page, made while it was open, reads it without signing in.
    let link = op(
        &world,
        "owner",
        "thing_link_create",
        json!({"kind": "page", "id": docs.page, "public": true}),
    )
    .await;
    let link = format!("/api/v1/public/pages/{}", id_at(&link, "/token"));
    assert_eq!(link_opens(&world, &link).await, StatusCode::OK);
    for who in ["lena", "mara"] {
        assert!(watches(&world, who, &docs).await, "{who}");
    }

    // Restricted to lena and the agent rook; the owner who did it stays on.
    let access = op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": vault, "restricted": true, "people": [lena, rook]}),
    )
    .await;
    let named = ok(&access)["people"].as_array().expect("people");
    assert_eq!(named.len(), 3, "{access}");
    assert_eq!(ok(&access)["restricted"], true);

    for who in ["owner", "lena", "rook"] {
        check(&world, who, &work, &docs, true).await;
        let entries = tree(&world, who, &work).await;
        assert_eq!(
            entry(&entries, &vault).expect("vault")["restrictedHere"],
            true
        );
        let inside = entry(&entries, &docs.page).expect("page");
        assert_eq!(inside["restricted"], true, "{inside}");
        assert_eq!(inside["restrictedHere"], false, "{inside}");
    }
    // The link is held by whoever has it, so it stops opening the page, and
    // a live connection only watches for someone named.
    assert_eq!(link_opens(&world, &link).await, StatusCode::NOT_FOUND);
    assert!(watches(&world, "lena", &docs).await);
    assert!(!watches(&world, "mara", &docs).await);
    // The collection's templates go with it: a rename by id is refused and
    // answers nothing of the template.
    for who in ["mara", "wren"] {
        let answer = op(
            &world,
            who,
            "record_template_update",
            json!({"template": template, "name": "Seen"}),
        )
        .await;
        assert_ne!(answer["outcome"], "ok", "{who}: {answer}");
        assert!(!answer.to_string().contains("Quasar"), "{who}: {answer}");
    }
    ok(&op(
        &world,
        "lena",
        "record_template_update",
        json!({"template": template, "name": "Weekly"}),
    )
    .await);
    // Everyone else in the space: nothing, anywhere, for a person and an
    // agent alike. What isn't restricted is as it was.
    for who in ["mara", "wren"] {
        check(&world, who, &work, &docs, false).await;
        check(&world, who, &work, &open, true).await;
        assert!(entry(&tree(&world, who, &work).await, &vault).is_none());
        let found = op(
            &world,
            who,
            "workspace_search",
            json!({"query": "Quasar", "projectId": work}),
        )
        .await;
        for (kind, id) in docs.all() {
            assert!(
                !found.to_string().contains(id),
                "{who} found the {kind}: {found}"
            );
        }
        let moved = op(&world, who, "doc_move", json!({"id": docs.sheet})).await;
        assert_eq!(moved["outcome"], "not_found", "{moved}");
        let into = op(
            &world,
            who,
            "doc_move",
            json!({"id": open.sheet, "folderId": vault}),
        )
        .await;
        assert_ne!(into["outcome"], "ok", "{into}");
        let said = op(
            &world,
            who,
            "page_comment_create",
            json!({"pageId": docs.page, "text": "Can I see this?"}),
        )
        .await;
        assert_ne!(said["outcome"], "ok", "{said}");
    }
    // An agent opening it as a thing by id alone gets nothing either.
    let thing = op(&world, "wren", "thing_open", json!({"id": docs.page})).await;
    assert_ne!(thing["outcome"], "ok", "{thing}");
    assert!(!thing.to_string().contains("Quasar"), "{thing}");
    let thing = op(&world, "rook", "thing_open", json!({"id": docs.page})).await;
    assert_eq!(thing["outcome"], "ok", "{thing}");
    // The named still search it.
    let found = op(
        &world,
        "lena",
        "workspace_search",
        json!({"query": "Quasar", "projectId": work}),
    )
    .await;
    assert!(found.to_string().contains(&docs.page), "{found}");

    // What sits inside says where its rule comes from.
    let inherited = op(&world, "lena", "doc_access_get", json!({"id": docs.sheet})).await;
    assert_eq!(ok(&inherited)["restricted"], false);
    assert_eq!(ok(&inherited)["inheritedFrom"]["id"], vault);
    assert_eq!(ok(&inherited)["inheritedFrom"]["title"], "Vault");

    // A document restricted on its own inside the folder narrows to its
    // own names: lena loses the sheet, and keeps the rest.
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": docs.sheet, "restricted": true, "people": [rook]}),
    )
    .await);
    assert!(!opens(&world, "lena", &work, "sheet", &docs.sheet).await);
    assert!(!listed(&world, "lena", &work, "sheet", &docs.sheet).await);
    assert!(opens(&world, "rook", &work, "sheet", &docs.sheet).await);
    assert!(opens(&world, "lena", &work, "page", &docs.page).await);
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": docs.sheet, "restricted": false}),
    )
    .await);
    assert!(opens(&world, "lena", &work, "sheet", &docs.sheet).await);

    // Leaving someone out of the list takes them off at once.
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": vault, "restricted": true, "people": [lena]}),
    )
    .await);
    check(&world, "rook", &work, &docs, false).await;
    check(&world, "lena", &work, &docs, true).await;

    // Moved out of the folder, a document is the space's again.
    ok(&op(&world, "lena", "doc_move", json!({"id": docs.page})).await);
    assert!(opens(&world, "mara", &work, "page", &docs.page).await);
    assert!(!opens(&world, "mara", &work, "sheet", &docs.sheet).await);

    // Opened again, everything is back for everyone.
    ok(&op(
        &world,
        "lena",
        "doc_access_set",
        json!({"id": vault, "restricted": false}),
    )
    .await);
    for who in ["mara", "wren", "rook"] {
        check(&world, who, &work, &docs, true).await;
    }
    assert_eq!(link_opens(&world, &link).await, StatusCode::OK);
    assert!(watches(&world, "mara", &docs).await);
}

#[tokio::test]
async fn one_restricted_document_and_the_pages_inside_a_page() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("HOME")
        .person("lena", "member")
        .person("mara", "member")
        .build("awd223_single")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let lena = world.people["lena"].actor;
    let outer = id_at(
        &op(
            &world,
            "owner",
            "page_create",
            json!({"projectId": work, "title": "Offer letters", "body": "Drafts."}),
        )
        .await,
        "/id",
    );
    let inner = id_at(
        &op(
            &world,
            "owner",
            "page_create",
            json!({"projectId": work, "title": "Offer for Sam", "body": "Terms.", "parentPageId": outer}),
        )
        .await,
        "/id",
    );
    let entries = tree(&world, "mara", &work).await;
    assert_eq!(entry(&entries, &inner).expect("inner")["parentId"], outer);

    // Restricting the outer page covers the page inside it.
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": outer, "restricted": true, "people": [lena]}),
    )
    .await);
    for page in [&outer, &inner] {
        assert!(opens(&world, "lena", &work, "page", page).await);
        assert!(!opens(&world, "mara", &work, "page", page).await);
        assert!(!listed(&world, "mara", &work, "page", page).await);
    }
    // A page made inside it afterwards is covered from the start.
    let later = id_at(
        &op(
            &world,
            "lena",
            "page_create",
            json!({"projectId": work, "title": "Offer for Ada", "body": "Terms.", "parentPageId": outer}),
        )
        .await,
        "/id",
    );
    assert!(!opens(&world, "mara", &work, "page", &later).await);
    assert!(opens(&world, "owner", &work, "page", &later).await);

    // A page inside a page moves with it, not on its own.
    let folder = id_at(
        &op(
            &world,
            "owner",
            "doc_folder_create",
            json!({"spaceId": work, "title": "People"}),
        )
        .await,
        "/id",
    );
    let alone = op(
        &world,
        "owner",
        "doc_move",
        json!({"id": inner, "folderId": folder}),
    )
    .await;
    assert_eq!(alone["outcome"], "invalid_request", "{alone}");
    ok(&op(
        &world,
        "owner",
        "doc_move",
        json!({"id": outer, "folderId": folder}),
    )
    .await);
    assert!(opens(&world, "lena", &work, "page", &inner).await);
    assert!(!opens(&world, "mara", &work, "page", &inner).await);

    // Someone who can't see it can't change who does.
    let pried = op(
        &world,
        "mara",
        "doc_access_set",
        json!({"id": outer, "restricted": false}),
    )
    .await;
    assert_eq!(pried["outcome"], "not_found", "{pried}");

    // A page with a page inside that the mover can't see doesn't go to
    // another space in their hands: it would be carried along unseen.
    let home = world.project("HOME").to_owned();
    let plans = id_at(
        &op(
            &world,
            "owner",
            "page_create",
            json!({"projectId": work, "title": "Plans", "body": "Open."}),
        )
        .await,
        "/id",
    );
    let sealed = id_at(
        &op(
            &world,
            "owner",
            "page_create",
            json!({"projectId": work, "title": "Sealed plan", "body": "Closed.", "parentPageId": plans}),
        )
        .await,
        "/id",
    );
    // lena watches the page inside until it is kept from her.
    let watching = |who: &'static str| {
        let document = DocumentRef {
            kind: DocumentKind::Page,
            id: sealed.parse().expect("id"),
        };
        let account = world.people[who].actor;
        let pool = world.pool().clone();
        async move {
            agent_workspace_server::tenancy::system(
                "realtime.connections",
                can_view(&pool, account, document),
            )
            .await
            .expect("can view")
        }
    };
    assert!(watching("lena").await);
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": sealed, "restricted": true, "people": []}),
    )
    .await);
    assert!(!watching("lena").await);
    // Nor does a page she can't see hide a loop: a page inside the sealed
    // one, with her named on it, is still below Plans.
    let below = id_at(
        &op(
            &world,
            "owner",
            "page_create",
            json!({"projectId": work, "title": "Plan notes", "body": "Notes.", "parentPageId": sealed}),
        )
        .await,
        "/id",
    );
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": below, "restricted": true, "people": [lena]}),
    )
    .await);
    assert!(opens(&world, "lena", &work, "page", &below).await);
    let looped = op(
        &world,
        "lena",
        "page_update",
        json!({"pageId": plans, "parentPageId": below}),
    )
    .await;
    assert_eq!(looped["outcome"], "invalid_request", "{looped}");
    assert!(looped.to_string().contains("inside itself"), "{looped}");

    // A folder that looks empty to her, but holds a sheet kept from her,
    // goes; the sheet comes up to the top rather than staying filed under it.
    let drawer = id_at(
        &op(
            &world,
            "owner",
            "doc_folder_create",
            json!({"spaceId": work, "title": "Drawer"}),
        )
        .await,
        "/id",
    );
    let ledger = id_at(
        &op(
            &world,
            "owner",
            "sheet_create",
            json!({"projectId": work, "name": "Ledger"}),
        )
        .await,
        "/id",
    );
    ok(&op(
        &world,
        "owner",
        "doc_move",
        json!({"id": ledger, "folderId": drawer}),
    )
    .await);
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": ledger, "restricted": true, "people": []}),
    )
    .await);
    ok(&op(
        &world,
        "lena",
        "doc_folder_update",
        json!({"folderId": drawer, "remove": true}),
    )
    .await);
    let entries = tree(&world, "owner", &work).await;
    let kept = entry(&entries, &ledger).expect("ledger");
    assert!(kept["parentId"].is_null(), "{kept}");
    assert_eq!(kept["restrictedHere"], true, "{kept}");
    assert!(!opens(&world, "lena", &work, "sheet", &ledger).await);

    // The guide is everyone's: it isn't kept to chosen people.
    let guide = id_at(
        &op(
            &world,
            "owner",
            "page_create",
            json!({"projectId": work, "title": "How we work", "body": "Read me.", "isGuide": true}),
        )
        .await,
        "/id",
    );
    let kept = op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": guide, "restricted": true, "people": []}),
    )
    .await;
    assert_eq!(kept["outcome"], "invalid_request", "{kept}");
    // Nor by way of a folder: not restricted with the guide in it, and the
    // guide doesn't go into one that is.
    let shelf = id_at(
        &op(
            &world,
            "owner",
            "doc_folder_create",
            json!({"spaceId": work, "title": "Shelf"}),
        )
        .await,
        "/id",
    );
    ok(&op(
        &world,
        "owner",
        "doc_move",
        json!({"id": guide, "folderId": shelf}),
    )
    .await);
    let kept = op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": shelf, "restricted": true, "people": []}),
    )
    .await;
    assert_eq!(kept["outcome"], "invalid_request", "{kept}");
    ok(&op(&world, "owner", "doc_move", json!({"id": guide})).await);
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": shelf, "restricted": true, "people": []}),
    )
    .await);
    let kept = op(
        &world,
        "owner",
        "doc_move",
        json!({"id": guide, "folderId": shelf}),
    )
    .await;
    assert_eq!(kept["outcome"], "invalid_request", "{kept}");

    // A folder kept to chosen people isn't removed over something it still
    // covers that the list doesn't show (here, a sheet kept to the owner).
    let safe = id_at(
        &op(
            &world,
            "owner",
            "doc_folder_create",
            json!({"spaceId": work, "title": "Safe"}),
        )
        .await,
        "/id",
    );
    ok(&op(
        &world,
        "owner",
        "doc_move",
        json!({"id": ledger, "folderId": safe}),
    )
    .await);
    ok(&op(
        &world,
        "owner",
        "doc_access_set",
        json!({"id": safe, "restricted": true, "people": [lena]}),
    )
    .await);
    let refused = op(
        &world,
        "lena",
        "doc_folder_update",
        json!({"folderId": safe, "remove": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    let moving = json!({"pageId": plans, "toSpaceId": home, "expectedVersion": 1});
    let refused = op(&world, "lena", "page_move", moving.clone()).await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    ok(&op(&world, "owner", "page_move", moving).await);
}

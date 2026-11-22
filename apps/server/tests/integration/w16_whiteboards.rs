//! W16: whiteboards. The costly failures are silent: a board (or its images)
//! reaching someone outside its project, a concurrent edit quietly dropping
//! someone else's change, an agent writing where it can't see, links that
//! delete or copy the board, and changes that never reach open boards.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{Browser, create_agent, creates_once, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::{add_person, set_role},
};

/// A 1x1 PNG.
const TINY_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

async fn request(
    setup: &Setup,
    browser: &Browser,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    send(&setup.app, browser.request(method, uri, None, body)).await
}

/// A whiteboard operation, as the web app runs it.
async fn op(setup: &Setup, browser: &Browser, name: &str, input: Value) -> (StatusCode, Value) {
    browser.op(&setup.app, name, input).await
}

async fn create_board(setup: &Setup, browser: &Browser, body: Value) -> String {
    let (status, created) = op(setup, browser, "whiteboard_create", body).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    created["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned()
}

/// An Excalidraw rectangle as a browser sends it.
fn rectangle(id: &str, version: i64, nonce: i64, x: f64) -> Value {
    json!({
        "id": id, "type": "rectangle", "x": x, "y": 0, "width": 100, "height": 60,
        "angle": 0, "strokeColor": "#1e1e1e", "backgroundColor": "transparent",
        "fillStyle": "solid", "strokeWidth": 2, "strokeStyle": "solid", "roughness": 1,
        "opacity": 100, "groupIds": [], "frameId": null, "index": "a0", "roundness": null,
        "seed": 1, "version": version, "versionNonce": nonce, "isDeleted": false,
        "boundElements": null, "updated": 1, "link": null, "locked": false,
    })
}

async fn write(setup: &Setup, browser: &Browser, board: &str, elements: Vec<Value>) -> Value {
    let (status, written) = request(
        setup,
        browser,
        "POST",
        &format!("/api/v1/whiteboards/{board}/scene"),
        json!({"elements": elements}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{written}");
    written
}

async fn scene(setup: &Setup, browser: &Browser, board: &str, since: Option<i64>) -> Value {
    let uri = match since {
        Some(since) => format!("/api/v1/whiteboards/{board}/scene?since={since}"),
        None => format!("/api/v1/whiteboards/{board}/scene"),
    };
    let (status, scene) = request(setup, browser, "GET", &uri, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{scene}");
    scene
}

fn element<'a>(scene: &'a Value, id: &str) -> Option<&'a Value> {
    scene["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .find(|element| element["id"] == id)
}

async fn events(setup: &Setup, board: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        &format!(
"SELECT count(*) FROM {n} outbox WHERE event_type = 'whiteboard.updated' AND payload->>'whiteboardId' = $1",
 n = crate::support::NOTICES
),
    )
    .bind(board)
    .fetch_one(&setup.pool)
    .await
    .expect("events")
}

#[tokio::test]
async fn boards_live_in_a_workspace_or_project_and_links_never_copy_or_delete_them() {
    let Some(setup) = setup("w16_boards").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTH", "Other").await;
    let (_mara, mara) = add_person(&setup, &[&other], "mara").await;
    let (viewer_id, viewer) = add_person(&setup, &[&work], "vic").await;
    set_role(&setup.pool, &work, viewer_id, "viewer").await;
    let owner = &setup.browser;

    // A board for the whole workspace, with no task or project.
    let open = create_board(
        &setup,
        owner,
        json!({"title": "  Whole   team ", "workspaceId": setup.organization_id}),
    )
    .await;
    // A project board, linked to a task as it's made.
    let (status, task) = crate::support::tasks::send(
        &setup.app,
        owner,
        "POST",
        &format!("/api/v1/projects/{work}/work-items"),
        Some("task-sync"),
        json!({"title": "Design the sync"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let private = create_board(
        &setup,
        owner,
        json!({"title": "Sync design", "projectId": work,
               "linkTo": {"targetKind": "task", "targetId": task}}),
    )
    .await;

    let (_, listed) = request(&setup, owner, "GET", "/api/v1/whiteboards", Value::Null).await;
    let titles = listed["whiteboards"]
        .as_array()
        .expect("boards")
        .iter()
        .map(|board| board["title"].as_str().expect("title").to_owned())
        .collect::<Vec<_>>();
    assert!(titles.contains(&"Whole team".to_owned()), "{listed}");
    assert!(titles.contains(&"Sync design".to_owned()), "{listed}");

    // Mara is in the workspace but not the project: the workspace board is
    // hers to draw on, the project board doesn't exist for her, links
    // included, and she can't link her board to the project's task.
    let (status, _) = request(
        &setup,
        &mara,
        "GET",
        &format!("/api/v1/whiteboards/{open}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for path in ["", "/scene", "/preview.svg"] {
        let (status, body) = request(
            &setup,
            &mara,
            "GET",
            &format!("/api/v1/whiteboards/{private}{path}"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    let (_, hers) = request(
        &setup,
        &mara,
        "GET",
        "/api/v1/whiteboards?q=sync",
        Value::Null,
    )
    .await;
    assert_eq!(hers["whiteboards"], json!([]), "{hers}");
    let (status, _) = request(
        &setup,
        &mara,
        "GET",
        &format!("/api/v1/whiteboard-links?targetKind=task&targetId={task}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = op(
        &setup,
        &mara,
        "whiteboard_link",
        json!({"whiteboardId": open, "targetKind": "task", "targetId": task}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    write(&setup, &mara, &open, vec![rectangle("hers", 1, 7, 0.0)]).await;

    // A viewer sees the project board but can't change it.
    let (status, detail) = request(
        &setup,
        &viewer,
        "GET",
        &format!("/api/v1/whiteboards/{private}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["canEdit"], false);
    let (status, _) = request(
        &setup,
        &viewer,
        "POST",
        &format!("/api/v1/whiteboards/{private}/scene"),
        json!({"elements": [rectangle("nope", 1, 1, 0.0)]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // One board on a task, its project and a page, without copies.
    let (status, page) = request(
        &setup,
        owner,
        "POST",
        &format!("/api/v1/projects/{work}/pages"),
        json!({"title": "Notes", "body": "Sync notes"}),
    )
    .await;
    assert!(status.is_success(), "{page}");
    let page = page["id"].as_str().expect("page").to_owned();
    for (kind, target) in [
        ("project", work.as_str()),
        ("page", page.as_str()),
        ("task", task.as_str()),
    ] {
        let (status, linked) = op(
            &setup,
            owner,
            "whiteboard_link",
            json!({"whiteboardId": private, "targetKind": kind, "targetId": target}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{linked}");
    }
    let (_, detail) = request(
        &setup,
        owner,
        "GET",
        &format!("/api/v1/whiteboards/{private}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        detail["links"].as_array().expect("links").len(),
        3,
        "{detail}"
    );
    for (kind, target) in [("project", &work), ("page", &page), ("task", &task)] {
        let (status, on) = request(
            &setup,
            owner,
            "GET",
            &format!("/api/v1/whiteboard-links?targetKind={kind}&targetId={target}"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{on}");
        assert_eq!(on["whiteboards"][0]["id"], private.as_str(), "{kind}: {on}");
    }
    let boards = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM whiteboards")
        .fetch_one(&setup.pool)
        .await
        .expect("count");
    assert_eq!(boards, 2);

    // Drawing and an image survive detaching, the trash and a reload.
    write(&setup, owner, &private, vec![rectangle("box", 1, 5, 10.0)]).await;
    let (status, stored) = request(
        &setup,
        owner,
        "POST",
        &format!("/api/v1/whiteboards/{private}/files"),
        json!({"id": "photo1", "mimeType": "image/png", "dataURL": TINY_PNG}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let (status, _) = request(
        &setup,
        owner,
        "POST",
        &format!("/api/v1/whiteboards/{private}/files"),
        json!({"id": "fake", "mimeType": "image/png", "dataURL": "data:image/png;base64,PHN2Zz48L3N2Zz4="}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bytes that aren't a PNG");
    let (status, _) = op(
        &setup,
        owner,
        "whiteboard_link",
        json!({"whiteboardId": private, "targetKind": "task", "targetId": task, "remove": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, trashed) = op(
        &setup,
        owner,
        "whiteboard_update",
        json!({"whiteboardId": private, "deleted": true, "title": "Sync design v2"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{trashed}");
    assert_eq!(trashed["whiteboard"]["deleted"], true);
    let (_, trash) = request(
        &setup,
        owner,
        "GET",
        "/api/v1/whiteboards?deleted=true",
        Value::Null,
    )
    .await;
    assert_eq!(trash["whiteboards"][0]["id"], private.as_str());
    let (status, _) = op(
        &setup,
        owner,
        "whiteboard_update",
        json!({"whiteboardId": private, "deleted": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let reloaded = scene(&setup, owner, &private, None).await;
    assert!(element(&reloaded, "box").is_some(), "{reloaded}");
    assert_eq!(reloaded["files"][0]["id"], "photo1");
    let (status, file) = request(
        &setup,
        owner,
        "GET",
        &format!("/api/v1/whiteboards/{private}/files/photo1"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(file["dataURL"], TINY_PNG);
    let (status, _) = request(
        &setup,
        &mara,
        "GET",
        &format!("/api/v1/whiteboards/{private}/files/photo1"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "images follow the board");

    // A copy takes the drawing and images, not the links.
    let (status, copy) = op(
        &setup,
        owner,
        "whiteboard_duplicate",
        json!({"whiteboardId": private}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    assert_eq!(copy["whiteboard"]["title"], "Copy of Sync design v2");
    let copy = copy["whiteboard"]["id"].as_str().expect("copy").to_owned();
    let (_, copied) = request(
        &setup,
        owner,
        "GET",
        &format!("/api/v1/whiteboards/{copy}"),
        Value::Null,
    )
    .await;
    assert_eq!(copied["links"], json!([]), "{copied}");
    let copied = scene(&setup, owner, &copy, None).await;
    assert!(element(&copied, "box").is_some(), "{copied}");
    assert_eq!(copied["files"][0]["id"], "photo1");

    // Moving the board to the whole workspace lets Mara in.
    let (status, moved) = op(
        &setup,
        owner,
        "whiteboard_update",
        json!({"whiteboardId": private, "workspaceWide": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let (status, _) = request(
        &setup,
        &mara,
        "GET",
        &format!("/api/v1/whiteboards/{private}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // But the links into the project stay hidden from her.
    let (_, hers) = request(
        &setup,
        &mara,
        "GET",
        &format!("/api/v1/whiteboards/{private}"),
        Value::Null,
    )
    .await;
    assert_eq!(hers["links"], json!([]), "{hers}");
}

#[tokio::test]
async fn trashed_boards_go_for_good_only_when_someone_who_can_edit_asks() {
    let Some(setup) = setup("w16_purge").await else {
        return;
    };
    let work = create_project(&setup, "PURGE", "Purge").await;
    let (viewer_id, viewer) = add_person(&setup, &[&work], "vic").await;
    set_role(&setup.pool, &work, viewer_id, "viewer").await;
    let owner = &setup.browser;
    let board = create_board(
        &setup,
        owner,
        json!({"title": "Old plan", "projectId": work}),
    )
    .await;
    let kept = create_board(&setup, owner, json!({"title": "Keep", "projectId": work})).await;
    write(&setup, owner, &board, vec![rectangle("box", 1, 1, 0.0)]).await;
    let path = format!("/api/v1/whiteboards/{board}");

    // Only from the trash.
    let (status, _) = request(&setup, owner, "DELETE", &path, Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, copy) = op(
        &setup,
        owner,
        "whiteboard_duplicate",
        json!({"whiteboardId": board}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    let copy = copy["whiteboard"]["id"].as_str().expect("copy").to_owned();
    let (status, _) = op(
        &setup,
        owner,
        "whiteboard_update",
        json!({"whiteboardId": board, "deleted": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A viewer sees the project's trash but can't delete from it.
    let (status, _) = request(&setup, &viewer, "DELETE", &path, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, emptied) = request(
        &setup,
        &viewer,
        "POST",
        "/api/v1/whiteboard-trash/empty",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{emptied}");
    assert_eq!(emptied["deleted"], 0);

    // The owner deletes it for good, drawing and all; the copy stays.
    let (status, _) = request(&setup, owner, "DELETE", &path, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let left = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM whiteboard_elements WHERE whiteboard_id::text = $1",
    )
    .bind(&board)
    .fetch_one(&setup.pool)
    .await
    .expect("elements");
    assert_eq!(left, 0);
    let (status, _) = request(&setup, owner, "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = request(
        &setup,
        owner,
        "GET",
        &format!("/api/v1/whiteboards/{copy}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a copy outlives the board it came from"
    );

    // Emptying the trash takes what's left there, and nothing outside it.
    let (status, _) = op(
        &setup,
        owner,
        "whiteboard_update",
        json!({"whiteboardId": kept, "deleted": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, emptied) = request(
        &setup,
        owner,
        "POST",
        "/api/v1/whiteboard-trash/empty",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{emptied}");
    assert_eq!(emptied["deleted"], 1);
    let (status, _) = request(
        &setup,
        owner,
        "GET",
        &format!("/api/v1/whiteboards/{copy}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Boards someone can only look at never fill their batch: with more of
    // them in the trash than one Empty trash takes, all older than a board
    // of their own, their own board still goes.
    let theirs = create_project(&setup, "THEIRS", "Theirs").await;
    add_member(&setup, &theirs, viewer_id, "member").await;
    let own = create_board(
        &setup,
        &viewer,
        json!({"title": "Mine", "projectId": theirs}),
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO whiteboards (
            id, organization_id, project_id, title, created_by_actor_id,
            deleted_at, deleted_by_actor_id
        )
        SELECT gen_random_uuid(), board.organization_id, board.project_id, 'Old ' || n,
               board.created_by_actor_id, now() - interval '1 day' + n * interval '1 second',
               board.created_by_actor_id
        FROM whiteboards board, generate_series(1, 201) AS n
        WHERE board.id::text = $1
        "#,
    )
    .bind(&copy)
    .execute(&setup.pool)
    .await
    .expect("older trashed boards");
    let (status, _) = op(
        &setup,
        &viewer,
        "whiteboard_update",
        json!({"whiteboardId": own, "deleted": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, emptied) = request(
        &setup,
        &viewer,
        "POST",
        "/api/v1/whiteboard-trash/empty",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{emptied}");
    assert_eq!(emptied["deleted"], 1, "{emptied}");
    let (status, _) = request(
        &setup,
        &viewer,
        "GET",
        &format!("/api/v1/whiteboards/{own}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn concurrent_edits_merge_by_element_and_never_drop_a_newer_change() {
    let Some(setup) = setup("w16_merge").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let (_, mara) = add_person(&setup, &[&work], "mara").await;
    let owner = &setup.browser;
    let board = create_board(&setup, owner, json!({"title": "Shared", "projectId": work})).await;
    let first = write(
        &setup,
        owner,
        &board,
        vec![rectangle("a", 1, 10, 0.0), rectangle("b", 1, 20, 200.0)],
    )
    .await;
    let base = first["sceneSeq"].as_i64().expect("seq");
    let before = events(&setup, &board).await;

    // Both start from the same scene and change different elements.
    let mine = write(&setup, owner, &board, vec![rectangle("a", 2, 11, 50.0)]).await;
    let theirs = write(&setup, &mara, &board, vec![rectangle("b", 2, 21, 250.0)]).await;
    assert_eq!(mine["accepted"], json!(["a"]));
    assert_eq!(theirs["accepted"], json!(["b"]));
    let merged = scene(&setup, owner, &board, None).await;
    assert_eq!(element(&merged, "a").expect("a")["x"], 50.0);
    assert_eq!(element(&merged, "b").expect("b")["x"], 250.0);

    // Mara, offline, edits "a" from version 1: the newer copy stays and
    // comes back to her as rejected.
    let stale = write(&setup, &mara, &board, vec![rectangle("a", 2, 99, 999.0)]).await;
    assert_eq!(stale["accepted"], json!([]), "{stale}");
    assert_eq!(stale["rejected"][0]["x"], 50.0, "{stale}");
    // On the same version the lower nonce wins, as in Excalidraw.
    let tie = write(&setup, &mara, &board, vec![rectangle("a", 2, 3, 70.0)]).await;
    assert_eq!(tie["accepted"], json!(["a"]), "{tie}");
    // Resending what's stored (an echo) changes nothing.
    let echo = write(&setup, owner, &board, vec![rectangle("a", 2, 3, 70.0)]).await;
    assert_eq!(echo["accepted"], json!([]));
    assert_eq!(echo["sceneSeq"], tie["sceneSeq"]);

    // A reconnecting client asks for what changed since it last looked,
    // deletions included.
    let mut gone = rectangle("b", 3, 22, 250.0);
    gone["isDeleted"] = json!(true);
    write(&setup, owner, &board, vec![gone]).await;
    let changed = scene(&setup, &mara, &board, Some(base)).await;
    let ids = changed["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .map(|element| element["id"].as_str().expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2, "{changed}");
    assert_eq!(element(&changed, "b").expect("b")["isDeleted"], true);
    let live = scene(&setup, &mara, &board, None).await;
    assert!(element(&live, "b").is_none());
    assert!(
        events(&setup, &board).await > before,
        "changes reach open boards"
    );

    // A bad element is refused whole, with the reason.
    let (status, refused) = request(
        &setup,
        owner,
        "POST",
        &format!("/api/v1/whiteboards/{board}/scene"),
        json!({"elements": [{"id": "x", "type": "iframe", "version": 1, "versionNonce": 1}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
}

#[tokio::test]
async fn agents_draw_read_render_and_restore_boards_through_mcp_alone() {
    let Some(setup) = setup("w16_agents").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let secret = create_project(&setup, "SEC", "Secret").await;
    let space = space(&setup).await;
    let (rook, token) = create_agent(app, &setup.browser, &space, "Rook", "rook").await;
    add_member(&setup, &work, rook, "member").await;
    let session = initialize(app, &token).await.expect("agent MCP");
    let (status, task) = crate::support::tasks::send(
        app,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{work}/work-items"),
        Some("task-launch"),
        json!({"title": "Plan the launch"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let hidden = create_board(
        &setup,
        &setup.browser,
        json!({"title": "Secret plans", "projectId": secret}),
    )
    .await;

    // Create, link and draw in one call.
    let created = tool(
        app,
        &token,
        &session,
        "whiteboard_create",
        json!({
            "title": "Launch flow",
            "projectId": work,
            "linkTo": {"targetKind": "task", "targetId": task},
            "operations": [
                {"op": "add", "type": "rectangle", "id": "plan", "text": "Plan"},
                {"op": "add", "type": "rectangle", "id": "build", "text": "Build"},
                {"op": "add", "type": "ellipse", "id": "ship", "text": "Ship"},
                {"op": "connect", "from": "plan", "to": "build"},
                {"op": "connect", "id": "last", "from": "build", "to": "ship", "text": "when green"},
                {"op": "layout", "ids": ["plan", "build", "ship"], "direction": "right", "gap": 80},
            ],
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let board = created["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    assert_eq!(created["value"]["whiteboard"]["linkCount"], 1);

    let read = tool(
        app,
        &token,
        &session,
        "whiteboard_read",
        json!({"whiteboardId": board}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let elements = read["value"]["elements"].as_array().expect("elements");
    let by_id = |id: &str| {
        elements
            .iter()
            .find(|element| element["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {read}"))
    };
    assert_eq!(by_id("ship")["text"], "Ship");
    assert_eq!(by_id("last")["from"], "build");
    assert_eq!(by_id("last")["to"], "ship");
    assert_eq!(by_id("last")["text"], "when green");
    let plan_x = by_id("plan")["x"].as_f64().expect("x");
    let build_x = by_id("build")["x"].as_f64().expect("x");
    assert!(
        build_x > plan_x + 100.0,
        "laid out to the right: {plan_x} {build_x}"
    );
    assert_eq!(read["value"]["links"][0]["targetKind"], "work_item");
    let seq = read["value"]["sceneSeq"].as_i64().expect("seq");

    // A person moves "plan" while the agent works from its old read: the
    // agent's edit of "plan" is refused, an edit of something else isn't.
    let raw = tool(
        app,
        &token,
        &session,
        "whiteboard_read",
        json!({"whiteboardId": board, "elementIds": ["plan"], "raw": true}),
    )
    .await;
    let mut plan = raw["value"]["raw"][0].clone();
    plan["x"] = json!(plan["x"].as_f64().expect("x") - 500.0);
    plan["version"] = json!(plan["version"].as_i64().expect("version") + 1);
    write(&setup, &setup.browser, &board, vec![plan]).await;
    let conflict = tool(
        app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": board, "expectedSceneSeq": seq,
               "operations": [{"op": "update", "id": "plan", "text": "Plan it"}]}),
    )
    .await;
    assert_eq!(conflict["outcome"], "conflict", "{conflict}");
    let edited = tool(
        app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": board, "expectedSceneSeq": seq,
               "operations": [{"op": "update", "id": "ship", "text": "Ship it", "backgroundColor": "#b2f2bb"}]}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let after_edit = edited["value"]["sceneSeq"].as_i64().expect("seq");
    assert!(after_edit > seq);
    let audited = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE action = 'whiteboard_update_elements' AND actor_id = $1",
    )
    .bind(rook)
    .fetch_one(&setup.pool)
    .await
    .expect("audit");
    assert_eq!(audited, 1);
    assert!(
        events(&setup, &board).await >= 2,
        "agent changes reach open boards"
    );

    // A person with the board open sees the agent's change.
    let seen = scene(&setup, &setup.browser, &board, Some(seq)).await;
    let ship_label = seen["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .find(|element| element["containerId"] == "ship")
        .expect("label");
    assert_eq!(ship_label["originalText"], "Ship it");

    // Removing many elements needs confirmDelete, and saves a revision first.
    let many = (0..30)
        .map(|n| json!({"op": "add", "type": "rectangle", "id": format!("n{n}"), "x": n * 20, "y": 400}))
        .collect::<Vec<_>>();
    let added = tool(
        app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": board, "operations": many}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    let ids = (0..30).map(|n| format!("n{n}")).collect::<Vec<_>>();
    let refused = tool(
        app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": board, "operations": [{"op": "delete", "ids": ids}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    let removed = tool(
        app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": board, "confirmDelete": true, "operations": [{"op": "delete", "ids": ids}]}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    let revision = removed["value"]["savedRevision"]
        .as_i64()
        .expect("revision");
    let restored = tool(
        app,
        &token,
        &session,
        "whiteboard_update",
        json!({"whiteboardId": board, "restoreRevision": revision}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    let back = scene(&setup, &setup.browser, &board, None).await;
    assert!(element(&back, "n29").is_some(), "restored");

    // A picture to check the drawing by.
    let rendered = tool(
        app,
        &token,
        &session,
        "whiteboard_render",
        json!({"whiteboardId": board, "elementIds": ["plan", "build"]}),
    )
    .await;
    assert_eq!(rendered["outcome"], "ok", "{rendered}");
    assert!(rendered["value"]["width"].as_f64().expect("width") > 10.0);
    let svg = tool(
        app,
        &token,
        &session,
        "whiteboard_render",
        json!({"whiteboardId": board, "format": "svg"}),
    )
    .await;
    assert!(
        svg["value"]["svg"]
            .as_str()
            .expect("svg")
            .contains("Ship it"),
        "{svg}"
    );

    // Links through task_link too, and boards on a task are listed.
    let other = tool(
        app,
        &token,
        &session,
        "whiteboard_create",
        json!({"title": "Scratch"}),
    )
    .await;
    assert_eq!(other["outcome"], "ok", "{other}");
    assert_eq!(
        other["value"]["whiteboard"]["projectId"],
        Value::Null,
        "a workspace board by default"
    );
    let scratch = other["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let linked = tool(
        app,
        &token,
        &session,
        "task_link",
        json!({"taskId": task, "whiteboardId": scratch}),
    )
    .await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    assert_eq!(linked["value"]["kind"], "whiteboard");
    let on_task = tool(
        app,
        &token,
        &session,
        "whiteboard_list",
        json!({"linkedToKind": "task", "linkedToId": task}),
    )
    .await;
    assert_eq!(
        on_task["value"]["whiteboards"]
            .as_array()
            .expect("boards")
            .len(),
        2,
        "{on_task}"
    );

    // The secret project's board doesn't exist for the agent.
    for name in ["whiteboard_read", "whiteboard_render"] {
        let hidden_read = tool(app, &token, &session, name, json!({"whiteboardId": hidden})).await;
        assert_eq!(hidden_read["outcome"], "not_found", "{name}: {hidden_read}");
    }
    let hidden_edit = tool(
        app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": hidden, "operations": [{"op": "add", "type": "text", "text": "hi"}]}),
    )
    .await;
    assert_eq!(hidden_edit["outcome"], "not_found", "{hidden_edit}");
    let into_secret = tool(
        app,
        &token,
        &session,
        "whiteboard_create",
        json!({"title": "Sneaky", "projectId": secret}),
    )
    .await;
    assert_ne!(into_secret["outcome"], "ok", "{into_secret}");
    let listed = tool(app, &token, &session, "whiteboard_list", json!({})).await;
    assert!(!listed.to_string().contains("Secret plans"), "{listed}");
    let unchanged = scene(&setup, &setup.browser, &hidden, None).await;
    assert_eq!(unchanged["elements"], json!([]));
}

#[tokio::test]
async fn creating_or_copying_a_board_again_makes_one() {
    let Some(setup) = setup("w16_retry").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = space(&setup).await;
    let (rook, token) = create_agent(app, &setup.browser, &space, "Rook", "rook").await;
    add_member(&setup, &work, rook, "member").await;
    let session = initialize(app, &token).await.expect("agent MCP");
    let boards = "SELECT count(*) FROM whiteboards";
    let created = creates_once(
        app,
        &setup.pool,
        &token,
        &session,
        "whiteboard_create",
        json!({
            "title": "Launch flow",
            "projectId": work,
            "operations": [{"op": "add", "type": "rectangle", "id": "plan", "text": "Plan"}],
            "clientRequestId": "board-retry-1",
        }),
        json!({"title": "Other flow", "projectId": work, "clientRequestId": "board-retry-1"}),
        boards,
    )
    .await;
    let board = created["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board");
    let copied = creates_once(
        app,
        &setup.pool,
        &token,
        &session,
        "whiteboard_duplicate",
        json!({"whiteboardId": board, "clientRequestId": "copy-retry-1"}),
        json!({"whiteboardId": board, "title": "Mine", "clientRequestId": "copy-retry-1"}),
        boards,
    )
    .await;
    let copy = copied["value"]["whiteboard"]["id"].as_str().expect("copy");

    // Both boards move to a project Rook isn't in, renamed: a retry now
    // finds nothing and names neither board, project nor editors.
    let secret = create_project(&setup, "SEC", "Secret project").await;
    for id in [board, copy] {
        sqlx::query(
            "UPDATE whiteboards SET project_id = $2::uuid, title = 'Hidden plans' WHERE id = $1::uuid",
        )
        .bind(id)
        .bind(&secret)
        .execute(&setup.pool)
        .await
        .expect("move the board");
    }
    for (name, arguments) in [
        (
            "whiteboard_create",
            json!({
                "title": "Launch flow",
                "projectId": work,
                "operations": [{"op": "add", "type": "rectangle", "id": "plan", "text": "Plan"}],
                "clientRequestId": "board-retry-1",
            }),
        ),
        (
            "whiteboard_duplicate",
            json!({"whiteboardId": board, "clientRequestId": "copy-retry-1"}),
        ),
    ] {
        let replay = tool(app, &token, &session, name, arguments).await;
        assert_eq!(replay["outcome"], "not_found", "{name}: {replay}");
        let text = replay.to_string();
        assert!(
            !text.contains("Hidden plans") && !text.contains("Secret project"),
            "{name} replay leaks the moved board: {replay}"
        );
    }
}

#[tokio::test]
async fn agents_put_images_on_boards_they_can_edit() {
    let Some(setup) = setup("w16_images").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = space(&setup).await;
    let (rook, token) = create_agent(app, &setup.browser, &space, "Rook", "rook").await;
    add_member(&setup, &work, rook, "member").await;
    let session = initialize(app, &token).await.expect("agent MCP");
    let png = TINY_PNG
        .strip_prefix("data:image/png;base64,")
        .expect("png");
    let other = create_board(
        &setup,
        &setup.browser,
        json!({"title": "Other", "projectId": work}),
    )
    .await;

    // An image in the call that creates the board, stored once by content.
    let created = tool(
        app,
        &token,
        &session,
        "whiteboard_create",
        json!({"title": "Screens", "projectId": work, "operations": [
            {"op": "add", "type": "image", "id": "shot", "dataBase64": png, "x": 10, "y": 20, "width": 64, "height": 64},
        ]}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let board = created["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let read = tool(
        app,
        &token,
        &session,
        "whiteboard_read",
        json!({"whiteboardId": board}),
    )
    .await;
    let shot = read["value"]["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .find(|element| element["id"] == "shot")
        .unwrap_or_else(|| panic!("{read}"))
        .clone();
    assert_eq!(shot["type"], "image");
    assert_eq!(shot["width"], 64.0);
    let file_id = shot["fileId"].as_str().expect("fileId").to_owned();

    // People with the board open load it like an image they dropped there.
    let (status, file) = request(
        &setup,
        &setup.browser,
        "GET",
        &format!("/api/v1/whiteboards/{board}/files/{file_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["dataURL"], TINY_PNG);

    // Again by fileId, and the same bytes again: still one stored file.
    let again = tool(
        app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": board, "operations": [
            {"op": "add", "type": "image", "id": "copy", "fileId": file_id, "x": 200},
            {"op": "add", "type": "image", "id": "twice", "dataBase64": png, "mimeType": "image/png", "x": 400},
        ]}),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM files WHERE thing_kind = 'whiteboard' AND thing_id = $1::text::uuid",
    )
    .bind(&board)
    .fetch_one(&setup.pool)
    .await
    .expect("files");
    assert_eq!(stored, 1);

    // A file from another board, bytes that aren't the image they claim,
    // and image fields on a shape are all refused.
    for refused in [
        json!({"whiteboardId": other, "operations": [{"op": "add", "type": "image", "fileId": file_id}]}),
        json!({"whiteboardId": board, "operations": [{"op": "add", "type": "image", "dataBase64": png, "mimeType": "image/jpeg"}]}),
        json!({"whiteboardId": board, "operations": [{"op": "add", "type": "image", "dataBase64": "aGVsbG8="}]}),
        json!({"whiteboardId": board, "operations": [{"op": "add", "type": "rectangle", "fileId": file_id}]}),
        json!({"whiteboardId": board, "operations": [{"op": "add", "type": "image"}]}),
    ] {
        let result = tool(app, &token, &session, "whiteboard_edit", refused.clone()).await;
        assert_eq!(result["outcome"], "invalid_request", "{refused}: {result}");
    }
}

/// K4.10: the workspace's own space is a plain space. A board across the
/// workspace lives there and is shown as the whole workspace's; its events
/// ride on that space and reach every workspace member, people in no project
/// included, and nobody else; a project board moved to the whole workspace
/// lands there; and a channel there works in a workspace with no project.
#[tokio::test]
async fn workspace_boards_and_channels_live_in_the_workspace_space() {
    let Some(setup) = setup("k4_10_workspace_space").await else {
        return;
    };
    let owner = &setup.browser;
    let owner_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT actor_id FROM users WHERE login_name = 'owner'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    // A board across the workspace, before any project exists.
    let (status, workspace) = op(
        &setup,
        owner,
        "whiteboard_create",
        json!({"title": "Whole team", "workspaceId": setup.organization_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{workspace}");
    let open = workspace["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    assert_eq!(
        workspace["whiteboard"]["projectId"],
        Value::Null,
        "{workspace}"
    );
    let space = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM spaces WHERE workspace_id = $1 AND kind = 'workspace'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("the workspace space");
    let stored = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT project_id FROM whiteboards WHERE id = $1::uuid",
    )
    .bind(&open)
    .fetch_one(&setup.pool)
    .await
    .expect("stored board");
    assert_eq!(stored, space, "the board lives in the workspace space");
    // Mara is in the workspace and in a project of her own, not one the
    // board or the channel is in.
    let elsewhere_project = create_project(&setup, "OTH", "Other").await;
    let (mara_id, mara) = add_person(&setup, &[&elsewhere_project], "mara").await;
    // A channel there works end to end (ruling R22).
    let (status, channel) = send(
        &setup.app,
        owner.request(
            "POST",
            &format!("/api/v1/projects/{space}/channels"),
            Some("workspace-channel"),
            json!({"name": "everyone"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let (status, posted) = send(
        &setup.app,
        mara.request(
            "POST",
            &format!("/api/v1/channels/{channel}/messages"),
            Some("mara-hello"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "hello all"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{posted}");
    let (status, timeline) = request(
        &setup,
        owner,
        "GET",
        &format!("/api/v1/conversations/{channel}/timeline"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    assert!(timeline.to_string().contains("hello all"), "{timeline}");

    // Mara draws on it; the change rides on the workspace space.
    write(&setup, &mara, &open, vec![rectangle("hers", 1, 7, 0.0)]).await;
    let carriers = sqlx::query_scalar::<_, uuid::Uuid>(&format!(
        "SELECT DISTINCT project_id FROM {} notice \
         WHERE event_type = 'whiteboard.updated' AND payload->>'whiteboardId' = $1",
        crate::support::NOTICES
    ))
    .bind(&open)
    .fetch_all(&setup.pool)
    .await
    .expect("events");
    assert_eq!(carriers, vec![space]);

    // Realtime delivers board events by whether the board is visible.
    let stranger = uuid::Uuid::now_v7();
    let elsewhere = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(elsewhere)
        .bind(format!(
            "elsewhere-{}",
            &elsewhere.simple().to_string()[20..]
        ))
        .execute(&setup.pool)
        .await
        .expect("another workspace");
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(stranger)
        .execute(&setup.pool)
        .await
        .expect("stranger");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(elsewhere)
    .bind(stranger)
    .execute(&setup.pool)
    .await
    .expect("stranger's membership");
    let board = open.parse::<uuid::Uuid>().expect("board id");
    let sees = |actor: uuid::Uuid| {
        let pool = setup.pool.clone();
        async move {
            crate::support::as_member(
                actor,
                agent_workspace_server::whiteboards::visible_to(&pool, board, actor),
            )
            .await
            .expect("visible_to")
        }
    };
    assert!(sees(owner_id).await);
    assert!(sees(mara_id).await);
    assert!(!sees(stranger).await);
    sqlx::query("UPDATE organization_memberships SET enabled = false WHERE actor_id = $1")
        .bind(mara_id)
        .execute(&setup.pool)
        .await
        .expect("mara leaves");
    assert!(
        !sees(mara_id).await,
        "leaving the workspace stops the events"
    );

    // A project board moved to the whole workspace lands in its space.
    let work = create_project(&setup, "WORK", "Work").await;
    let private = create_board(
        &setup,
        owner,
        json!({"title": "Sync design", "projectId": work}),
    )
    .await;
    let (status, moved) = op(
        &setup,
        owner,
        "whiteboard_update",
        json!({"whiteboardId": private, "workspaceWide": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let stored = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT project_id FROM whiteboards WHERE id = $1::uuid",
    )
    .bind(&private)
    .fetch_one(&setup.pool)
    .await
    .expect("moved board");
    assert_eq!(stored, space);
    let (_, detail) = request(
        &setup,
        owner,
        "GET",
        &format!("/api/v1/whiteboards/{private}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail["whiteboard"]["projectId"], Value::Null, "{detail}");
}

/// C1.32: five notes in two frames become five tasks labelled by frame,
/// each linking back to the board, and the board becomes a page with a
/// heading per frame and its notes as bullets. Someone who can't see the
/// board makes neither.
#[tokio::test]
async fn board_notes_become_tasks_and_a_page() {
    let Some(setup) = setup("c132_board_notes").await else {
        return;
    };
    let space = space(&setup).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTH", "Other").await;
    let owner = &setup.browser;
    let (wren, token) = create_agent(&setup.app, owner, &space, "Wren", "wren").await;
    add_member(&setup, &work, wren, "member").await;
    let (kite, kite_token) = create_agent(&setup.app, owner, &space, "Kite", "kite").await;
    add_member(&setup, &other, kite, "member").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let kite_session = initialize(&setup.app, &kite_token)
        .await
        .expect("MCP session");
    let board = tool(
        &setup.app,
        &token,
        &session,
        "whiteboard_create",
        json!({"projectId": work, "title": "Retro"}),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    let board = board["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let note = |id: &str, frame: &str, y: f64, text: &str| {
        json!({"op": "add", "type": "rectangle", "id": id, "x": 20, "y": y, "frameId": frame,
               "text": text, "backgroundColor": "#ffec99"})
    };
    let drawn = tool(
        &setup.app,
        &token,
        &session,
        "whiteboard_edit",
        json!({"whiteboardId": board, "operations": [
            {"op": "add", "type": "frame", "id": "went", "x": 0, "y": 0, "name": "Went well"},
            {"op": "add", "type": "frame", "id": "fix", "x": 1000, "y": 0, "name": "To fix"},
            note("n1", "went", 40.0, "Fast reviews"),
            note("n2", "went", 160.0, "Clear plan"),
            note("n3", "fix", 40.0, "Flaky tests\nThe file tests time out"),
            note("n4", "fix", 160.0, "Slow deploys"),
            note("n5", "fix", 280.0, "Missing docs"),
            {"op": "connect", "id": "arrow", "from": "n1", "to": "n3", "text": "then"},
        ]}),
    )
    .await;
    assert_eq!(drawn["outcome"], "ok", "{drawn}");

    // Someone outside the board's project makes no tasks of it.
    let refused = tool(
        &setup.app,
        &kite_token,
        &kite_session,
        "whiteboard_to_tasks",
        json!({"whiteboardId": board, "clientRequestId": "kite-retro"}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    let refused = tool(
        &setup.app,
        &kite_token,
        &kite_session,
        "whiteboard_to_tasks",
        json!({"whiteboardId": board, "spaceId": other, "clientRequestId": "kite-retro-2"}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");

    let made = tool(
        &setup.app,
        &token,
        &session,
        "whiteboard_to_tasks",
        json!({"whiteboardId": board, "clientRequestId": "retro-tasks"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let tasks = made["value"]["tasks"].as_array().expect("tasks");
    let titles: Vec<&str> = tasks
        .iter()
        .map(|task| task["title"].as_str().expect("title"))
        .collect();
    assert_eq!(
        titles,
        [
            "Fast reviews",
            "Clear plan",
            "Flaky tests",
            "Slow deploys",
            "Missing docs"
        ],
        "{made}"
    );
    let mut created = made["value"]["createdLabels"]
        .as_array()
        .expect("labels")
        .clone();
    created.sort_by_key(|label| label.as_str().map(str::to_owned));
    assert_eq!(created, vec![json!("To fix"), json!("Went well")], "{made}");
    let flaky = tasks[2]["id"].as_str().expect("task");
    let opened = tool(
        &setup.app,
        &token,
        &session,
        "task_open",
        json!({"taskId": flaky}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let text = opened.to_string();
    assert!(text.contains("The file tests time out"), "{opened}");
    assert!(text.contains(&format!("/whiteboards/{board}")), "{opened}");
    assert!(text.contains("To fix"), "{opened}");

    // The board as a page.
    let page = tool(
        &setup.app,
        &token,
        &session,
        "page_make",
        json!({"whiteboardId": board, "clientRequestId": "retro-page"}),
    )
    .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    let read = tool(
        &setup.app,
        &token,
        &session,
        "page_get",
        json!({"pageId": page["value"]["id"]}),
    )
    .await;
    let body = read["value"]["page"]["body"]
        .as_str()
        .or_else(|| read["value"]["body"].as_str())
        .expect("body")
        .to_owned();
    assert_eq!(
        body.trim(),
        "## Went well\n\n- Fast reviews\n- Clear plan\n\n## To fix\n\n- Flaky tests\n  The file tests time out\n- Slow deploys\n- Missing docs",
        "{read}"
    );
    let refused = tool(
        &setup.app,
        &kite_token,
        &kite_session,
        "page_make",
        json!({"whiteboardId": board, "spaceId": other, "clientRequestId": "kite-page"}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
}

/// C1.10: board writes are changes. An agent's edits are one change each,
/// a person's drawing joins into one while they keep drawing, and undo puts
/// back the elements nobody touched since (an added one goes).
#[tokio::test]
async fn board_changes_are_listed_and_undone() {
    let Some(setup) = setup("c110_board_undo").await else {
        return;
    };
    let space = space(&setup).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let owner = &setup.browser;
    let (wren, token) = create_agent(&setup.app, owner, &space, "Wren", "wren").await;
    add_member(&setup, &work, wren, "member").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        let app = &setup.app;
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let created = call(
        "whiteboard_create",
        json!({"projectId": work, "title": "Flow", "operations": [
            {"op": "add", "type": "rectangle", "id": "plan", "text": "Plan"},
        ]}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let board = created["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let edited = call(
        "whiteboard_edit",
        json!({"whiteboardId": board, "operations": [
            {"op": "update", "id": "plan", "x": 400},
            {"op": "add", "type": "ellipse", "id": "ship", "x": 800},
        ]}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let listed = call(
        "change_list",
        json!({"thingKind": "whiteboard", "thingId": board}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let changes = listed["value"]["changes"].as_array().expect("changes");
    assert_eq!(changes.len(), 2, "one change per agent edit: {listed}");
    let touched = changes[0]["touched"]["elements"]
        .as_array()
        .expect("elements");
    assert!(
        touched.contains(&json!("plan")) && touched.contains(&json!("ship")),
        "{listed}"
    );

    let undo = call(
        "change_undo",
        json!({"changeSetId": changes[0]["id"], "clientRequestId": "board-undo"}),
    )
    .await;
    assert_eq!(undo["outcome"], "ok", "{undo}");
    let now = scene(&setup, owner, &board, None).await;
    assert_eq!(
        element(&now, "plan").expect("plan")["x"],
        json!(0.0),
        "{now}"
    );
    assert!(
        element(&now, "ship").is_none_or(|ship| ship["isDeleted"] == true),
        "{now}"
    );

    // A person's drawing joins one change while they keep at it.
    let raw = call(
        "whiteboard_read",
        json!({"whiteboardId": board, "elementIds": ["plan"], "raw": true}),
    )
    .await;
    let mut plan = raw["value"]["raw"][0].clone();
    for step in 1..=3 {
        plan["y"] = json!(step * 100);
        plan["version"] = json!(plan["version"].as_i64().expect("version") + 1);
        write(&setup, owner, &board, vec![plan.clone()]).await;
    }
    let listed = call(
        "change_list",
        json!({"thingKind": "whiteboard", "thingId": board}),
    )
    .await;
    let changes = listed["value"]["changes"].as_array().expect("changes");
    assert_eq!(changes.len(), 4, "three strokes, one change: {listed}");
    let undo = call(
        "change_undo",
        json!({"changeSetId": changes[0]["id"], "clientRequestId": "board-undo-2"}),
    )
    .await;
    assert_eq!(undo["outcome"], "ok", "{undo}");
    let now = scene(&setup, owner, &board, None).await;
    assert_eq!(
        element(&now, "plan").expect("plan")["y"],
        json!(0.0),
        "{now}"
    );

    // C1.34: history lists each version with its change set; a month on, old
    // versions move out of the table into the file store and still show and
    // restore.
    let history = call("history_list", json!({"thing": board})).await;
    assert_eq!(history["outcome"], "ok", "{history}");
    let entries = history["value"]["entries"].as_array().expect("entries");
    assert!(
        entries.iter().any(|entry| entry["changeSetId"].is_string()),
        "versions name their change: {history}"
    );
    let first = entries
        .iter()
        .filter_map(|entry| entry["version"].as_i64())
        .min()
        .expect("a first version");
    sqlx::query(
        "UPDATE whiteboard_revisions SET created_at = now() - interval '40 days' WHERE whiteboard_id::text = $1",
    )
    .bind(&board)
    .execute(&setup.pool)
    .await
    .expect("age the versions");
    let state = crate::support::app::state_on(&setup.pool);
    let compacted = crate::support::as_server_job(
        agent_workspace_server::whiteboards::store::compact_revisions(&state),
    )
    .await
    .expect("compact");
    assert!(compacted > 0, "old versions move out");
    let inline = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM whiteboard_revisions WHERE whiteboard_id::text = $1 AND payload_key IS NULL AND jsonb_array_length(elements) > 0",
    )
    .bind(&board)
    .fetch_one(&setup.pool)
    .await
    .expect("left inline");
    assert_eq!(inline, 0);
    let shown = call("history_show", json!({"thing": board, "version": first})).await;
    assert_eq!(shown["outcome"], "ok", "{shown}");
    assert!(shown.to_string().contains("plan"), "{shown}");
    let at = scene(&setup, owner, &board, None).await["sceneSeq"]
        .as_i64()
        .expect("scene seq");
    let restored = call(
        "history_restore",
        json!({"thing": board, "version": first, "expectedVersion": at,
               "clientRequestId": "board-restore-compacted"}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    let now = scene(&setup, owner, &board, None).await;
    assert_eq!(
        element(&now, "plan").expect("plan")["x"],
        json!(0.0),
        "{now}"
    );
}

/// C1.29: a board's images are files of the board in the file store. A
/// duplicate adds rows over the same bytes, an image still in the old table
/// is read from there until the move job takes it, and deleting a board
/// for good leaves the bytes its copy still shows.
#[tokio::test]
async fn board_images_live_in_the_file_store() {
    let Some(setup) = setup("w16_images").await else {
        return;
    };
    let state = crate::support::app::state_on(&setup.pool);
    let work = create_project(&setup, "IMG", "Images").await;
    let owner = &setup.browser;
    let board = create_board(
        &setup,
        owner,
        json!({"title": "Moodboard", "projectId": work}),
    )
    .await;
    let (status, stored) = request(
        &setup,
        owner,
        "POST",
        &format!("/api/v1/whiteboards/{board}/files"),
        json!({"id": "photo1", "mimeType": "image/png", "dataURL": TINY_PNG}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let (kind, purpose, key) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT thing_kind, purpose, storage_key FROM files WHERE thing_id::text = $1 AND external_key = 'photo1'",
    )
    .bind(&board)
    .fetch_one(&setup.pool)
    .await
    .expect("the image is a file of the board");
    assert_eq!((kind.as_str(), purpose.as_str()), ("whiteboard", "inline"));
    let file_url = |board: &str, file: &str| format!("/api/v1/whiteboards/{board}/files/{file}");
    let (status, file) = request(
        &setup,
        owner,
        "GET",
        &file_url(&board, "photo1"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["dataURL"], TINY_PNG);

    // A copy shows the image over the same stored bytes.
    let (status, copy) = op(
        &setup,
        owner,
        "whiteboard_duplicate",
        json!({"whiteboardId": board}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    let copy = copy["whiteboard"]["id"].as_str().expect("copy").to_owned();
    let copied_key = sqlx::query_scalar::<_, String>(
        "SELECT storage_key FROM files WHERE thing_id::text = $1 AND external_key = 'photo1'",
    )
    .bind(&copy)
    .fetch_one(&setup.pool)
    .await
    .expect("the copy's image");
    assert_eq!(copied_key, key, "no bytes copied");

    // An image from before the move is read from the old table, then moved.
    const GIF: &str = "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
    let gif = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, GIF).expect("gif");
    sqlx::query(
        r#"
        INSERT INTO whiteboard_files (whiteboard_id, file_id, mime_type, data, created_by_actor_id)
        SELECT id, 'legacy1', 'image/gif', $2, created_by_actor_id FROM whiteboards WHERE id::text = $1
        "#,
    )
    .bind(&board)
    .bind(&gif)
    .execute(&setup.pool)
    .await
    .expect("an old image");
    let gif_url = format!("data:image/gif;base64,{GIF}");
    let (_, before) = request(
        &setup,
        owner,
        "GET",
        &file_url(&board, "legacy1"),
        Value::Null,
    )
    .await;
    assert_eq!(before["dataURL"], gif_url.as_str(), "{before}");
    let listed = scene(&setup, owner, &board, None).await;
    assert_eq!(
        listed["files"].as_array().map(Vec::len),
        Some(2),
        "{listed}"
    );
    let more =
        crate::support::as_server_job(agent_workspace_server::whiteboards::store::move_images(
            &state,
            std::time::Duration::from_secs(30),
        ))
        .await
        .expect("move");
    assert!(!more);
    let left = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM whiteboard_files")
        .fetch_one(&setup.pool)
        .await
        .expect("left");
    assert_eq!(left, 0);
    let (_, after) = request(
        &setup,
        owner,
        "GET",
        &file_url(&board, "legacy1"),
        Value::Null,
    )
    .await;
    assert_eq!(after["dataURL"], gif_url.as_str(), "{after}");
    let listed = scene(&setup, owner, &board, None).await;
    assert_eq!(
        listed["files"].as_array().map(Vec::len),
        Some(2),
        "{listed}"
    );

    // Deleting the original for good keeps the bytes its copy shows.
    let path = format!("/api/v1/whiteboards/{board}");
    let (status, _) = op(
        &setup,
        owner,
        "whiteboard_update",
        json!({"whiteboardId": board, "deleted": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&setup, owner, "DELETE", &path, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    agent_workspace_server::files::cleanup(&state)
        .await
        .expect("cleanup");
    let (status, kept) = request(
        &setup,
        owner,
        "GET",
        &file_url(&copy, "photo1"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{kept}");
    assert_eq!(kept["dataURL"], TINY_PNG);
}

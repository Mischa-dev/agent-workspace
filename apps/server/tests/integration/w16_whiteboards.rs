//! W16: whiteboards. The costly failures are silent: a board (or its images)
//! reaching someone outside its project, a concurrent edit quietly dropping
//! someone else's change, an agent writing where it can't see, links that
//! delete or copy the board, and changes that never reach open boards.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{Browser, create_agent, initialize, send, tool},
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

async fn create_board(setup: &Setup, browser: &Browser, body: Value) -> String {
    let (status, created) = request(setup, browser, "POST", "/api/v1/whiteboards", body).await;
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
        "SELECT count(*) FROM outbox_events WHERE event_type = 'whiteboard.updated' AND payload->>'whiteboardId' = $1",
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
        json!({"title": "  Whole   team ", "organizationId": setup.organization_id}),
    )
    .await;
    // A project board, linked to a task as it's made.
    let (status, task) = send(
        &setup.app,
        owner.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("task-sync"),
            json!({"title": "Design the sync"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let private = create_board(
        &setup,
        owner,
        json!({"title": "Sync design", "projectId": work,
               "link": {"targetKind": "task", "targetId": task}}),
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
    let (status, _) = request(
        &setup,
        &mara,
        "POST",
        &format!("/api/v1/whiteboards/{open}/links"),
        json!({"targetKind": "task", "targetId": task}),
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
        let (status, linked) = request(
            &setup,
            owner,
            "POST",
            &format!("/api/v1/whiteboards/{private}/links"),
            json!({"targetKind": kind, "targetId": target}),
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
    let (status, _) = request(
        &setup,
        owner,
        "DELETE",
        &format!("/api/v1/whiteboards/{private}/links/task/{task}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, trashed) = request(
        &setup,
        owner,
        "PATCH",
        &format!("/api/v1/whiteboards/{private}"),
        json!({"deleted": true, "title": "Sync design v2"}),
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
    let (status, _) = request(
        &setup,
        owner,
        "PATCH",
        &format!("/api/v1/whiteboards/{private}"),
        json!({"deleted": false}),
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
    let (status, copy) = request(
        &setup,
        owner,
        "POST",
        &format!("/api/v1/whiteboards/{private}/duplicate"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    assert_eq!(copy["whiteboard"]["title"], "Copy of Sync design v2");
    assert_eq!(copy["links"], json!([]));
    let copy = copy["whiteboard"]["id"].as_str().expect("copy").to_owned();
    let copied = scene(&setup, owner, &copy, None).await;
    assert!(element(&copied, "box").is_some(), "{copied}");
    assert_eq!(copied["files"][0]["id"], "photo1");

    // Moving the board to the whole workspace lets Mara in.
    let (status, moved) = request(
        &setup,
        owner,
        "PATCH",
        &format!("/api/v1/whiteboards/{private}"),
        json!({"projectId": null}),
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
    let (status, copy) = request(
        &setup,
        owner,
        "POST",
        &format!("{path}/duplicate"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    let copy = copy["whiteboard"]["id"].as_str().expect("copy").to_owned();
    let (status, _) = request(&setup, owner, "PATCH", &path, json!({"deleted": true})).await;
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
    let (status, _) = request(
        &setup,
        owner,
        "PATCH",
        &format!("/api/v1/whiteboards/{kept}"),
        json!({"deleted": true}),
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
    let (status, _) = request(
        &setup,
        &viewer,
        "PATCH",
        &format!("/api/v1/whiteboards/{own}"),
        json!({"deleted": true}),
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
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("task-launch"),
            json!({"title": "Plan the launch"}),
        ),
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
        "SELECT count(*) FROM audit_events WHERE action = 'whiteboard.edited' AND actor_id = $1",
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

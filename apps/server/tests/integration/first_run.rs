//! H1.14: first run in the browser. The silent, costly failures: a setup
//! code that works twice, or after someone has an account; a code that
//! anyone can read; a refused attempt that leaves an account or workspace
//! behind; and an import's upload taken from someone other than the
//! person who made the first account for it.

use agent_workspace_server::{export::transfer, setup};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::support::{
    app::{Browser, app, send},
    db::db,
};

const ORIGIN: &str = "http://127.0.0.1:3100";

/// POSTs `body` with no session; the status, body and the browser its
/// cookies make.
pub(crate) async fn post_setup(
    router: &axum::Router,
    body: Value,
) -> (StatusCode, Value, Option<Browser>) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup")
                .header("origin", ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("body")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
        .collect::<Vec<_>>();
    let browser = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .map(|csrf| Browser {
            csrf: csrf.to_owned(),
            cookie: cookies.join("; "),
        });
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value, browser)
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count")
}

fn person(login: &str) -> Value {
    json!({
        "displayName": "First Person",
        "loginName": login,
        "password": "a long first-run password",
    })
}

#[tokio::test]
async fn the_first_person_sets_up_once_and_imports_only_for_themselves() {
    let Some(first) = db("h1_14_first_run").await else {
        return;
    };
    let data = tempfile::tempdir().expect("data root");
    let (router, state) = app(&first)
        .with(
            "AGENT_WORKSPACE_DATA_ROOT",
            data.path().to_string_lossy().into_owned(),
        )
        .build();
    let pool = &first.pool;
    assert_eq!(count(pool, "users").await, 0, "a fresh server");

    // A code waits in the data root, for its owner's eyes only.
    let prepared = setup::prepare(pool, &state.config).await.expect("prepare");
    let path = setup::code_path(&state.config);
    assert_eq!(prepared, setup::Prepared::Waiting(path.clone()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("code file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "only its owner reads the code: {mode:o}");
    }
    let code = std::fs::read_to_string(&path).expect("code");
    assert_eq!(
        setup::prepare(pool, &state.config).await.expect("again"),
        setup::Prepared::Waiting(path.clone()),
        "starting again keeps the same code"
    );
    assert_eq!(std::fs::read_to_string(&path).expect("code"), code);

    let (status, shown) = send(
        &router,
        Request::builder()
            .uri("/api/v1/setup")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown["needed"], true, "{shown}");
    assert!(
        shown["shapes"]
            .as_array()
            .is_some_and(|shapes| shapes.iter().any(|shape| shape["id"] == "small_team")),
        "{shown}"
    );

    // A wrong code makes nothing.
    let workspace = json!({ "name": "First Place", "shape": "small_team" });
    let (status, refused, _) = post_setup(
        &router,
        json!({ "code": "abcd-efgh", "person": person("first"), "workspace": workspace }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(count(pool, "users").await, 0);
    assert_eq!(count(pool, "organizations").await, 0);

    // The right one: the person, signed in, owning a workspace in the shape
    // they chose.
    let (status, session, browser) = post_setup(
        &router,
        json!({ "code": code.trim().to_uppercase(), "person": person("first"), "workspace": workspace }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    let browser = browser.expect("signed in");
    let (workspace_id, role) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT membership.organization_id, membership.role \
         FROM organization_memberships membership \
         JOIN users person ON person.actor_id = membership.actor_id \
         WHERE person.login_name = 'first'",
    )
    .fetch_one(pool)
    .await
    .expect("the first person's workspace");
    assert_eq!(role, "owner");
    let shape = sqlx::query_scalar::<_, String>(
        "SELECT template FROM workspace_template_uses WHERE organization_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await
    .expect("its shape");
    assert_eq!(shape, "small_team");
    assert!(!path.exists(), "the code's file goes");
    assert_eq!(count(pool, "server_setup").await, 0);

    // The code works once.
    let (status, again, _) = post_setup(
        &router,
        json!({ "code": code.trim(), "person": person("second"), "workspace": { "name": "Again" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(count(pool, "users").await, 1);
    assert_eq!(
        setup::prepare(pool, &state.config).await.expect("later"),
        setup::Prepared::Done
    );
    assert!(!path.exists(), "no new code once someone has an account");

    // An archive of it, to bring to a second server.
    let (status, started) = send(
        &router,
        browser.request(
            "POST",
            &format!("/api/v2/w/{workspace_id}/ops/workspace_export_start"),
            Some("h1-14-export"),
            json!({}),
        ),
    )
    .await;
    assert!(status.is_success(), "{started}");
    let export = started["value"]["id"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("export");
    transfer::run_export(&state, export).await.expect("export");
    let response = router
        .clone()
        .oneshot(browser.request(
            "GET",
            &transfer::download_path(workspace_id, export),
            None,
            Value::Null,
        ))
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    let archive = to_bytes(response.into_body(), 256 * 1024 * 1024)
        .await
        .expect("archive");

    // The second server: the first person's account first, then the upload.
    let Some(second) = db("h1_14_first_run_import").await else {
        return;
    };
    let data = tempfile::tempdir().expect("data root");
    let (router, state) = app(&second)
        .with(
            "AGENT_WORKSPACE_DATA_ROOT",
            data.path().to_string_lossy().into_owned(),
        )
        .build();
    let pool = &second.pool;
    setup::prepare(pool, &state.config).await.expect("prepare");
    let code = std::fs::read_to_string(setup::code_path(&state.config)).expect("code");
    let (status, _, importer) =
        post_setup(&router, json!({ "code": code, "person": person("first") })).await;
    assert_eq!(status, StatusCode::OK);
    let importer = importer.expect("signed in");
    assert_eq!(count(pool, "organizations").await, 0, "no workspace yet");
    let (_, shown) = send(
        &router,
        importer.request("GET", "/api/v1/setup", None, Value::Null),
    )
    .await;
    assert_eq!(shown["awaitingImport"], true, "{shown}");
    let upload = |who: &Browser, bytes: Vec<u8>| {
        let mut request = who.request("POST", "/api/v1/setup/import", None, Value::Null);
        *request.body_mut() = Body::from(bytes);
        request
    };

    // Nobody else uploads it, not even with the code.
    let (status, _) = send(
        &router,
        Request::builder()
            .method("POST")
            .uri("/api/v1/setup/import")
            .header("origin", ORIGIN)
            .body(Body::from(archive.to_vec()))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A file that isn't an archive is refused, and the person can try again.
    let (status, refused) = send(&router, upload(&importer, b"not a zip".to_vec())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(count(pool, "organizations").await, 0);
    assert_eq!(count(pool, "server_setup").await, 1);

    let (status, accepted) = send(&router, upload(&importer, archive.to_vec())).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let transfer_id = accepted["transferId"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("transfer");
    assert_eq!(count(pool, "server_setup").await, 0, "setup is over");
    assert!(
        agent_workspace_server::export::import::run_import(
            &state,
            transfer_id,
            agent_workspace_server::export::import::RunLimits::default(),
        )
        .await
        .expect("import")
    );
    let (status, progress) = send(
        &router,
        importer.request(
            "GET",
            &format!("/api/v1/setup/import/{transfer_id}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{progress}");
    assert_eq!(progress["state"], "succeeded", "{progress}");
    assert_eq!(
        progress["slug"],
        json!(started_slug(&first.pool, workspace_id).await)
    );
    let owner = sqlx::query_scalar::<_, String>(
        "SELECT membership.role FROM organization_memberships membership \
         JOIN users person ON person.actor_id = membership.actor_id \
         WHERE person.login_name = 'first' AND membership.organization_id = $1",
    )
    .bind(
        progress["workspaceId"]
            .as_str()
            .and_then(|id| id.parse::<Uuid>().ok())
            .expect("workspace"),
    )
    .fetch_one(pool)
    .await
    .expect("the importer's membership");
    assert_eq!(owner, "owner");

    first.finish().await;
    second.finish().await;
}

async fn started_slug(pool: &PgPool, workspace: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
        .bind(workspace)
        .fetch_one(pool)
        .await
        .expect("slug")
}

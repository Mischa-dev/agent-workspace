//! Bug report links: another app POSTs a report to a project's secret link
//! and it becomes a task there, made as the link's maker. A resend makes no
//! second task, a paused or replaced link takes nothing, other people can't
//! change someone's link, and a link whose maker lost the project turns
//! itself off instead of acting for them.

use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex},
};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    seed::dev_seed,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::Executor;
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    m3_scheduler_http::{integration_pool, schema_pool},
    w2_harness_platform::{MEMBER_LOGIN, insert_member, login, send, send_get},
};

const OWNER_LOGIN: &str = "bug-report-owner";

/// Every log line written on this thread while it's held.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log lock")).into_owned()
    }
}
const PASSWORD: &str = "correct horse battery staple";

/// A POST from another app: no cookie, no CSRF token, any origin.
async fn report(
    application: &axum::Router,
    path: &str,
    content_type: &str,
    body: &str,
) -> (StatusCode, Value, Option<String>) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("origin", "https://game.example.com")
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body.to_owned()))
                .expect("build report"),
        )
        .await
        .expect("report response");
    let status = response.status();
    let cors = response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("report body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value, cors)
}

fn path_of(url: &str) -> String {
    let start = url.find("/api/v1/bug-reports/").expect("report path");
    url[start..].to_owned()
}

#[tokio::test]
async fn bug_report_links_make_tasks_as_their_maker_only() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("bug_reports_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    let temporary = tempfile::tempdir().expect("temporary root");
    let data_root = temporary.path().join("data");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data_root
                .join("codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("configuration");
    let seed = dev_seed(&pool, &config).await.expect("seed");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OWNER_LOGIN.to_owned(),
            display_name: "Report Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    insert_member(&pool, seed.organization_id, seed.project_id).await;
    // Reports become bugs when the project has a Bug type.
    sqlx::query(
        r#"
        INSERT INTO work_item_types (id, project_id, name, position)
        SELECT $1, $2, 'Bug', 900
        WHERE NOT EXISTS (
            SELECT 1 FROM work_item_types
            WHERE project_id = $2 AND enabled AND lower(name) = 'bug'
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(seed.project_id)
    .execute(&pool)
    .await
    .expect("bug type");

    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(64),
        supervisors,
    };
    let application = router(state);
    let (owner_cookie, owner_csrf) = login(&application, OWNER_LOGIN).await;
    let (member_cookie, member_csrf) = login(&application, MEMBER_LOGIN).await;
    let owner = (owner_cookie.as_str(), owner_csrf.as_str());
    let member = (member_cookie.as_str(), member_csrf.as_str());
    let links = format!("/api/v1/projects/{}/bug-report-links", seed.project_id);

    let created = send(
        &application,
        owner,
        "POST",
        &links,
        "link",
        json!({"name": "Ragdoll Rapture"}),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    let link_id = created.2["link"]["id"]
        .as_str()
        .expect("link id")
        .to_owned();
    let url = created.2["url"].as_str().expect("url").to_owned();
    let path = path_of(&url);
    assert!(!created.1.contains("tokenDigest"));

    // The link's secret never reaches the logs: request lines carry the
    // route's pattern instead of the path.
    let token = path.rsplit('/').next().expect("token").to_owned();
    let logs = CapturedLogs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let (status, _, _) = report(&application, &path, "text/plain", "Logged report").await;
    let (status_bad, _, _) = report(&application, &path, "application/json", "{not json").await;
    drop(guard);
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(status_bad, StatusCode::BAD_REQUEST);
    let logged = logs.text();
    assert!(
        logged.contains("/api/v1/bug-reports/{token}"),
        "request lines name the route: {logged}"
    );
    assert!(!logged.contains(&token), "the secret was logged: {logged}");

    // A JSON report from a game: a task, as the owner, typed Bug, with the
    // extra fields listed.
    let body = json!({
        "title": "Ragdoll falls through the floor",
        "description": "On level 3 after a respawn.",
        "priority": "critical",
        "version": "1.4.2",
        "stack": "at physics.step\nat world.tick",
    })
    .to_string();
    let (status, sent, cors) = report(&application, &path, "application/json", &body).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(cors.as_deref(), Some("*"));
    assert_eq!(sent["duplicate"], json!(false));
    let key = sent["task"].as_str().expect("task key").to_owned();
    let (title, description, priority, creator, type_name) =
        sqlx::query_as::<_, (String, String, String, Uuid, Option<String>)>(
            r#"
            SELECT item.title, item.description, item.priority, item.created_by_actor_id,
                   item_type.name
            FROM work_items item
            LEFT JOIN work_item_types item_type ON item_type.id = item.type_id
            WHERE item.project_id = $1 AND item.title = 'Ragdoll falls through the floor'
            "#,
        )
        .bind(seed.project_id)
        .fetch_one(&pool)
        .await
        .expect("reported task");
    assert_eq!(creator, seed.human_actor_id);
    assert_eq!(priority, "urgent");
    assert_eq!(type_name.as_deref(), Some("Bug"));
    assert!(description.starts_with("On level 3 after a respawn."));
    assert!(description.contains("- **version**: 1.4.2"));
    assert!(description.contains("at physics.step\nat world.tick"));
    assert!(description.contains("Ragdoll Rapture"));
    let _ = title;

    // The same report again gives back the same task.
    let (status, again, _) = report(&application, &path, "application/json", &body).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["duplicate"], json!(true));
    assert_eq!(again["task"].as_str(), Some(key.as_str()));

    // sendBeacon's text/plain JSON, and plain text, both work.
    let (status, beacon, _) = report(
        &application,
        &path,
        "text/plain;charset=UTF-8",
        r#"{"message":"Menu freezes"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{beacon}");
    let (status, empty, _) = report(&application, &path, "text/plain", "   ").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{empty}");

    // A browser on another origin may ask first.
    let preflight = application
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri(&path)
                .header("origin", "https://game.example.com")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .expect("build preflight"),
        )
        .await
        .expect("preflight");
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        preflight.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "*"
    );

    let (status, unknown, _) = report(
        &application,
        "/api/v1/bug-reports/awbr_not-a-real-token",
        "application/json",
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");

    // The member sees the link and its count but can't change it.
    let listed = send_get(&application, member, &links).await;
    assert_eq!(listed.0, StatusCode::OK, "{}", listed.1);
    assert_eq!(listed.2["links"][0]["reportCount"], json!(3));
    assert_eq!(listed.2["links"][0]["canManage"], json!(false));
    assert!(!listed.1.contains("awbr_"));
    let link_path = format!("/api/v1/bug-report-links/{link_id}");
    let refused = send(
        &application,
        member,
        "PATCH",
        &link_path,
        "member-pause",
        json!({"state": "paused"}),
    )
    .await;
    assert_eq!(refused.0, StatusCode::NOT_FOUND, "{}", refused.1);

    // Paused, it takes nothing.
    let paused = send(
        &application,
        owner,
        "PATCH",
        &link_path,
        "pause",
        json!({"state": "paused"}),
    )
    .await;
    assert_eq!(paused.0, StatusCode::OK, "{}", paused.1);
    let (status, _, _) = report(&application, &path, "text/plain", "Paused report").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let resumed = send(
        &application,
        owner,
        "PATCH",
        &link_path,
        "resume",
        json!({"state": "active"}),
    )
    .await;
    assert_eq!(resumed.0, StatusCode::OK, "{}", resumed.1);

    // Replaced, the old link stops working and the new one works.
    let replaced = send(
        &application,
        owner,
        "POST",
        &format!("{link_path}/replace"),
        "replace",
        json!({}),
    )
    .await;
    assert_eq!(replaced.0, StatusCode::OK, "{}", replaced.1);
    let new_path = path_of(replaced.2["url"].as_str().expect("new url"));
    assert_ne!(new_path, path);
    let (status, _, _) = report(&application, &path, "text/plain", "Old link").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = report(&application, &new_path, "text/plain", "New link").await;
    assert_eq!(status, StatusCode::CREATED);

    // A link whose maker left the project turns itself off.
    let member_link = send(
        &application,
        member,
        "POST",
        &links,
        "member-link",
        json!({"name": "Website"}),
    )
    .await;
    assert_eq!(member_link.0, StatusCode::CREATED, "{}", member_link.1);
    let member_path = path_of(member_link.2["url"].as_str().expect("member url"));
    let (status, _, _) = report(&application, &member_path, "text/plain", "Works").await;
    assert_eq!(status, StatusCode::CREATED);
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND handle = 'w2-member'",
    )
    .bind(seed.project_id)
    .execute(&pool)
    .await
    .expect("remove member");
    let (status, _, _) = report(&application, &member_path, "text/plain", "After").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (state, reason) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, disabled_reason FROM bug_report_links WHERE name = 'Website'",
    )
    .fetch_one(&pool)
    .await
    .expect("member link");
    assert_eq!(state, "disabled");
    assert!(reason.expect("reason").contains("can no longer add tasks"));
    let after = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_items WHERE project_id = $1 AND title = 'After'",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(after, 0);
}

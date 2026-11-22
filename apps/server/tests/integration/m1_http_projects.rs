use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn authenticated_project_lifecycle_is_scoped_csrf_safe_and_idempotent() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace=debug")
        .try_init();
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let administrative_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("m1_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated HTTP schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply current migrations");
    let bootstrapped = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "ada".to_owned(),
            display_name: "Ada Owner".to_owned(),
            organization_name: Some("Personal".to_owned()),
            organization_slug: Some(format!("personal-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        "correct horse battery staple".to_owned(),
    )
    .await
    .expect("bootstrap administrator");
    let app = test_app(pool.clone(), &database_url);

    let login = request(
        "POST",
        "/api/v1/auth/login",
        &[("origin", "http://127.0.0.1:3100")],
        json!({
            "loginName": "ada",
            "password": "correct horse battery staple"
        }),
    );
    let login_response = app.clone().oneshot(login).await.expect("login response");
    assert_eq!(login_response.status(), StatusCode::OK);
    let cookies = login_response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("cookie header")
                .split(';')
                .next()
                .expect("cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(cookies.len(), 2);
    let cookie_header = cookies.join("; ");
    let _ = response_json(login_response).await;

    let rotated_login = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/auth/login",
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
            ],
            json!({
                "loginName": "ada",
                "password": "correct horse battery staple"
            }),
        ))
        .await
        .expect("rotated login response");
    assert_eq!(rotated_login.status(), StatusCode::OK);
    let rotated_cookies = rotated_login
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("rotated cookie header")
                .split(';')
                .next()
                .expect("rotated cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let rotated_cookie_header = rotated_cookies.join("; ");
    let rotated_csrf = rotated_cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("rotated CSRF cookie")
        .to_owned();
    assert_ne!(rotated_cookie_header, cookie_header);
    let _ = response_json(rotated_login).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE revoked_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .expect("count rotated Sessions"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sessions WHERE rotated_from_session_id IS NOT NULL"
        )
        .fetch_one(&pool)
        .await
        .expect("count linked Session rotations"),
        1
    );
    let cookie_header = rotated_cookie_header;
    let csrf = rotated_csrf;

    let create_uri = format!(
        "/api/v2/w/{}/ops/space_create",
        bootstrapped.organization_id
    );
    let create_body = json!({
        "key": "one",
        "name": "First Project"
    });
    let create_headers = [
        ("origin", "http://127.0.0.1:3100"),
        ("cookie", cookie_header.as_str()),
        ("x-csrf-token", csrf.as_str()),
        ("idempotency-key", "m1-create-first"),
    ];
    let missing_csrf = request(
        "POST",
        &create_uri,
        &[
            ("origin", "http://127.0.0.1:3100"),
            ("cookie", cookie_header.as_str()),
            ("idempotency-key", "must-not-commit"),
        ],
        create_body.clone(),
    );
    assert_eq!(
        app.clone()
            .oneshot(missing_csrf)
            .await
            .expect("CSRF response")
            .status(),
        StatusCode::FORBIDDEN
    );
    let mismatched_csrf = request(
        "POST",
        &create_uri,
        &[
            ("origin", "http://127.0.0.1:3100"),
            ("cookie", cookie_header.as_str()),
            ("x-csrf-token", "not-the-cookie-token"),
            ("idempotency-key", "must-also-not-commit"),
        ],
        create_body.clone(),
    );
    assert_eq!(
        app.clone()
            .oneshot(mismatched_csrf)
            .await
            .expect("mismatched CSRF response")
            .status(),
        StatusCode::FORBIDDEN
    );

    let first_create = app
        .clone()
        .oneshot(request(
            "POST",
            &create_uri,
            &create_headers,
            create_body.clone(),
        ))
        .await
        .expect("create Project");
    assert_eq!(first_create.status(), StatusCode::OK);
    let first_project = response_json(first_create).await["value"].clone();
    let first_project_id = first_project["id"].as_str().expect("Project id");
    assert_eq!(first_project["key"], "ONE");

    let replay = app
        .clone()
        .oneshot(request(
            "POST",
            &create_uri,
            &create_headers,
            create_body.clone(),
        ))
        .await
        .expect("replay Project creation");
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = response_json(replay).await;
    assert_eq!(replay["replayed"], true, "{replay}");
    assert_eq!(replay["value"]["id"], first_project_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM projects WHERE project_key = 'ONE'")
            .fetch_one(&pool)
            .await
            .expect("count Projects"),
        1
    );

    let duplicate_key = app
        .clone()
        .oneshot(request(
            "POST",
            &create_uri,
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
                ("x-csrf-token", csrf.as_str()),
                ("idempotency-key", "m1-create-duplicate-key"),
            ],
            json!({
                "key": "oNe",
                "name": "Duplicate Project Key"
            }),
        ))
        .await
        .expect("duplicate Project key response");
    assert_eq!(duplicate_key.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(duplicate_key).await["code"],
        "space_key_taken"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM projects WHERE lower(project_key) = 'one'"
        )
        .fetch_one(&pool)
        .await
        .expect("count Projects after duplicate-key conflict"),
        1,
        "a duplicate-key HTTP conflict must roll back its Project transaction"
    );

    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(bootstrapped.organization_id)
    .bind(bootstrapped.actor_id)
    .execute(&pool)
    .await
    .expect("disable Organization membership");
    let revoked_replay = app
        .clone()
        .oneshot(request(
            "POST",
            &create_uri,
            &create_headers,
            create_body.clone(),
        ))
        .await
        .expect("revoked idempotent replay");
    assert_eq!(
        revoked_replay.status(),
        StatusCode::NOT_FOUND,
        "cached responses never bypass current Organization authority"
    );
    sqlx::query(
        "UPDATE organization_memberships SET enabled = true WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(bootstrapped.organization_id)
    .bind(bootstrapped.actor_id)
    .execute(&pool)
    .await
    .expect("restore Organization membership");

    // A replay hands back the answer as it was first given; reading the
    // space (below) is what checks access as it is now.

    let switch = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/projects/{first_project_id}/switch"),
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
                ("x-csrf-token", csrf.as_str()),
            ],
            json!({"expectedVersion": 1}),
        ))
        .await
        .expect("switch Project");
    assert_eq!(switch.status(), StatusCode::OK);
    assert_eq!(response_json(switch).await["navigation"]["version"], 2);
    let stale_switch = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/projects/{first_project_id}/switch"),
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
                ("x-csrf-token", csrf.as_str()),
            ],
            json!({"expectedVersion": 1}),
        ))
        .await
        .expect("stale switch response");
    assert_eq!(stale_switch.status(), StatusCode::CONFLICT);
    let stale_switch_body = response_json(stale_switch).await;
    assert_eq!(stale_switch_body["error"]["code"], "version_conflict");
    assert_eq!(
        stale_switch_body["error"]["details"]["currentVersion"],
        serde_json::Value::from(2)
    );

    let foreign_actor_id = Uuid::now_v7();
    let foreign_project_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(foreign_actor_id)
        .execute(&pool)
        .await
        .expect("insert foreign Actor");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'OTHER', 'Other Project')",
    )
    .bind(foreign_project_id)
    .bind(bootstrapped.organization_id)
    .execute(&pool)
    .await
    .expect("insert foreign Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'viewer', 'Other User', 'other-user')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(foreign_project_id)
    .bind(foreign_actor_id)
    .execute(&pool)
    .await
    .expect("insert foreign membership");
    let cross_project = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/projects/{foreign_project_id}"),
            &[("cookie", cookie_header.as_str())],
            Value::Null,
        ))
        .await
        .expect("cross-Project response");
    assert_eq!(cross_project.status(), StatusCode::NOT_FOUND);

    let list = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/projects",
            &[("cookie", cookie_header.as_str())],
            Value::Null,
        ))
        .await
        .expect("list response");
    let listed = response_json(list).await;
    assert_eq!(listed["projects"].as_array().expect("Projects").len(), 1);

    // A Project whose name sorts first and contains the key must not push the
    // exact key out of a limited search.
    let named_like_key = app
        .clone()
        .oneshot(request(
            "POST",
            &create_uri,
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
                ("x-csrf-token", csrf.as_str()),
                ("idempotency-key", "m1-create-named-like-key"),
            ],
            json!({
                "key": "alp",
                "name": "Alpha one notes"
            }),
        ))
        .await
        .expect("create second Project");
    assert_eq!(named_like_key.status(), StatusCode::OK);
    let second_project_id = response_json(named_like_key).await["value"]["id"]
        .as_str()
        .expect("second Project id")
        .to_owned();
    let exact_first = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/projects?q=one&limit=1",
            &[("cookie", cookie_header.as_str())],
            Value::Null,
        ))
        .await
        .expect("search response");
    let exact_first = response_json(exact_first).await;
    assert_eq!(exact_first["projects"][0]["key"], "ONE");

    let active_run_id = insert_active_run_fixture(
        &pool,
        first_project_id.parse().expect("Project UUID"),
        bootstrapped.actor_id,
    )
    .await;
    let blocked_archive = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v2/ops/space_archive",
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
                ("x-csrf-token", csrf.as_str()),
                ("idempotency-key", "archive-blocked"),
            ],
            json!({"spaceId": first_project_id, "expectedVersion": 1}),
        ))
        .await
        .expect("active-run archive response");
    assert_eq!(blocked_archive.status(), StatusCode::CONFLICT);
    assert!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT archived_at::text FROM projects WHERE id = $1"
        )
        .bind(first_project_id.parse::<Uuid>().expect("Project UUID"))
        .fetch_one(&pool)
        .await
        .expect("read active Project")
        .is_none()
    );
    sqlx::query(
        "UPDATE agent_runs SET state = 'completed', finished_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(active_run_id)
    .execute(&pool)
    .await
    .expect("complete archive-blocking run");
    crate::support::builtin_run_moved(&pool, active_run_id, "completed").await;

    let archive = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v2/ops/space_archive",
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
                ("x-csrf-token", csrf.as_str()),
                ("idempotency-key", "archive"),
            ],
            json!({"spaceId": first_project_id, "expectedVersion": 1}),
        ))
        .await
        .expect("archive response");
    let status = archive.status();
    let archived_project = response_json(archive).await;
    assert_eq!(status, StatusCode::OK, "{archived_project}");
    let archived_project = archived_project["value"].clone();
    assert_eq!(archived_project["archived"], true, "{archived_project}");
    assert_eq!(archived_project["version"], 2);

    let repeated_archive = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v2/ops/space_archive",
            &[
                ("origin", "http://127.0.0.1:3100"),
                ("cookie", cookie_header.as_str()),
                ("x-csrf-token", csrf.as_str()),
                ("idempotency-key", "archive"),
            ],
            json!({"spaceId": first_project_id, "expectedVersion": 1}),
        ))
        .await
        .expect("repeated archive response");
    // An archived space is closed to every call but space_restore, so the
    // same archive sent again reads as not found and changes nothing.
    assert_eq!(repeated_archive.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM audit_events
            WHERE action = 'space_archive' AND target_id = $1
            "#,
        )
        .bind(first_project_id.parse::<Uuid>().expect("Project UUID"))
        .fetch_one(&pool)
        .await
        .expect("count Project archive audit events"),
        1,
        "replaying archive must not version or audit the Project twice"
    );

    let archived_replay = app
        .clone()
        .oneshot(request("POST", &create_uri, &create_headers, create_body))
        .await
        .expect("archived idempotent replay");
    assert_eq!(archived_replay.status(), StatusCode::OK);
    let archived_replay = response_json(archived_replay).await;
    assert_eq!(archived_replay["replayed"], true, "{archived_replay}");
    assert_eq!(archived_replay["value"]["id"], first_project_id);

    // Renaming from project settings: managers only, against the version they
    // read, never an archived project, and audited once per real change.
    let write_headers = [
        ("origin", "http://127.0.0.1:3100"),
        ("cookie", cookie_header.as_str()),
        ("x-csrf-token", csrf.as_str()),
    ];
    // Each call is a new change, with its own request id.
    let op = |name: &str, body: Value| {
        let key = Uuid::now_v7().to_string();
        let mut headers = write_headers.to_vec();
        headers.push(("idempotency-key", key.as_str()));
        app.clone().oneshot(request(
            "POST",
            &format!("/api/v2/ops/{name}"),
            &headers,
            body,
        ))
    };
    let rename = |project_id: &str, body: Value| {
        let mut body = body;
        body["spaceId"] = json!(project_id);
        op("space_update", body)
    };
    let archived_rename = rename(
        first_project_id,
        json!({"name": "Back again", "expectedVersion": 2}),
    )
    .await
    .expect("archived rename response");
    assert_eq!(archived_rename.status(), StatusCode::NOT_FOUND);

    // Archived projects are listed only when asked for, and a manager can
    // bring one back against the version they read, audited once.
    let listed_ids = |uri: &'static str| {
        let app = app.clone();
        let cookie = cookie_header.clone();
        async move {
            let response = app
                .oneshot(request(
                    "GET",
                    uri,
                    &[("cookie", cookie.as_str())],
                    Value::Null,
                ))
                .await
                .expect("list response");
            assert_eq!(response.status(), StatusCode::OK);
            response_json(response).await["projects"]
                .as_array()
                .expect("Projects")
                .iter()
                .map(|project| project["id"].as_str().expect("Project id").to_owned())
                .collect::<Vec<_>>()
        }
    };
    let restore = |project_id: &str, expected_version: i64| {
        op(
            "space_restore",
            json!({"spaceId": project_id, "expectedVersion": expected_version}),
        )
    };
    assert!(
        !listed_ids("/api/v1/projects")
            .await
            .contains(&first_project_id.to_owned())
    );
    assert_eq!(
        listed_ids("/api/v1/projects?archived=true").await,
        vec![first_project_id.to_owned()]
    );
    let stale_restore = restore(first_project_id, 1)
        .await
        .expect("stale restore response");
    assert_eq!(stale_restore.status(), StatusCode::CONFLICT);
    let restored = restore(first_project_id, 2)
        .await
        .expect("restore response");
    assert_eq!(restored.status(), StatusCode::OK);
    let restored = response_json(restored).await["value"].clone();
    assert_eq!(restored["archived"], false, "{restored}");
    assert_eq!(restored["version"], 3);
    let repeated_restore = restore(first_project_id, 2)
        .await
        .expect("repeated restore response");
    assert_eq!(repeated_restore.status(), StatusCode::OK);
    assert_eq!(response_json(repeated_restore).await["value"]["version"], 3);
    assert!(
        listed_ids("/api/v1/projects")
            .await
            .contains(&first_project_id.to_owned())
    );
    assert!(
        listed_ids("/api/v1/projects?archived=true")
            .await
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE action = 'space_restore' AND target_id = $1",
        )
        .bind(first_project_id.parse::<Uuid>().expect("Project UUID"))
        .fetch_one(&pool)
        .await
        .expect("count restore audit events"),
        2,
        "each call is audited; the second changed nothing (still version 3)"
    );
    let renamed = rename(
        &second_project_id,
        json!({"name": "  Alpha renamed  ", "expectedVersion": 1}),
    )
    .await
    .expect("rename response");
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed = response_json(renamed).await["value"].clone();
    assert_eq!(renamed["name"], "Alpha renamed", "{renamed}");
    assert_eq!(renamed["key"], "ALP");
    assert_eq!(renamed["version"], 2);
    let stale_rename = rename(
        &second_project_id,
        json!({"name": "Someone else's name", "expectedVersion": 1}),
    )
    .await
    .expect("stale rename response");
    assert_eq!(stale_rename.status(), StatusCode::CONFLICT);
    let stale_rename = response_json(stale_rename).await;
    assert_eq!(
        (&stale_rename["outcome"], &stale_rename["code"]),
        (&json!("conflict"), &json!("version")),
        "{stale_rename}"
    );
    let blank_rename = rename(
        &second_project_id,
        json!({"name": "   ", "expectedVersion": 2}),
    )
    .await
    .expect("blank rename response");
    assert_eq!(blank_rename.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let unchanged = rename(
        &second_project_id,
        json!({"name": "Alpha renamed", "expectedVersion": 2}),
    )
    .await
    .expect("unchanged rename response");
    assert_eq!(unchanged.status(), StatusCode::OK);
    assert_eq!(response_json(unchanged).await["value"]["version"], 2);
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(second_project_id.parse::<Uuid>().expect("Project UUID"))
    .bind(bootstrapped.actor_id)
    .execute(&pool)
    .await
    .expect("make the renamer a Viewer");
    let viewer_rename = rename(
        &second_project_id,
        json!({"name": "Viewer name", "expectedVersion": 2}),
    )
    .await
    .expect("Viewer rename response");
    assert_eq!(viewer_rename.status(), StatusCode::FORBIDDEN);
    let viewer_restore = restore(&second_project_id, 2)
        .await
        .expect("Viewer restore response");
    assert_eq!(viewer_restore.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT name FROM projects WHERE id = $1")
            .bind(second_project_id.parse::<Uuid>().expect("Project UUID"))
            .fetch_one(&pool)
            .await
            .expect("read the Project name"),
        "Alpha renamed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE action = 'space_update' AND target_id = $1",
        )
        .bind(second_project_id.parse::<Uuid>().expect("Project UUID"))
        .fetch_one(&pool)
        .await
        .expect("count rename audit events"),
        2,
        "the rename and the unchanged call; the refused ones leave none"
    );

    let logout_headers = [
        ("origin", "http://127.0.0.1:3100"),
        ("cookie", cookie_header.as_str()),
        ("x-csrf-token", csrf.as_str()),
    ];
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                "/api/v1/auth/logout",
                &logout_headers,
                Value::Null,
            ))
            .await
            .expect("logout")
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                "/api/v1/auth/logout",
                &[("origin", "http://127.0.0.1:3100")],
                Value::Null,
            ))
            .await
            .expect("idempotent logout")
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "GET",
                "/api/v1/projects",
                &[("cookie", cookie_header.as_str())],
                Value::Null,
            ))
            .await
            .expect("revoked session")
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let expiring_login = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/auth/login",
            &[("origin", "http://127.0.0.1:3100")],
            json!({
                "loginName": "ada",
                "password": "correct horse battery staple"
            }),
        ))
        .await
        .expect("expiring login");
    assert_eq!(expiring_login.status(), StatusCode::OK);
    let expiring_cookie = expiring_login
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| {
            cookie
                .split(';')
                .next()
                .and_then(|pair| pair.strip_prefix("agent_workspace_session="))
        })
        .expect("expiring Session cookie")
        .to_owned();
    let _ = response_json(expiring_login).await;
    sqlx::query(
        r#"
        UPDATE sessions
        SET created_at = now() - interval '2 days',
            last_seen_at = now() - interval '2 days',
            idle_expires_at = now() - interval '1 second'
        WHERE revoked_at IS NULL
        "#,
    )
    .execute(&pool)
    .await
    .expect("expire active Session");
    assert_eq!(
        app.clone()
            .oneshot(request(
                "GET",
                "/api/v1/projects",
                &[(
                    "cookie",
                    &format!("agent_workspace_session={expiring_cookie}")
                )],
                Value::Null,
            ))
            .await
            .expect("expired session")
            .status(),
        StatusCode::UNAUTHORIZED
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated HTTP schema");
}

async fn insert_active_run_fixture(pool: &PgPool, project_id: Uuid, owner_actor_id: Uuid) -> Uuid {
    let worker_actor_id = Uuid::now_v7();
    let repository_id = Uuid::now_v7();
    let provider_connection_id = Uuid::now_v7();
    let execution_profile_id = Uuid::now_v7();
    let execution_worker_id = Uuid::now_v7();
    let channel_id = Uuid::now_v7();
    let thread_id = Uuid::now_v7();
    let run_id = Uuid::now_v7();
    let suffix = run_id.simple().to_string();
    let mut transaction = pool.begin().await.expect("active-run transaction");
    sqlx::query(
        "INSERT INTO actors (id, kind, owner_workspace_id) \
         SELECT $1, 'execution_worker', organization_id FROM projects WHERE id = $2",
    )
    .bind(worker_actor_id)
    .bind(project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert execution Worker Actor");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'worker', 'Archive guard Worker', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(worker_actor_id)
    .bind(format!("guard-{}", &suffix[..16]))
    .execute(&mut *transaction)
    .await
    .expect("insert Worker membership");
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, owner, name, clone_url, default_branch,
            mirror_path, trusted, preparation_state, prepared_at
        )
        VALUES ($1, $2, 'test', $3, 'https://example.invalid/test.git',
                'main', $4, true, 'ready', now())
        "#,
    )
    .bind(repository_id)
    .bind(project_id)
    .bind(format!("archive-{}", &suffix[..12]))
    .bind(format!("/tmp/archive-guard-{suffix}.git"))
    .execute(&mut *transaction)
    .await
    .expect("insert Repository");
    sqlx::query(
        r#"
        INSERT INTO provider_connections (
            id, project_id, kind, host, capabilities
        )
        VALUES ($1, $2, 'codex_app_server', 'local_project_host', '{}'::jsonb)
        "#,
    )
    .bind(provider_connection_id)
    .bind(project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert provider connection");
    crate::harness_support::insert_account_profile(
        &mut transaction,
        execution_profile_id,
        project_id,
        provider_connection_id,
        owner_actor_id,
        "archive-guard",
        &format!("/tmp/archive-guard-profile-{suffix}"),
        "available",
        "authenticated",
    )
    .await
    .expect("insert execution profile");
    sqlx::query(
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id,
            execution_profile_id
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(execution_worker_id)
    .bind(project_id)
    .bind(worker_actor_id)
    .bind(provider_connection_id)
    .bind(execution_profile_id)
    .execute(&mut *transaction)
    .await
    .expect("insert execution Worker");
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name)
        VALUES ($1, $2, 'channel', 'Archive guard')
        "#,
    )
    .bind(channel_id)
    .bind(project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Channel");
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, repository_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, 'Archive guard run')
        "#,
    )
    .bind(thread_id)
    .bind(project_id)
    .bind(channel_id)
    .bind(repository_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Thread");
    crate::support::builtin_run_record(
        &mut *transaction,
        run_id,
        thread_id,
        execution_worker_id,
        "working",
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'working',
                'Verify archive guard.', '{}'::jsonb)
        "#,
    )
    .bind(run_id)
    .bind(thread_id)
    .bind(execution_worker_id)
    .bind(owner_actor_id)
    .bind(execution_profile_id)
    .bind(repository_id)
    .execute(&mut *transaction)
    .await
    .expect("insert active Agent Run");
    transaction
        .commit()
        .await
        .expect("commit active-run fixture");
    run_id
}

fn test_app(pool: PgPool, database_url: &str) -> axum::Router {
    let values = BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url.to_owned(),
    )]);
    let config = AppConfig::from_values(&values).expect("test configuration");
    let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
        .expect("test supervisor registry");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    })
}

fn request(method: &str, uri: &str, headers: &[(&str, &str)], body: Value) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("serialize request body"))
    };
    builder.body(body).expect("build request")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("read response body"),
    )
    .expect("parse response JSON")
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |connection, _metadata| {
            let schema = Arc::clone(&schema);
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema.as_str())
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect schema-scoped HTTP pool")
}

/// P0.13: the caller's address comes from the configured header only.
/// With it set, one address runs out of sign-ins of its own; without it,
/// the same header means nothing and there's no per-address limit.
#[tokio::test]
async fn the_client_address_header_is_a_setting() {
    // The limits are counted in the database (K5.6).
    let Some(db) = crate::support::db::db("client_address_header").await else {
        return;
    };
    let database_url = db.url.clone();
    let pool = db.pool.clone();
    for (header, limited) in [(Some("x-client-address"), true), (None, false)] {
        let mut values = BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        )]);
        if let Some(header) = header {
            values.insert(
                "AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER".to_owned(),
                header.to_owned(),
            );
        }
        let config = AppConfig::from_values(&values).expect("test configuration");
        let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
            .expect("test supervisor registry");
        let app = router(AppState {
            config: Arc::new(config),
            pool: pool.clone(),
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(32),
            supervisors,
        });
        let mut statuses = Vec::new();
        for attempt in 0..21 {
            let response = app
                .clone()
                .oneshot(request(
                    "POST",
                    "/api/v1/auth/login",
                    &[
                        ("origin", "http://127.0.0.1:3100"),
                        ("x-client-address", "192.0.2.7"),
                    ],
                    json!({"loginName": format!("nobody-{attempt}"), "password": "short"}),
                ))
                .await
                .expect("login response");
            statuses.push(response.status());
        }
        let refused = statuses
            .iter()
            .filter(|status| **status == StatusCode::TOO_MANY_REQUESTS)
            .count();
        if limited {
            assert_eq!(refused, 1, "{statuses:?}");
            assert_eq!(statuses[20], StatusCode::TOO_MANY_REQUESTS);
        } else {
            assert_eq!(refused, 0, "{statuses:?}");
        }
    }
    db.finish().await;
}

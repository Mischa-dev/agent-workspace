//! Tasks at project scale over HTTP: planning fields, dependencies across
//! projects, keyset lists and boards, bulk changes, editable workflows and
//! field-level history.

use std::{collections::BTreeMap, sync::Arc, time::Instant};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
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
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const OWNER: &str = "w3-task-owner";
const MEMBER: &str = "w3-task-member";
const PASSWORD: &str = "correct horse battery staple";

struct Client {
    application: axum::Router,
    cookie: String,
    csrf: String,
}

impl Client {
    async fn login(application: &axum::Router, login_name: &str) -> Self {
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("origin", "http://127.0.0.1:3100")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(
                            &json!({ "loginName": login_name, "password": PASSWORD }),
                        )
                        .expect("login body"),
                    ))
                    .expect("login request"),
            )
            .await
            .expect("login response");
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
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
            .collect::<Vec<_>>()
            .join("; ");
        let csrf = cookie
            .split("; ")
            .find_map(|pair| pair.strip_prefix("agent_workspace_csrf="))
            .expect("CSRF cookie")
            .to_owned();
        Self {
            application: application.clone(),
            cookie,
            csrf,
        }
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.send("GET", uri, None).await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send("POST", uri, Some(body)).await
    }

    async fn patch(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send("PATCH", uri, Some(body)).await
    }

    async fn put(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send("PUT", uri, Some(body)).await
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &self.cookie);
        if body.is_some() {
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .header("origin", "http://127.0.0.1:3100")
                .header("x-csrf-token", &self.csrf)
                .header("idempotency-key", Uuid::now_v7().to_string());
        }
        let response = self
            .application
            .clone()
            .oneshot(
                request
                    .body(body.map_or_else(Body::empty, |body| {
                        Body::from(serde_json::to_vec(&body).expect("request body"))
                    }))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 10_000_000)
            .await
            .expect("response body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                panic!(
                    "JSON response to {method} {uri}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, value)
    }
}

struct Fixture {
    pool: PgPool,
    owner: Client,
    member: Client,
    project_id: Uuid,
    organization_id: Uuid,
    _temporary: tempfile::TempDir,
}

async fn fixture(name: &str) -> Option<Fixture> {
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL")
        .ok()
        .or_else(|| {
            eprintln!(
                "skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset"
            );
            None
        })?;
    let administrative = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("w3_{name}_{}", Uuid::now_v7().simple());
    administrative
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&database_url)
        .await
        .expect("connect schema pool");
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
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            "/usr/bin/false".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("configuration");
    let seed = dev_seed(&pool, &config).await.expect("seed");
    for (login_name, display_name, actor, admin) in [
        (OWNER, "Task Owner", seed.human_actor_id, true),
        (MEMBER, "Task Member", seed.collaborator_actor_id, false),
    ] {
        let command = BootstrapCommand {
            login_name: login_name.to_owned(),
            display_name: display_name.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor),
            organization_id: Some(seed.organization_id),
        };
        if admin {
            bootstrap_admin(&pool, command, PASSWORD.to_owned()).await
        } else {
            bootstrap_development_member(&pool, command, PASSWORD.to_owned()).await
        }
        .expect("bootstrap login");
    }
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors,
    });
    Some(Fixture {
        owner: Client::login(&application, OWNER).await,
        member: Client::login(&application, MEMBER).await,
        pool,
        project_id: seed.project_id,
        organization_id: seed.organization_id,
        _temporary: temporary,
    })
}

fn statuses(workflow: &Value) -> Vec<Value> {
    workflow["statuses"].as_array().expect("statuses").clone()
}

fn status_named(workflow: &Value, name: &str) -> String {
    statuses(workflow)
        .iter()
        .find(|status| status["name"] == name)
        .and_then(|status| status["id"].as_str())
        .unwrap_or_else(|| panic!("status {name}"))
        .to_owned()
}

async fn create_task(client: &Client, project_id: Uuid, body: Value) -> Value {
    let (status, task) = client
        .post(&format!("/api/v1/projects/{project_id}/work-items"), body)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    task
}

#[tokio::test]
async fn planning_fields_dependencies_and_history_work_over_http() {
    let Some(fixture) = fixture("planning").await else {
        return;
    };
    let project = fixture.project_id;
    let owner = &fixture.owner;
    let base = format!("/api/v1/projects/{project}");

    let (status, label) = owner
        .post(&format!("{base}/labels"), json!({ "name": "Backend" }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{label}");
    let (status, milestone) = owner
        .post(
            &format!("{base}/milestones"),
            json!({ "name": "Beta", "targetDate": "2026-12-01" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{milestone}");

    let design = create_task(
        owner,
        project,
        json!({
            "title": "Design the schema",
            "labelIds": [label["id"]],
            "milestoneId": milestone["id"],
            "startDate": "2026-10-20",
            "dueDate": "2026-10-30",
            "estimate": 3,
            "acceptanceCriteria": ["Reviewed by someone"]
        }),
    )
    .await;
    assert_eq!(design["milestone"]["name"], "Beta");
    assert_eq!(design["dueDate"], "2026-10-30");
    assert_eq!(design["estimate"], 3.0);
    assert_eq!(design["labels"][0]["name"], "Backend");
    assert_eq!(
        design["acceptanceCriteria"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(design["watching"], true);
    assert_eq!(design["estimateUnit"], "points");
    assert_eq!(design["ready"], true);

    let build = create_task(
        owner,
        project,
        json!({ "title": "Build the API", "parentWorkItemId": design["id"] }),
    )
    .await;
    assert_eq!(build["parent"]["key"], design["key"]);

    // Dates must be in order.
    let (status, _) = owner
        .patch(
            &format!(
                "{base}/work-items/{}",
                design["id"].as_str().expect("test value")
            ),
            json!({ "expectedVersion": design["version"], "startDate": "2026-11-05" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Build waits on design; the reverse link would close a cycle.
    let build_id = build["id"].as_str().expect("test value");
    let design_id = design["id"].as_str().expect("test value");
    let (status, linked) = owner
        .post(
            &format!("{base}/work-items/{build_id}/dependencies"),
            json!({ "expectedVersion": build["version"], "blockedBy": [design["key"]] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(linked["blockedBy"][0]["key"], design["key"]);
    assert_eq!(linked["ready"], false);
    let (_, design_now) = owner.get(&format!("{base}/work-items/{design_id}")).await;
    assert_eq!(design_now["blocks"][0]["key"], build["key"]);
    let (status, cycle) = owner
        .post(
            &format!("{base}/work-items/{design_id}/dependencies"),
            json!({ "expectedVersion": design_now["version"], "blockedBy": [build_id] }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{cycle}");
    assert_eq!(cycle["error"]["code"], "dependency_cycle");
    assert!(
        cycle["error"]["details"]["path"]
            .as_array()
            .is_some_and(|path| path.len() >= 2)
    );

    // Lists know which tasks are ready and which are blocked.
    let (status, ready) = owner.get(&format!("{base}/work-items?ready=true")).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let ready_keys = ready["items"]
        .as_array()
        .expect("test value")
        .iter()
        .map(|item| item["key"].clone())
        .collect::<Vec<_>>();
    assert!(ready_keys.contains(&design["key"]));
    assert!(!ready_keys.contains(&build["key"]));
    let (_, blocked) = owner.get(&format!("{base}/work-items?blocked=true")).await;
    assert_eq!(blocked["total"], 1);
    assert_eq!(blocked["items"][0]["openBlockerCount"], 1);

    // A field change is recorded before and after, with names.
    let (status, changed) = owner
        .patch(
            &format!("{base}/work-items/{design_id}"),
            json!({
                "expectedVersion": design_now["version"],
                "priority": "urgent",
                "dueDate": null,
                "blockedReason": "Waiting on a decision"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["blocked"]["reason"], "Waiting on a decision");
    assert_eq!(changed["dueDate"], Value::Null);
    let (_, timeline) = owner
        .get(&format!("{base}/work-items/{design_id}/timeline"))
        .await;
    let changes = timeline["entries"]
        .as_array()
        .expect("test value")
        .iter()
        .find_map(|entry| entry["activity"]["payload"].get("changes"))
        .expect("an entry with field changes")
        .clone();
    assert_eq!(changes["priority"]["from"], "no_priority");
    assert_eq!(changes["priority"]["to"], "urgent");
    assert_eq!(changes["dueDate"]["from"], "2026-10-30");
    assert_eq!(changes["dueDate"]["to"], Value::Null);

    // Watching and unwatching.
    let (status, watch) = fixture
        .member
        .post(
            &format!("{base}/work-items/{design_id}/watch"),
            json!({ "watching": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{watch}");
    assert_eq!(watch["watching"], true);
    assert_eq!(watch["watchers"].as_array().map(Vec::len), Some(2));

    // Milestone progress counts its tasks.
    let (_, milestones) = owner.get(&format!("{base}/milestones")).await;
    assert_eq!(milestones["milestones"][0]["total"], 1);

    // A task with two labels shows in both label columns.
    let (_, frontend) = owner
        .post(&format!("{base}/labels"), json!({ "name": "Frontend" }))
        .await;
    let both = create_task(
        owner,
        project,
        json!({ "title": "Full stack", "labelIds": [label["id"], frontend["id"]] }),
    )
    .await;
    let (status, board) = owner
        .get(&format!("{base}/work-items/board?columnsBy=label"))
        .await;
    assert_eq!(status, StatusCode::OK, "{board}");
    for name in ["Backend", "Frontend"] {
        let column = board["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .find(|column| column["label"] == name)
            .expect("label column");
        assert!(
            column["items"]
                .as_array()
                .expect("items")
                .iter()
                .any(|item| item["id"] == both["id"]),
            "{name} column lost the task"
        );
    }

    // Filtered to Backend, the Frontend column holds only tasks with both.
    let frontend_only = create_task(
        owner,
        project,
        json!({ "title": "Styles", "labelIds": [frontend["id"]] }),
    )
    .await;
    let backend_id = label["id"].as_str().expect("label id");
    let (status, board) = owner
        .get(&format!(
            "{base}/work-items/board?columnsBy=label&labelIds={backend_id}"
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{board}");
    let frontend_column = board["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .find(|column| column["label"] == "Frontend")
        .expect("frontend column");
    let ids = frontend_column["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["id"].clone())
        .collect::<Vec<_>>();
    assert!(ids.contains(&both["id"]), "{board}");
    assert!(!ids.contains(&frontend_only["id"]), "{board}");

    // Every priority is a column to drop on, even one no task has yet.
    let (status, board) = owner
        .get(&format!("{base}/work-items/board?columnsBy=priority"))
        .await;
    assert_eq!(status, StatusCode::OK, "{board}");
    let keys = board["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|column| column["key"].as_str().expect("key").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        ["urgent", "high", "medium", "low", "no_priority"],
        "{board}"
    );

    // An archived milestone keeps its column for the tasks still in it, but
    // nothing can be dropped there.
    let milestone_id = milestone["id"].as_str().expect("milestone id");
    let (status, archived) = owner
        .patch(
            &format!("{base}/milestones/{milestone_id}"),
            json!({ "expectedVersion": milestone["version"], "archived": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    let (status, board) = owner
        .get(&format!("{base}/work-items/board?columnsBy=milestone"))
        .await;
    assert_eq!(status, StatusCode::OK, "{board}");
    let droppable = |key: &str| {
        board["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .find(|column| column["key"] == key)
            .map(|column| column["droppable"].clone())
    };
    assert_eq!(droppable(milestone_id), Some(json!(false)), "{board}");
    assert_eq!(droppable("none"), Some(json!(true)), "{board}");
}

#[tokio::test]
async fn links_to_other_projects_respect_membership() {
    let Some(fixture) = fixture("visibility").await else {
        return;
    };
    let project = fixture.project_id;
    let owner = &fixture.owner;
    let member = &fixture.member;
    // A second project in the workspace that only the owner belongs to.
    let (status, side) = owner
        .post(
            "/api/v1/projects",
            json!({ "organizationId": fixture.organization_id, "key": "SIDE", "name": "Side" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{side}");
    let side_id: Uuid = side["id"]
        .as_str()
        .expect("test value")
        .parse()
        .expect("test value");
    let secret = create_task(owner, side_id, json!({ "title": "Private groundwork" })).await;
    let main = create_task(owner, project, json!({ "title": "Ship it" })).await;
    let main_id = main["id"].as_str().expect("test value");
    let base = format!("/api/v1/projects/{project}");

    // The member can't link to a task they can't see, by key or by ID.
    for reference in [&secret["key"], &secret["id"]] {
        let (status, refused) = member
            .post(
                &format!("{base}/work-items/{main_id}/dependencies"),
                json!({ "expectedVersion": main["version"], "blockedBy": [reference] }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    }

    // The owner can; the member then sees that something blocks the task,
    // but not what.
    let (status, linked) = owner
        .post(
            &format!("{base}/work-items/{main_id}/dependencies"),
            json!({ "expectedVersion": main["version"], "blockedBy": [secret["key"]] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(linked["blockedBy"][0]["key"], "SIDE-1");
    let (_, seen) = member.get(&format!("{base}/work-items/{main_id}")).await;
    assert_eq!(seen["blockedBy"], json!([]));
    assert_eq!(seen["ready"], false);

    // Refusals don't name the hidden task either: not as the original of a
    // duplicate, nor along a dependency cycle.
    let other = create_task(owner, project, json!({ "title": "Ship it again" })).await;
    let other_id = other["id"].as_str().expect("test value");
    let (status, duplicate) = owner
        .post(
            &format!("{base}/work-items/{main_id}/relations"),
            json!({ "expectedVersion": linked["version"], "kind": "duplicates", "target": secret["key"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{duplicate}");
    let (status, refused) = member
        .post(
            &format!("{base}/work-items/{main_id}/relations"),
            json!({ "expectedVersion": duplicate["version"], "kind": "duplicates", "target": other["key"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(!refused.to_string().contains("SIDE-"), "{refused}");
    let secret_id = secret["id"].as_str().expect("test value");
    let (_, secret_now) = owner
        .get(&format!(
            "/api/v1/projects/{side_id}/work-items/{secret_id}"
        ))
        .await;
    let (status, closed_loop) = owner
        .post(
            &format!("/api/v1/projects/{side_id}/work-items/{secret_id}/dependencies"),
            json!({ "expectedVersion": secret_now["version"], "blockedBy": [other["key"]] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{closed_loop}");
    let (_, other_now) = member.get(&format!("{base}/work-items/{other_id}")).await;
    let (status, refused) = member
        .post(
            &format!("{base}/work-items/{other_id}/dependencies"),
            json!({ "expectedVersion": other_now["version"], "blockedBy": [main["key"]] }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(!refused.to_string().contains("SIDE-"), "{refused}");
    assert!(
        refused["error"]["details"]["path"]
            .as_array()
            .expect("test value")
            .contains(&json!("a task in another project")),
        "{refused}"
    );

    // Nor can the member read the side project's list or labels.
    let (status, _) = member
        .get(&format!("/api/v1/projects/{side_id}/work-items"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = member
        .get(&format!("/api/v1/projects/{side_id}/labels"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bulk_changes_are_all_or_nothing() {
    let Some(fixture) = fixture("bulk").await else {
        return;
    };
    let project = fixture.project_id;
    let owner = &fixture.owner;
    let base = format!("/api/v1/projects/{project}");
    let first = create_task(owner, project, json!({ "title": "First" })).await;
    let second = create_task(owner, project, json!({ "title": "Second" })).await;

    let (status, refused) = owner
        .post(
            &format!("{base}/work-items/bulk"),
            json!({
                "items": [
                    { "id": first["id"], "expectedVersion": first["version"] },
                    { "id": second["id"], "expectedVersion": 99 }
                ],
                "changes": { "priority": "high" }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"]["code"], "bulk_rejected");
    assert_eq!(
        refused["error"]["details"]["problems"][0]["id"],
        second["id"]
    );
    let (_, unchanged) = owner
        .get(&format!(
            "{base}/work-items/{}",
            first["id"].as_str().expect("test value")
        ))
        .await;
    assert_eq!(unchanged["priority"], "no_priority");

    let (status, applied) = owner
        .post(
            &format!("{base}/work-items/bulk"),
            json!({
                "items": [
                    { "id": first["id"], "expectedVersion": first["version"] },
                    { "id": second["id"], "expectedVersion": second["version"] }
                ],
                "changes": { "priority": "high", "dueDate": "2026-11-01" }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["items"][0]["id"], first["id"]);
    assert!(
        applied["items"]
            .as_array()
            .expect("test value")
            .iter()
            .all(|item| item["priority"] == "high" && item["dueDate"] == "2026-11-01")
    );

    // A viewer-level change can't be smuggled through bulk.
    let (status, _) = fixture
        .member
        .post(
            &format!("{base}/work-items/bulk"),
            json!({
                "items": [{ "id": first["id"], "expectedVersion": 2 }],
                "changes": { "archived": true, "priority": "low" }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn owners_edit_the_workflow_and_retired_statuses_move_their_tasks() {
    let Some(fixture) = fixture("workflow").await else {
        return;
    };
    let project = fixture.project_id;
    let owner = &fixture.owner;
    let uri = format!("/api/v1/projects/{project}/work-item-workflow");
    let (_, workflow) = owner.get(&uri).await;
    assert_eq!(workflow["canEdit"], true);
    let (_, member_view) = fixture.member.get(&uri).await;
    assert_eq!(member_view["canEdit"], false);

    // Add a QA status between Review and Verification, open to every move.
    let mut drafts = statuses(&workflow)
        .iter()
        .map(|status| {
            json!({
                "id": status["id"], "name": status["name"],
                "semanticCategory": status["semanticCategory"], "isDefault": status["isDefault"]
            })
        })
        .collect::<Vec<_>>();
    let review = drafts
        .iter()
        .position(|status| status["name"] == "Review")
        .expect("test value");
    drafts.insert(
        review + 1,
        json!({ "key": "qa", "name": "QA", "semanticCategory": "review", "isDefault": false }),
    );
    let types = workflow["types"]
        .as_array()
        .expect("test value")
        .iter()
        .map(|kind| json!({ "id": kind["id"], "name": kind["name"], "isDefault": kind["isDefault"] }))
        .collect::<Vec<_>>();
    let document = json!({
        "expectedVersion": workflow["version"],
        "statuses": drafts,
        "types": types,
        "transitions": "all"
    });
    let (status, refused) = fixture.member.put(&uri, document.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, updated) = owner.put(&uri, document).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let qa = status_named(&updated, "QA");
    assert_eq!(
        updated["version"],
        workflow["version"].as_i64().expect("test value") + 1
    );

    // A task goes to QA, then QA is retired and its task moves to Review.
    let task = create_task(owner, project, json!({ "title": "Check the release" })).await;
    let task_id = task["id"].as_str().expect("test value");
    let (status, moved) = owner
        .patch(
            &format!("/api/v1/projects/{project}/work-items/{task_id}"),
            json!({ "expectedVersion": task["version"], "statusId": qa }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    // An archived task in QA moves too, so restoring it later works.
    let shelved = create_task(owner, project, json!({ "title": "Old check" })).await;
    let shelved_id = shelved["id"].as_str().expect("test value");
    let (_, shelved) = owner
        .patch(
            &format!("/api/v1/projects/{project}/work-items/{shelved_id}"),
            json!({ "expectedVersion": shelved["version"], "statusId": qa }),
        )
        .await;
    let (status, _) = owner
        .post(
            &format!("/api/v1/projects/{project}/work-items/{shelved_id}/archive"),
            json!({ "expectedVersion": shelved["version"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let without_qa = statuses(&updated)
        .iter()
        .filter(|status| status["name"] != "QA")
        .map(|status| {
            json!({
                "id": status["id"], "name": status["name"],
                "semanticCategory": status["semanticCategory"], "isDefault": status["isDefault"]
            })
        })
        .collect::<Vec<_>>();
    let retire = |replacements: Value| {
        json!({
            "expectedVersion": updated["version"],
            "statuses": without_qa,
            "types": types,
            "transitions": "all",
            "replacements": replacements
        })
    };
    let (status, refused) = owner.put(&uri, retire(json!({}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let review_id = status_named(&updated, "Review");
    let (status, retired) = owner
        .put(&uri, retire(json!({ qa.clone(): review_id })))
        .await;
    assert_eq!(status, StatusCode::OK, "{retired}");
    let (_, task_now) = owner
        .get(&format!("/api/v1/projects/{project}/work-items/{task_id}"))
        .await;
    assert_eq!(task_now["status"]["name"], "Review");
    let (_, shelved_now) = owner
        .get(&format!(
            "/api/v1/projects/{project}/work-items/{shelved_id}"
        ))
        .await;
    assert_eq!(shelved_now["status"]["name"], "Review");

    // The estimate unit can't change under tasks that carry estimates.
    create_task(owner, project, json!({ "title": "Sized", "estimate": 8 })).await;
    let (_, current) = owner.get(&uri).await;
    let (status, refused) = owner
        .put(
            &uri,
            json!({
                "expectedVersion": current["version"],
                "statuses": without_qa,
                "types": types,
                "transitions": "all",
                "estimateUnit": "hours"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(
        refused["error"]["details"]["field"], "estimateUnit",
        "{refused}"
    );
}

#[tokio::test]
async fn lists_page_every_sort_and_grouping_quickly_at_ten_thousand_tasks() {
    let Some(fixture) = fixture("scale").await else {
        return;
    };
    let project = fixture.project_id;
    let owner = &fixture.owner;
    let base = format!("/api/v1/projects/{project}/work-items");
    let first = create_task(owner, project, json!({ "title": "Seed task" })).await;
    let label = owner
        .post(
            &format!("/api/v1/projects/{project}/labels"),
            json!({ "name": "Ops" }),
        )
        .await
        .1;

    // 10,000 more tasks straight into the table, like a long-lived project.
    sqlx::query(
        r#"
        INSERT INTO work_items (id, project_id, number, title, description, type_id, status_id,
                                priority, assignee_actor_id, created_by_actor_id, board_rank,
                                due_date, created_at, updated_at)
        SELECT gen_random_uuid(), seed.project_id, seed.number + n,
               'Task ' || n || CASE WHEN n % 97 = 0 THEN ' login bug' ELSE '' END,
               '', seed.type_id,
               (SELECT id FROM work_item_statuses s WHERE s.project_id = seed.project_id
                 ORDER BY s.position OFFSET (n % 7) LIMIT 1),
               (ARRAY['no_priority', 'low', 'medium', 'high', 'urgent'])[1 + n % 5],
               CASE WHEN n % 3 = 0 THEN seed.created_by_actor_id END,
               seed.created_by_actor_id, 1000 + n,
               CASE WHEN n % 4 = 0 THEN DATE '2026-10-01' + (n % 90) END,
               now() - make_interval(secs => n), now() - make_interval(secs => n)
        FROM work_items seed, generate_series(1, 10000) n
        WHERE seed.id = $1
        "#,
    )
    .bind(Uuid::parse_str(first["id"].as_str().expect("test value")).expect("test value"))
    .execute(&fixture.pool)
    .await
    .expect("insert 10,000 tasks");
    sqlx::query(
        r#"
        INSERT INTO work_item_labels (project_id, work_item_id, label_id, added_by_actor_id)
        SELECT item.project_id, item.id, $2, item.created_by_actor_id
        FROM work_items item WHERE item.project_id = $1 AND item.number % 5 = 0
        "#,
    )
    .bind(project)
    .bind(Uuid::parse_str(label["id"].as_str().expect("test value")).expect("test value"))
    .execute(&fixture.pool)
    .await
    .expect("label every fifth task");
    sqlx::query("ANALYZE work_items")
        .execute(&fixture.pool)
        .await
        .expect("analyze");

    // Every sort and grouping pages through without repeats, in order.
    for query in [
        "sort=newest",
        "sort=due&groupBy=assignee",
        "sort=priority&groupBy=status",
        "sort=title&groupBy=label&includeDone=false",
        "sort=updated&groupBy=milestone",
    ] {
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        let mut rows = 0;
        for _ in 0..3 {
            let uri = match &cursor {
                Some(cursor) => format!("{base}?{query}&limit=200&cursor={cursor}"),
                None => format!("{base}?{query}&limit=200"),
            };
            let (status, page) = owner.get(&uri).await;
            assert_eq!(status, StatusCode::OK, "{query}: {page}");
            for item in page["items"].as_array().expect("test value") {
                let group = item["group"]["key"].as_str().unwrap_or("");
                assert!(
                    seen.insert(format!("{group}:{}", item["id"])),
                    "{query} repeated a row"
                );
                rows += 1;
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
        }
        assert_eq!(rows, 600, "{query}");
    }

    // A cursor is refused under other filters.
    let (_, page) = owner.get(&format!("{base}?sort=due&limit=10")).await;
    let cursor = page["nextCursor"].as_str().expect("test value");
    let (status, _) = owner
        .get(&format!("{base}?sort=title&limit=10&cursor={cursor}"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Search, groups and the board.
    let (_, found) = owner.get(&format!("{base}?q=login%20bug")).await;
    assert_eq!(found["total"], 103);
    let (status, groups) = owner.get(&format!("{base}/groups?groupBy=label")).await;
    assert_eq!(status, StatusCode::OK, "{groups}");
    assert_eq!(groups["groups"][0]["label"], "Ops");
    assert_eq!(groups["groups"][0]["count"], 2000);
    let (status, board) = owner
        .get(&format!("{base}/board?columnsBy=priority&limit=20"))
        .await;
    assert_eq!(status, StatusCode::OK, "{board}");
    let column = &board["columns"][0];
    assert_eq!(column["key"], "urgent");
    assert_eq!(column["items"].as_array().map(Vec::len), Some(20));
    let mut next = String::new();
    for (key, value) in column["filter"].as_object().expect("test value") {
        next.push_str(&format!("{key}={}&", value.as_str().expect("test value")));
    }
    let (status, more) = owner
        .get(&format!(
            "{base}?{next}limit=20&cursor={}",
            column["nextCursor"].as_str().expect("test value")
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{more}");
    assert_eq!(more["items"][0]["priority"], "urgent");

    // Timing: the slowest of a few list requests, with the server in-process.
    let mut slowest = 0;
    for query in [
        "sort=newest&limit=50",
        "sort=due&groupBy=assignee&limit=50",
        "q=login&limit=50",
        "blocked=false&includeDone=false&sort=priority&limit=50",
    ] {
        let started = Instant::now();
        let (status, _) = owner.get(&format!("{base}?{query}")).await;
        assert_eq!(status, StatusCode::OK);
        slowest = slowest.max(started.elapsed().as_millis());
    }
    eprintln!("slowest list request at 10,000 tasks: {slowest} ms");
    // The target is 200 ms on a release build; debug builds and shared CI
    // machines get headroom.
    assert!(slowest < 1_000, "slowest list request took {slowest} ms");
}

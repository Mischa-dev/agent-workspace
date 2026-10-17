//! Views and data (W11) over HTTP: saved views stay private unless shared,
//! recurring tasks make one task per occurrence and pause when the person
//! they act as can no longer create tasks, imports create each row once,
//! and a new project can start from a saved setup.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
    config::AppConfig,
    jobs::{self, NewJob, WorkerOptions},
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

const OWNER: &str = "w11-owner";
const MEMBER: &str = "w11-member";
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
    state: AppState,
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
    let schema = format!("w11_{name}_{}", Uuid::now_v7().simple());
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
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors,
    };
    let application = router(state.clone());
    Some(Fixture {
        state,
        owner: Client::login(&application, OWNER).await,
        member: Client::login(&application, MEMBER).await,
        pool,
        project_id: seed.project_id,
        organization_id: seed.organization_id,
        _temporary: temporary,
    })
}

async fn run_jobs(fixture: &Fixture) {
    jobs::run_due(
        &fixture.state,
        &jobs::default_registry(),
        &WorkerOptions {
            concurrency: 1,
            poll_interval: Duration::from_millis(20),
            lease: Duration::from_secs(30),
            worker_id: "w11-test".to_owned(),
        },
    )
    .await
    .expect("run jobs");
}

async fn sweep_recurring(fixture: &Fixture) {
    jobs::enqueue(&fixture.pool, NewJob::new("tasks.recurring_sweep"))
        .await
        .expect("queue sweep");
    run_jobs(fixture).await;
}

fn views_named<'a>(list: &'a Value, name: &str) -> Vec<&'a Value> {
    list["views"]
        .as_array()
        .expect("views")
        .iter()
        .filter(|view| view["name"] == name)
        .collect()
}

#[tokio::test]
async fn saved_views_stay_private_unless_shared() {
    let Some(fixture) = fixture("views").await else {
        return;
    };
    let base = format!("/api/v1/projects/{}", fixture.project_id);
    let (status, list) = fixture.owner.get(&format!("{base}/views")).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ready = views_named(&list, "Ready for agents");
    assert_eq!(ready.len(), 1, "{list}");
    assert_eq!(ready[0]["pinned"], true);
    assert!(
        ready[0]["href"]
            .as_str()
            .expect("present")
            .contains("ready=true")
    );
    assert_eq!(ready[0]["canDelete"], false);

    let (status, mine) = fixture
        .member
        .post(
            &format!("{base}/views"),
            json!({"name": "My bugs", "query": {"assigneeActorId": "me", "priority": "high"}, "fields": ["key", "status"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{mine}");
    assert_eq!(mine["shared"], false);
    // "me" is kept as written and read as whoever looks at the view.
    assert_eq!(mine["query"]["assigneeActorId"], "me");
    assert!(
        mine["resolvedQuery"]["assigneeActorId"]
            .as_str()
            .and_then(|id| id.parse::<Uuid>().ok())
            .is_some(),
        "{mine}"
    );
    let view_id = mine["id"].as_str().expect("view id").to_owned();

    // Someone else's personal view is invisible and can't be changed.
    let (_, owners) = fixture.owner.get(&format!("{base}/views")).await;
    assert!(views_named(&owners, "My bugs").is_empty(), "{owners}");
    let (status, _) = fixture
        .owner
        .patch(
            &format!("{base}/views/{view_id}"),
            json!({"expectedVersion": 1, "name": "Taken"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A query the list would refuse is refused when saved.
    let (status, refused) = fixture
        .member
        .post(
            &format!("{base}/views"),
            json!({"name": "Broken", "query": {"cursor": "abc"}}),
        )
        .await;
    assert!(status.is_client_error(), "{refused}");

    // Built-ins can be pinned and unpinned per person but not deleted.
    let blocked = views_named(&owners, "Blocked")[0]["id"]
        .as_str()
        .expect("blocked id")
        .to_owned();
    let (status, pinned) = fixture
        .member
        .post(
            &format!("{base}/views/{blocked}/pin"),
            json!({"pinned": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{pinned}");
    let (_, member_list) = fixture.member.get(&format!("{base}/views")).await;
    assert_eq!(views_named(&member_list, "Blocked")[0]["pinned"], true);
    let (_, owner_list) = fixture.owner.get(&format!("{base}/views")).await;
    assert_eq!(views_named(&owner_list, "Blocked")[0]["pinned"], false);
    let (status, _) = fixture
        .owner
        .send(
            "DELETE",
            &format!("{base}/views/{blocked}"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, work) = fixture.owner.get("/api/v1/my-work").await;
    assert_eq!(status, StatusCode::OK, "{work}");
    for section in ["assigned", "claimed", "review"] {
        assert!(
            work[section].is_object() || work[section].is_array(),
            "{work}"
        );
    }
    let (status, timeline) = fixture.owner.get(&format!("{base}/timeline")).await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    assert_eq!(timeline["suggestedDependencies"], json!([]));
}

#[tokio::test]
async fn a_recurring_task_runs_once_per_occurrence_and_pauses_when_its_owner_leaves() {
    let Some(fixture) = fixture("recurring").await else {
        return;
    };
    let base = format!("/api/v1/projects/{}", fixture.project_id);
    let (status, template) = fixture
        .member
        .post(
            &format!("{base}/task-templates"),
            json!({
                "name": "Weekly review",
                "title": "Review the week",
                "labels": ["ritual"],
                "acceptanceCriteria": ["Notes posted"],
                "subtasks": [{"title": "Collect numbers"}]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{template}");
    let template_id = template["id"].as_str().expect("template").to_owned();
    let (status, recurring) = fixture
        .member
        .post(
            &format!("{base}/recurring-tasks"),
            json!({"templateId": template_id, "frequency": "weekly", "weekdays": [1, 4], "timezone": "Europe/Amsterdam"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{recurring}");
    let recurring_id = recurring["id"]
        .as_str()
        .expect("recurring")
        .parse::<Uuid>()
        .expect("valid");
    assert!(recurring["nextRunAt"].is_string(), "{recurring}");

    let count_made = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM work_items WHERE project_id = $1 AND title = 'Review the week'",
        )
        .bind(fixture.project_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("count")
    };
    let make_due = || async {
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "UPDATE recurring_tasks SET next_run_at = now() - interval '1 minute' WHERE id = $1 RETURNING next_run_at",
        )
        .bind(recurring_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("due")
    };

    let due = make_due().await;
    sweep_recurring(&fixture).await;
    assert_eq!(count_made().await, 1);
    let (next, subtasks) = sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>, i64)>(
        r#"
        SELECT recurring.next_run_at,
               (SELECT count(*) FROM work_items child WHERE child.parent_work_item_id = recurring.last_work_item_id)
        FROM recurring_tasks recurring WHERE recurring.id = $1
        "#,
    )
    .bind(recurring_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("after run");
    assert!(next.expect("next run") > chrono::Utc::now());
    assert_eq!(subtasks, 1);

    // The same occurrence again (a job delivered twice) makes nothing new.
    sqlx::query("UPDATE recurring_tasks SET next_run_at = $2 WHERE id = $1")
        .bind(recurring_id)
        .bind(due)
        .execute(&fixture.pool)
        .await
        .expect("rewind");
    sweep_recurring(&fixture).await;
    assert_eq!(count_made().await, 1);

    // Once its owner can't create tasks here, it pauses instead of acting.
    sqlx::query(
        r#"
        UPDATE project_memberships SET enabled = false
        WHERE project_id = $1 AND actor_id = (SELECT created_by_actor_id FROM recurring_tasks WHERE id = $2)
        "#,
    )
    .bind(fixture.project_id)
    .bind(recurring_id)
    .execute(&fixture.pool)
    .await
    .expect("remove owner");
    make_due().await;
    sweep_recurring(&fixture).await;
    assert_eq!(count_made().await, 1);
    let (enabled, reason) = sqlx::query_as::<_, (bool, Option<String>)>(
        "SELECT enabled, paused_reason FROM recurring_tasks WHERE id = $1",
    )
    .bind(recurring_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("paused");
    assert!(!enabled);
    assert!(reason.unwrap_or_default().contains("no longer"));
}

const LINEAR_CSV: &str = "ID,Title,Description,Status,Priority,Labels,Parent issue,Due Date\n\
ENG-1,Ship the importer,\"Read CSV, Markdown and issues\",Todo,High,\"Imports, Backend\",,2026-11-02\n\
ENG-2,Write the parser,,Done,Medium,Backend,ENG-1,\n\
ENG-3,,No title here,Todo,Low,,,\n";

#[tokio::test]
async fn an_import_creates_each_row_once_and_exports_back() {
    let Some(fixture) = fixture("imports").await else {
        return;
    };
    let base = format!("/api/v1/projects/{}", fixture.project_id);
    let (status, import) = fixture
        .owner
        .post(
            &format!("{base}/imports"),
            json!({"source": "csv", "text": LINEAR_CSV, "fileName": "linear.csv"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{import}");
    assert_eq!(import["preview"]["willCreate"], 2, "{import}");
    assert_eq!(import["preview"]["withoutTitle"], 1, "{import}");
    assert_eq!(import["mapping"]["title"], "Title");
    let id = import["id"].as_str().expect("import id").to_owned();

    // Changing the mapping reads the text again.
    let (status, remapped) = fixture
        .owner
        .patch(
            &format!("{base}/imports/{id}"),
            json!({"expectedVersion": import["version"], "mapping": {"title": "Description", "externalId": "ID"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{remapped}");
    assert_eq!(remapped["preview"]["willCreate"], 2, "{remapped}");
    let (status, restored) = fixture
        .owner
        .patch(
            &format!("{base}/imports/{id}"),
            json!({"expectedVersion": remapped["version"], "mapping": import["mapping"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restored}");

    let (status, committed) = fixture
        .owner
        .post(
            &format!("{base}/imports/{id}/commit"),
            json!({"expectedVersion": restored["version"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert_eq!(committed["status"], "queued");
    run_jobs(&fixture).await;
    let (_, done) = fixture.owner.get(&format!("{base}/imports/{id}")).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["createdCount"], 2, "{done}");
    assert_eq!(done["skippedCount"], 1, "{done}");

    let (parent_category, child_parent, labels) = sqlx::query_as::<_, (String, bool, i64)>(
        r#"
        SELECT status.semantic_category,
               child.parent_work_item_id = parent.id,
               (SELECT count(*) FROM work_item_active_labels label
                 WHERE label.project_id = parent.project_id AND label.work_item_id = parent.id)
        FROM work_items parent
        JOIN work_items child ON child.project_id = parent.project_id AND child.title = 'Write the parser'
        JOIN work_item_statuses status ON status.project_id = child.project_id AND status.id = child.status_id
        WHERE parent.project_id = $1 AND parent.title = 'Ship the importer'
        "#,
    )
    .bind(fixture.project_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("imported tasks");
    assert_eq!(parent_category, "completed");
    assert!(child_parent);
    assert_eq!(labels, 2);

    // The same file again: everything is already here.
    let (status, again) = fixture
        .owner
        .post(
            &format!("{base}/imports"),
            json!({"source": "csv", "text": LINEAR_CSV}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{again}");
    assert_eq!(again["preview"]["willCreate"], 0, "{again}");
    assert_eq!(again["preview"]["alreadyImported"], 2, "{again}");

    // Markdown: list items, subtasks and headings as milestones.
    let (status, markdown) = fixture
        .owner
        .post(
            &format!("{base}/imports"),
            json!({"source": "markdown", "text": "# Beta\n- [ ] Invite testers #beta\n  - [ ] Write the email\n", "options": {"headingsAsMilestones": true, "createLabels": true, "createMilestones": true}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{markdown}");
    assert_eq!(
        markdown["preview"]["newMilestones"],
        json!(["Beta"]),
        "{markdown}"
    );

    let csv = export_text(&fixture, &format!("{base}/export?format=csv")).await;
    assert!(csv.contains("Ship the importer"), "{csv}");
    assert!(csv.starts_with('\u{feff}'));
}

async fn export_text(fixture: &Fixture, uri: &str) -> String {
    let response = fixture
        .owner
        .application
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header(header::COOKIE, &fixture.owner.cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .expect("valid")
            .contains("attachment")
    );
    String::from_utf8(
        to_bytes(response.into_body(), 10_000_000)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("utf-8")
}

#[tokio::test]
async fn a_new_project_starts_from_a_saved_setup() {
    let Some(fixture) = fixture("project_templates").await else {
        return;
    };
    let base = format!("/api/v1/projects/{}", fixture.project_id);
    let (status, label) = fixture
        .owner
        .post(
            &format!("{base}/labels"),
            json!({"name": "Customer", "color": "#aa3366"}),
        )
        .await;
    assert!(status.is_success(), "{label}");
    let (status, template) = fixture
        .owner
        .post(
            &format!("{base}/task-templates"),
            json!({"name": "Bug report", "title": "Bug: ", "labels": ["Customer"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{template}");

    // Only owners and admins can save a project's setup.
    let (status, _) = fixture
        .member
        .post(
            &format!("{base}/save-as-template"),
            json!({"name": "Member's setup"}),
        )
        .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
    let (status, saved) = fixture
        .owner
        .post(
            &format!("{base}/save-as-template"),
            json!({"name": "Product setup", "description": "Statuses, labels and bug template"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["contains"]["taskTemplates"], 1, "{saved}");
    let template_id = saved["id"].as_str().expect("template").to_owned();

    let (status, list) = fixture
        .member
        .get(&format!(
            "/api/v1/organizations/{}/project-templates",
            fixture.organization_id
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["templates"][0]["name"], "Product setup");
    assert_eq!(list["templates"][0]["canEdit"], false);

    let (status, project) = fixture
        .owner
        .post(
            "/api/v1/projects",
            json!({"organizationId": fixture.organization_id, "key": "NEXT", "name": "Next product", "templateId": template_id}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let new_project = project["id"].as_str().expect("project id").to_owned();
    let (status, templates) = fixture
        .owner
        .get(&format!("/api/v1/projects/{new_project}/task-templates"))
        .await;
    assert_eq!(status, StatusCode::OK, "{templates}");
    assert_eq!(templates["templates"][0]["name"], "Bug report");
    let color = sqlx::query_scalar::<_, String>(
        "SELECT color FROM project_labels WHERE project_id = $1::uuid AND name = 'Customer'",
    )
    .bind(&new_project)
    .fetch_one(&fixture.pool)
    .await
    .expect("label copied");
    assert_eq!(color, "#aa3366");
    let statuses = |project: String| {
        let pool = fixture.pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT name FROM work_item_statuses WHERE project_id = $1::uuid AND enabled ORDER BY position",
            )
            .bind(project)
            .fetch_all(&pool)
            .await
            .expect("statuses")
        }
    };
    assert_eq!(
        statuses(new_project.clone()).await,
        statuses(fixture.project_id.to_string()).await
    );

    // An unknown or archived template refuses the project.
    let (status, _) = fixture
        .owner
        .patch(
            &format!(
                "/api/v1/organizations/{}/project-templates/{template_id}",
                fixture.organization_id
            ),
            json!({"expectedVersion": 1, "archived": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = fixture
        .owner
        .post(
            "/api/v1/projects",
            json!({"organizationId": fixture.organization_id, "key": "LATER", "name": "Later", "templateId": template_id}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

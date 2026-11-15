//! AI planning (roadmap W10) against the fake provider: suggestions found,
//! accepted and dismissed, readiness and the graph, private sources that
//! must never reach someone who can't see them, the budget, automatic
//! apply stopping for someone who lost access, and a plan import that
//! resumes without creating anything twice.

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    ai::{
        detect::{self, Mode},
        plans::{self, Step},
    },
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
    config::AppConfig,
    jobs::{self, WorkerOptions},
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

const OWNER: &str = "w10-owner";
const MEMBER: &str = "w10-member";
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
        let bytes = to_bytes(response.into_body(), 20_000_000)
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
    state: AppState,
    owner: Client,
    member: Client,
    owner_actor: Uuid,
    member_actor: Uuid,
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
    let schema = format!("w10_{name}_{}", Uuid::now_v7().simple());
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
        (OWNER, "Plan Owner", seed.human_actor_id, true),
        (MEMBER, "Plan Member", seed.collaborator_actor_id, false),
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
        owner: Client::login(&application, OWNER).await,
        member: Client::login(&application, MEMBER).await,
        state,
        pool,
        owner_actor: seed.human_actor_id,
        member_actor: seed.collaborator_actor_id,
        project_id: seed.project_id,
        organization_id: seed.organization_id,
        _temporary: temporary,
    })
}

impl Fixture {
    fn base(&self) -> String {
        format!("/api/v1/projects/{}", self.project_id)
    }

    /// Picks the fake provider for the whole workspace.
    async fn use_fake_provider(&self) {
        self.set_workspace("ai.provider", json!("fake")).await;
    }

    async fn set_workspace(&self, key: &str, value: Value) {
        let (status, body) = self
            .owner
            .put(
                &format!("/api/v1/settings/values/{key}"),
                json!({"scope": "workspace", "organizationId": self.organization_id, "value": value}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{key}: {body}");
    }

    async fn set_project(&self, key: &str, value: Value) {
        let (status, body) = self
            .owner
            .put(
                &format!("/api/v1/settings/values/{key}"),
                json!({"scope": "project", "projectId": self.project_id, "value": value}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{key}: {body}");
    }

    async fn task(&self, title: &str, description: &str) -> Value {
        let (status, task) = self
            .owner
            .post(
                &format!("{}/work-items", self.base()),
                json!({"title": title, "description": description}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        task
    }

    async fn run_jobs(&self) {
        jobs::run_due(
            &self.state,
            &jobs::default_registry(),
            &WorkerOptions::default(),
        )
        .await
        .expect("run jobs");
    }

    async fn analyze(&self, client: &Client) {
        let (status, body) = client
            .post(&format!("{}/ai/analyze", self.base()), json!({}))
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        self.run_jobs().await;
        let state = sqlx::query_scalar::<_, String>("SELECT state FROM ai_runs WHERE id = $1")
            .bind(
                body["runId"]
                    .as_str()
                    .and_then(|id| id.parse::<Uuid>().ok()),
            )
            .fetch_one(&self.pool)
            .await
            .expect("run");
        assert_eq!(state, "succeeded");
    }

    async fn background_run(&self) {
        detect::request(
            &self.pool,
            self.organization_id,
            self.project_id,
            Mode::Nightly,
            None,
            Vec::new(),
        )
        .await
        .expect("queue run");
        self.run_jobs().await;
    }

    async fn suggestions(&self, client: &Client, query: &str) -> Vec<Value> {
        let (status, page) = client
            .get(&format!("{}/ai/suggestions?{query}", self.base()))
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        page["suggestions"].as_array().expect("suggestions").clone()
    }
}

fn find<'a>(suggestions: &'a [Value], kind: &str, task: &Value) -> Option<&'a Value> {
    suggestions
        .iter()
        .find(|suggestion| suggestion["kind"] == kind && suggestion["task"]["id"] == task["id"])
}

#[tokio::test]
async fn analyze_suggests_and_accepting_updates_readiness_and_the_graph() {
    let Some(fixture) = fixture("flow").await else {
        return;
    };
    let base = fixture.base();
    let (status, _) = fixture
        .owner
        .post(&format!("{base}/ai/analyze"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "no provider chosen yet");
    fixture.use_fake_provider().await;
    let (status, info) = fixture.owner.get(&format!("{base}/ai")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["configured"], true, "{info}");
    assert_eq!(info["provider"]["id"], "fake");

    let api = fixture
        .task(
            "Add the login API",
            "POST /login returns a session for the browser.",
        )
        .await;
    let page = fixture
        .task(
            "Login page",
            &format!(
                "Build the page once {} is done.",
                api["key"].as_str().expect("key")
            ),
        )
        .await;
    fixture.analyze(&fixture.owner).await;

    let pending = fixture.suggestions(&fixture.owner, "").await;
    let blocks = find(&pending, "blocks", &page).expect("a blocks suggestion on the page");
    assert_eq!(blocks["other"]["id"], api["id"]);
    assert!(blocks["confidence"].as_f64().expect("confidence") > 0.9);
    assert!(!blocks["reason"].as_str().expect("reason").is_empty());
    // The member sees the same suggestion: its sources are project tasks.
    assert!(
        find(
            &fixture.suggestions(&fixture.member, "").await,
            "blocks",
            &page
        )
        .is_some()
    );

    let (status, graph) = fixture.owner.get(&format!("{base}/ai/graph")).await;
    assert_eq!(status, StatusCode::OK, "{graph}");
    let suggested = graph["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .find(|edge| edge["suggestionId"] == blocks["id"])
        .expect("a dashed edge");
    assert_eq!(suggested["from"], api["id"]);
    assert_eq!(suggested["to"], page["id"]);

    // W11's timeline draws the same suggestion dashed between dated tasks.
    sqlx::query("UPDATE work_items SET due_date = current_date + 7 WHERE project_id = $1")
        .bind(fixture.project_id)
        .execute(&fixture.pool)
        .await
        .expect("dates");
    let (status, timeline) = fixture.member.get(&format!("{base}/timeline")).await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    assert_eq!(
        timeline["suggestedDependencies"],
        json!([{"blockingId": api["id"], "blockedId": page["id"]}]),
        "{timeline}"
    );

    let (status, ranking) = fixture.owner.get(&format!("{base}/ai/ready")).await;
    assert_eq!(status, StatusCode::OK, "{ranking}");
    let ready_ids = |ranking: &Value| {
        ranking["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .map(|task| task["id"].clone())
            .collect::<Vec<_>>()
    };
    assert!(ready_ids(&ranking).contains(&page["id"]));

    let (status, accepted) = fixture
        .owner
        .post(
            &format!(
                "{base}/ai/suggestions/{}/accept",
                blocks["id"].as_str().expect("id")
            ),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["suggestion"]["status"], "accepted");
    let (status, again) = fixture
        .owner
        .post(
            &format!(
                "{base}/ai/suggestions/{}/accept",
                blocks["id"].as_str().expect("id")
            ),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");

    let (_, ranking) = fixture.owner.get(&format!("{base}/ai/ready")).await;
    assert!(!ready_ids(&ranking).contains(&page["id"]), "blocked now");
    let (_, timeline) = fixture.owner.get(&format!("{base}/timeline")).await;
    assert_eq!(
        timeline["suggestedDependencies"],
        json!([]),
        "accepted: a real dependency now"
    );
    assert_eq!(timeline["dependencies"].as_array().map(Vec::len), Some(1));
    assert!(ready_ids(&ranking).contains(&api["id"]));
    let api_rank = ranking["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|task| task["id"] == api["id"])
        .expect("api ranked");
    assert_eq!(api_rank["unblocks"], 1);
    let (status, readiness) = fixture
        .owner
        .get(&format!(
            "{base}/work-items/{}/readiness",
            page["id"].as_str().expect("id")
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(readiness["ready"], false);
    assert_eq!(readiness["checks"][0]["id"], "dependencies");
    assert_eq!(readiness["checks"][0]["ok"], false);

    let (_, graph) = fixture.owner.get(&format!("{base}/ai/graph")).await;
    let edges = graph["edges"].as_array().expect("edges");
    assert!(edges.iter().any(|edge| edge["from"] == api["id"]
        && edge["to"] == page["id"]
        && edge["suggestionId"].is_null()));
    assert_eq!(graph["frontier"][0], api["id"]);

    // The change went through the task commands, marked as made via AI.
    let via_ai = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_activity WHERE payload->>'via' = 'ai' AND summary LIKE '%(via AI)'",
    )
    .fetch_one(&fixture.pool)
    .await
    .expect("activity");
    assert!(via_ai >= 1);
    // Every call was counted, and usage rows hold no prompt text.
    let (calls, prompts) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*), count(*) FILTER (WHERE error LIKE '%login%') FROM ai_usage",
    )
    .fetch_one(&fixture.pool)
    .await
    .expect("usage");
    assert!(calls >= 1);
    assert_eq!(prompts, 0);
}

#[tokio::test]
async fn private_sources_never_reach_someone_who_cant_see_them() {
    let Some(fixture) = fixture("private").await else {
        return;
    };
    fixture.use_fake_provider().await;
    let copy = fixture
        .task("Signup copy", "The words on the signup page.")
        .await;
    let key = copy["key"].as_str().expect("key").to_owned();
    let (status, channel) = fixture
        .owner
        .post(
            &format!("{}/channels", fixture.base()),
            json!({"name": "legal-review"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel_id = channel["conversation"]["id"]
        .as_str()
        .or_else(|| channel["id"].as_str())
        .expect("channel")
        .parse::<Uuid>()
        .expect("channel id");
    // Only the owner belongs to it.
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1")
        .bind(channel_id)
        .execute(&fixture.pool)
        .await
        .expect("make private");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1, $2, $3, $3) ON CONFLICT DO NOTHING",
    )
    .bind(fixture.project_id)
    .bind(channel_id)
    .bind(fixture.owner_actor)
    .execute(&fixture.pool)
    .await
    .expect("owner joins");
    sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND actor_id = $2")
        .bind(channel_id)
        .bind(fixture.member_actor)
        .execute(&fixture.pool)
        .await
        .expect("member leaves");
    let (status, sent) = fixture
        .owner
        .post(
            &format!("/api/v1/conversations/{channel_id}/messages"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": format!("{key} is waiting for legal to approve the copy.")}]}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");

    // A run with no person behind it never reads the private channel.
    fixture.background_run().await;
    let reasons = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ai_suggestions WHERE kind = 'blocked_reason'",
    )
    .fetch_one(&fixture.pool)
    .await
    .expect("count");
    assert_eq!(reasons, 0);

    // The owner's own Analyze reads it, for the owner alone.
    fixture.analyze(&fixture.owner).await;
    let mine = fixture.suggestions(&fixture.owner, "").await;
    let reason = find(&mine, "blocked_reason", &copy).expect("the owner sees it");
    assert_eq!(reason["personal"], true);
    assert!(
        reason["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .any(|source| source["kind"] == "message")
    );
    let id = reason["id"].as_str().expect("id").to_owned();
    assert!(
        find(
            &fixture.suggestions(&fixture.member, "status=all").await,
            "blocked_reason",
            &copy
        )
        .is_none(),
        "the member never sees it"
    );
    let (status, _) = fixture
        .member
        .post(
            &format!("{}/ai/suggestions/{id}/accept", fixture.base()),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, graph) = fixture
        .member
        .get(&format!("{}/ai/graph", fixture.base()))
        .await;
    assert!(!graph.to_string().contains(&id));

    // Losing the channel hides it from the owner too.
    sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND actor_id = $2")
        .bind(channel_id)
        .bind(fixture.owner_actor)
        .execute(&fixture.pool)
        .await
        .expect("owner leaves");
    assert!(
        find(
            &fixture.suggestions(&fixture.owner, "").await,
            "blocked_reason",
            &copy
        )
        .is_none()
    );
    let (status, _) = fixture
        .owner
        .post(
            &format!("{}/ai/suggestions/{id}/accept", fixture.base()),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dismissed_pairs_stay_dismissed_and_the_budget_stops_calls() {
    let Some(fixture) = fixture("dismiss").await else {
        return;
    };
    fixture.use_fake_provider().await;
    let schema = fixture
        .task("Design the schema", "Tables for accounts and sessions.")
        .await;
    let migrate = fixture
        .task(
            "Write the migration",
            &format!("Needs {} first.", schema["key"].as_str().expect("key")),
        )
        .await;
    fixture.analyze(&fixture.owner).await;
    let pending = fixture.suggestions(&fixture.owner, "").await;
    let suggestion = find(&pending, "blocks", &migrate).expect("suggested");
    let (status, dismissed) = fixture
        .owner
        .post(
            &format!(
                "{}/ai/suggestions/{}/dismiss",
                fixture.base(),
                suggestion["id"].as_str().expect("id")
            ),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{dismissed}");
    fixture.analyze(&fixture.owner).await;
    fixture.background_run().await;
    let pairs = sqlx::query_as::<_, (String, i64)>(
        "SELECT status, count(*) FROM ai_suggestions WHERE work_item_id = $1 OR other_work_item_id = $1 GROUP BY status",
    )
    .bind(migrate["id"].as_str().and_then(|id| id.parse::<Uuid>().ok()))
    .fetch_all(&fixture.pool)
    .await
    .expect("pairs");
    assert_eq!(pairs, vec![("dismissed".to_owned(), 1)]);

    // Over the monthly budget, calls stop with a message for the person.
    fixture
        .set_workspace("ai.monthly_budget_usd", json!(1))
        .await;
    sqlx::query(
        r#"
        INSERT INTO ai_usage (id, organization_id, project_id, prompt_id, prompt_version, provider, model, tier, input_tokens, output_tokens, cost_micros, outcome, latency_ms)
        VALUES ($1, $2, $3, 'ping', 1, 'fake', 'rules', 'quick', 0, 0, 2000000, 'succeeded', 1)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.organization_id)
    .bind(fixture.project_id)
    .execute(&fixture.pool)
    .await
    .expect("spend");
    let (status, body) = fixture
        .owner
        .post(
            &format!(
                "{}/work-items/{}/ai/criteria",
                fixture.base(),
                migrate["id"].as_str().expect("id")
            ),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "ai_budget_reached");
    let (status, workspace) = fixture
        .owner
        .get(&format!(
            "/api/v1/organizations/{}/ai",
            fixture.organization_id
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{workspace}");
    assert_eq!(workspace["budgetUsd"], 1);
    assert!(workspace["spentMicros"].as_i64().expect("spent") >= 2_000_000);
    let (status, _) = fixture
        .member
        .get(&format!(
            "/api/v1/organizations/{}/ai",
            fixture.organization_id
        ))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn automatic_apply_stops_for_someone_who_lost_access() {
    let Some(fixture) = fixture("auto").await else {
        return;
    };
    fixture.use_fake_provider().await;
    fixture
        .set_project("ai.auto_apply_suggestions", json!(true))
        .await;
    let first = fixture
        .task("Billing API", "Charges and refunds for accounts.")
        .await;
    let second = fixture
        .task(
            "Billing page",
            &format!(
                "Shows what {} returns.",
                first["key"].as_str().expect("key")
            ),
        )
        .await;
    fixture.background_run().await;
    let dependency = |blocked: &Value| {
        let id = blocked["id"]
            .as_str()
            .and_then(|id| id.parse::<Uuid>().ok());
        let pool = fixture.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM work_item_dependencies WHERE blocked_work_item_id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("dependencies")
        }
    };
    assert_eq!(dependency(&second).await, 1, "applied as the owner");
    let applied = sqlx::query_scalar::<_, String>(
        "SELECT status FROM ai_suggestions WHERE work_item_id = $1 AND kind = 'blocks'",
    )
    .bind(second["id"].as_str().and_then(|id| id.parse::<Uuid>().ok()))
    .fetch_one(&fixture.pool)
    .await
    .expect("status");
    assert_eq!(applied, "applied");

    // The person who turned the switch on leaves the project: nothing more
    // is applied in their name, and suggestions wait for people.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(fixture.project_id)
    .bind(fixture.owner_actor)
    .execute(&fixture.pool)
    .await
    .expect("disable owner");
    let third = sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
        .bind(fixture.project_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("key");
    let (status, report) = fixture
        .member
        .post(
            &format!("{}/work-items", fixture.base()),
            json!({"title": "Billing emails", "description": format!("Send after {}-{} is done.", third, first["number"])}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{report}");
    fixture.background_run().await;
    assert_eq!(
        dependency(&report).await,
        0,
        "not applied for someone who left"
    );
    let pending = sqlx::query_scalar::<_, String>(
        "SELECT status FROM ai_suggestions WHERE work_item_id = $1 AND kind = 'blocks'",
    )
    .bind(report["id"].as_str().and_then(|id| id.parse::<Uuid>().ok()))
    .fetch_one(&fixture.pool)
    .await
    .expect("status");
    assert_eq!(pending, "pending");
}

#[tokio::test]
async fn the_roadmap_imports_as_tasks_and_a_stopped_import_resumes() {
    let Some(fixture) = fixture("plan").await else {
        return;
    };
    let roadmap: Value =
        serde_json::from_str(include_str!("../../../../docs/plans/roadmap.json")).expect("json");
    let items = roadmap["items"].as_array().expect("items").len();
    let workstreams = roadmap["workstreams"]
        .as_array()
        .expect("workstreams")
        .len();
    let dependencies = roadmap["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["deps"].as_array().map_or(0, Vec::len))
        .sum::<usize>();
    let base = fixture.base();
    let (status, created) = fixture
        .owner
        .post(
            &format!("{base}/ai/plans"),
            json!({"title": "Roadmap", "source": {"kind": "structured", "plan": roadmap}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let plan = &created["plan"];
    assert_eq!(plan["state"], "draft");
    assert_eq!(plan["draft"]["tasks"].as_array().map(Vec::len), Some(items));
    let id = plan["id"]
        .as_str()
        .expect("id")
        .parse::<Uuid>()
        .expect("uuid");

    // A draft is edited with the version read; a stale version is refused.
    let mut draft = plan["draft"].clone();
    draft["tasks"][0]["priority"] = json!("high");
    let (status, saved) = fixture
        .owner
        .put(
            &format!("{base}/ai/plans/{id}"),
            json!({"expectedVersion": plan["version"], "draft": draft}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, _) = fixture
        .owner
        .put(
            &format!("{base}/ai/plans/{id}"),
            json!({"expectedVersion": plan["version"], "draft": draft}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, committed) = fixture
        .owner
        .post(
            &format!("{base}/ai/plans/{id}/commit"),
            json!({"expectedVersion": saved["plan"]["version"]}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{committed}");

    // Two steps (milestones, the first hundred tasks), then the server
    // "stops"; the job picks up from there.
    assert_eq!(
        plans::commit_next_step(&fixture.pool, id)
            .await
            .expect("step"),
        Step::Progress
    );
    assert_eq!(
        plans::commit_next_step(&fixture.pool, id)
            .await
            .expect("step"),
        Step::Progress
    );
    let so_far = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ai_plan_import_items WHERE import_id = $1 AND kind = 'task'",
    )
    .bind(id)
    .fetch_one(&fixture.pool)
    .await
    .expect("count");
    assert_eq!(so_far, 100);
    fixture.run_jobs().await;
    assert_eq!(
        plans::commit_next_step(&fixture.pool, id)
            .await
            .expect("step"),
        Step::Done
    );

    let (status, finished) = fixture.owner.get(&format!("{base}/ai/plans/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(finished["plan"]["state"], "committed", "{finished}");
    assert_eq!(finished["plan"]["createdTasks"], items);
    assert_eq!(finished["plan"]["createdMilestones"], workstreams);
    assert_eq!(finished["plan"]["createdDependencies"], dependencies);
    let tasks = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_rows WHERE project_id = $1 AND title LIKE 'F1 %'",
    )
    .bind(fixture.project_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("count");
    assert_eq!(tasks, 1, "created once");
    let (milestone, priority) = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT milestone.name, item.priority FROM task_rows item
        JOIN project_milestones milestone ON milestone.id = item.milestone_id
        WHERE item.project_id = $1 AND item.title LIKE 'F1 %'
        "#,
    )
    .bind(fixture.project_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("F1");
    assert!(milestone.starts_with("W0"), "{milestone}");
    assert_eq!(priority, "high");
    let inbox = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = 'ai_plan_import'",
    )
    .bind(fixture.owner_actor)
    .fetch_one(&fixture.pool)
    .await
    .expect("notification");
    assert_eq!(inbox, 1);

    // Text needs a provider; without one the person is told what to set.
    let (status, body) = fixture
        .owner
        .post(
            &format!("{base}/ai/plans"),
            json!({"title": "Notes", "source": {"kind": "text", "text": "# Beta\n- Ship it"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    fixture.use_fake_provider().await;
    let (status, body) = fixture
        .owner
        .post(
            &format!("{base}/ai/plans"),
            json!({"title": "Notes", "source": {"kind": "text", "text": "# Beta (by 2026-12-01)\n- Login API\n- Login page after"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["plan"]["state"], "drafting");
    fixture.run_jobs().await;
    let (_, drafted) = fixture
        .owner
        .get(&format!(
            "{base}/ai/plans/{}",
            body["plan"]["id"].as_str().expect("id")
        ))
        .await;
    assert_eq!(drafted["plan"]["state"], "draft", "{drafted}");
    assert_eq!(
        drafted["plan"]["draft"]["tasks"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        drafted["plan"]["draft"]["milestones"][0]["targetDate"],
        "2026-12-01"
    );
}

/// Calls `tool` on the public API with a personal token: a script, which
/// acts within the person's agent limit like an app.
async fn public_tool(
    application: &axum::Router,
    token: &str,
    tool: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/public/v1/{tool}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("tool body")))
                .expect("tool request"),
        )
        .await
        .expect("tool response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000)
        .await
        .expect("tool response body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

impl Fixture {
    /// A personal token for the member, with write access to the project.
    async fn member_token(&self) -> String {
        let (status, issued) = self
            .member
            .post(
                "/api/v1/account/api-tokens",
                json!({"projectId": self.project_id, "name": "script", "scopes": ["read", "write"], "expiresInDays": 30}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{issued}");
        issued["token"].as_str().expect("token").to_owned()
    }

    /// Narrows what apps and scripts acting for the member may do.
    async fn limit_member_apps(&self, permissions: Value) {
        let (status, body) = self
            .owner
            .put(
                &format!(
                    "/api/v1/organizations/{}/members/{}/agent-limit",
                    self.organization_id, self.member_actor
                ),
                json!({"permissions": permissions}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    async fn tasks_titled(&self, title: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM task_rows WHERE project_id = $1 AND title = $2",
        )
        .bind(self.project_id)
        .bind(title)
        .fetch_one(&self.pool)
        .await
        .expect("count tasks")
    }

    /// Runs jobs until the project's newest import has finished one way or
    /// the other; returns its status and error.
    async fn finish_newest_import(&self) -> (String, Option<String>) {
        let mut last = (String::new(), None);
        for _ in 0..10 {
            self.run_jobs().await;
            last = sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT status, error FROM task_imports WHERE project_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
            )
            .bind(self.project_id)
            .fetch_one(&self.pool)
            .await
            .expect("newest import");
            if matches!(last.0.as_str(), "completed" | "failed") {
                break;
            }
        }
        last
    }
}

/// AWD-74: an import a script committed creates its tasks within the
/// person's agent limit as it is when each chunk runs. Narrowed after the
/// commit, the import stops with a reason and makes none of its tasks.
#[tokio::test]
async fn an_import_a_script_committed_stops_when_its_limit_narrows() {
    let Some(fixture) = fixture("import_limit").await else {
        return;
    };
    let application = fixture.member.application.clone();
    let token = fixture.member_token().await;

    // Within the limit, the import runs to the end.
    let (status, queued) = public_tool(
        &application,
        &token,
        "task_import",
        json!({"projectId": fixture.project_id, "source": "markdown", "text": "- [ ] Control import", "commit": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{queued}");
    let (state, error) = fixture.finish_newest_import().await;
    assert_eq!(state, "completed", "{error:?}");
    assert_eq!(fixture.tasks_titled("Control import").await, 1);

    // Narrowed between the commit and the chunk: stopped, nothing made.
    let (status, queued) = public_tool(
        &application,
        &token,
        "task_import",
        json!({"projectId": fixture.project_id, "source": "markdown", "text": "- [ ] Narrowed import", "commit": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{queued}");
    fixture.limit_member_apps(json!(["tasks.view"])).await;
    let (state, error) = fixture.finish_newest_import().await;
    assert_eq!(state, "failed", "{error:?}");
    assert!(
        error.as_deref().is_some_and(|error| error.contains("app")),
        "{error:?}"
    );
    assert_eq!(fixture.tasks_titled("Narrowed import").await, 0);
}

/// AWD-74: each step of committing a plan needs what its changes need,
/// checked again every step: creating tasks and changing them. A plan a
/// script committed stops once the script's limit loses "edit tasks"; a
/// plan committed in the browser stops once the person's role loses
/// "create tasks". Neither makes its tasks.
#[tokio::test]
async fn a_plan_stops_when_its_committer_loses_what_its_steps_need() {
    let Some(fixture) = fixture("plan_limit").await else {
        return;
    };
    let base = fixture.base();
    let application = fixture.member.application.clone();
    let token = fixture.member_token().await;
    let plan = |title: &str| {
        json!({
            "title": title,
            "source": {"kind": "structured", "plan": {"tasks": [
                {"ref": "one", "title": format!("{title} one")},
                {"ref": "two", "title": format!("{title} two"), "blockedBy": ["one"]},
            ]}},
        })
    };

    // Committed by a script, whose limit then keeps "create" but not "edit".
    let (status, created) = fixture
        .member
        .post(&format!("{base}/ai/plans"), plan("Scripted plan"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["plan"]["id"]
        .as_str()
        .expect("plan id")
        .parse::<Uuid>()
        .expect("uuid");
    let (status, committed) = public_tool(
        &application,
        &token,
        "plan_import",
        json!({
            "projectId": fixture.project_id,
            "importId": id,
            "commit": true,
            "expectedVersion": created["plan"]["version"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    fixture
        .limit_member_apps(json!(["tasks.view", "tasks.create"]))
        .await;
    let step = plans::commit_next_step(&fixture.pool, id)
        .await
        .expect("step");
    assert!(
        matches!(&step, Step::Failed(message) if message.contains("app") && message.contains("change")),
        "{step:?}"
    );
    assert_eq!(fixture.tasks_titled("Scripted plan one").await, 0);

    // Committed in the browser; the person's role then loses "create".
    let (status, created) = fixture
        .member
        .post(&format!("{base}/ai/plans"), plan("Browser plan"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["plan"]["id"]
        .as_str()
        .expect("plan id")
        .parse::<Uuid>()
        .expect("uuid");
    let (status, committed) = fixture
        .member
        .post(
            &format!("{base}/ai/plans/{id}/commit"),
            json!({"expectedVersion": created["plan"]["version"]}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{committed}");
    let editors = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO workspace_roles (id, organization_id, key, base_role, name, permissions)
        VALUES (gen_random_uuid(), $1, 'editors-only', 'member', 'Editors only',
                ARRAY['projects.view', 'tasks.view', 'tasks.edit'])
        RETURNING id
        "#,
    )
    .bind(fixture.organization_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("role without create tasks");
    sqlx::query(
        "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(fixture.project_id)
    .bind(fixture.member_actor)
    .bind(editors)
    .execute(&fixture.pool)
    .await
    .expect("narrow the member's role");
    let step = plans::commit_next_step(&fixture.pool, id)
        .await
        .expect("step");
    assert!(
        matches!(&step, Step::Failed(message) if message.contains("can no longer create")),
        "{step:?}"
    );
    assert_eq!(fixture.tasks_titled("Browser plan one").await, 0);
}

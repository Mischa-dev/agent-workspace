//! AWD-91: the agent parity scenario. Mischa's bar is that an agent can do
//! everything a person can in a project, so this walks one agent from a goal
//! in an empty project to a finished run, through MCP alone: it writes the
//! plan page, builds a collection, draws a whiteboard, creates tasks with
//! dependencies and links, asks for a run, and reports back in a thread.
//!
//! The person does only what is theirs by design: posting the goal and
//! approving what spends their own account (the run, and the harness's
//! request to change a file). The run uses the built-in demo harness, so it
//! needs no provider binary.
//!
//! When a build-out feature lands, add its step here: what an agent would
//! do with it in this project, through its MCP tools, and a check that the
//! result is there and was made by the agent. The guard beside this test,
//! `agent_parity::tests` in the server crate, fails on a web write that has
//! no MCP tool.

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    git_runtime::expected_repository_mirror_path,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    seed::{DevSeed, dev_seed},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, PASSWORD, connect_app, initialize, login, schema_pool, send, tool},
    m3_scheduler_http::{create_test_mirror, wait_for_state},
};

struct Fixture {
    app: axum::Router,
    pool: PgPool,
    admin: PgPool,
    schema: String,
    browser: Browser,
    seed: DevSeed,
    agent: Uuid,
    token: String,
    session: String,
    _root: tempfile::TempDir,
}

impl Fixture {
    /// One MCP call as the agent. Every result is checked against the
    /// tool's declared output schema on the way.
    async fn call(&self, name: &str, arguments: Value) -> Value {
        let result = tool(
            &self.app,
            &self.token,
            &self.session,
            name,
            arguments.clone(),
        )
        .await;
        assert!(
            matches!(
                result["outcome"].as_str(),
                Some("ok" | "applied" | "requested" | "created")
            ),
            "{name} {arguments}: {result}"
        );
        result
    }

    /// A person's step in the web app.
    async fn web(&self, method: &str, uri: &str, body: Value) -> Value {
        let key = format!("awd91-{}", Uuid::now_v7().simple());
        let (status, value) = send(
            &self.app,
            self.browser.request(method, uri, Some(&key), body),
        )
        .await;
        assert!(status.is_success(), "{method} {uri}: {status} {value}");
        value
    }

    /// Who made a row, by the column that records it.
    async fn author(&self, sql: &str, id: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(sql)
            .bind(id.parse::<Uuid>().expect("row UUID"))
            .fetch_one(&self.pool)
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
    }

    async fn close(self) {
        drop(self.app);
        self.pool.close().await;
        agent_workspace_server::test_support::drop_schema(&self.admin, &self.schema)
            .await
            .expect("drop the parity schema");
    }
}

async fn fixture() -> Option<Fixture> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("awd91_parity_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");

    let root = tempfile::tempdir().expect("temporary root");
    let data = root.path().join("data");
    let mut config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data.join("codex/profiles").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_DEMO_EXECUTOR".to_owned(),
            "true".to_owned(),
        ),
    ]))
    .expect("configuration");
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;

    // A project with a repository and a harness account ready to run, and
    // nothing else of its own yet: no pages, tasks, collections or boards.
    let seed = dev_seed(&pool, &config).await.expect("seed");
    sqlx::query(
        "UPDATE execution_profiles SET status = 'available', account_state = 'authenticated' WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("sign the account in");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare the repository");
    let mirror = expected_repository_mirror_path(&data, seed.repository_id).expect("mirror path");
    create_test_mirror(root.path(), &mirror);
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap the owner");

    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let app = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(64),
        supervisors,
    });
    let browser = login(&app).await;

    // The agent joins as an ordinary member, the way a person adds one.
    let (status, created) = send(
        &app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{}/external-agents", seed.project_id),
            Some("awd91-agent"),
            json!({
                "connectionName": "Planner",
                "displayName": "Planner",
                "handle": "planner",
                "role": "member",
                "scopes": ["read", "write", "events", "approval_respond", "worker_request"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let agent = created["connection"]["actorId"]
        .as_str()
        .expect("agent actor")
        .parse::<Uuid>()
        .expect("agent UUID");
    let token = created["oneTimeCredential"]["token"]
        .as_str()
        .expect("agent token")
        .to_owned();
    let session = initialize(&app, &token).await.expect("agent MCP session");

    Some(Fixture {
        app,
        pool,
        admin,
        schema,
        browser,
        seed,
        agent,
        token,
        session,
        _root: root,
    })
}

fn string(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no {pointer} in {value}"))
        .to_owned()
}

#[tokio::test]
async fn an_agent_takes_an_empty_project_from_goal_to_finished_run_through_mcp_alone() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();
    let channel = f.seed.channel_id.to_string();

    // The person posts the goal in the project's channel.
    let goal = f
        .web(
            "POST",
            &format!("/api/v1/channels/{channel}/messages"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Planner, plan the proof-line release: track its risks, sketch the flow, make the tasks, run the first one and tell me how it went."}]}}),
        )
        .await;
    let goal_message = string(&goal, "/message/id");

    // 1. The agent finds the goal and gets oriented.
    let oriented = f.call("project_open", json!({"projectId": project})).await;
    assert_eq!(string(&oriented, "/value/project/key"), "AW", "{oriented}");
    let read = f
        .call("conversation_read", json!({"conversationId": channel}))
        .await;
    assert!(read.to_string().contains("proof-line release"), "{read}");

    // 2. It writes the plan page.
    let page = f
        .call(
            "page_write",
            json!({
                "projectId": project,
                "title": "Plan: proof-line release",
                "body": "# Plan: proof-line release\n\nShip the proof line.\n\n## Steps\n\n- Write the proof line\n- Check it\n- Release it\n",
            }),
        )
        .await;
    let page_id = string(&page, "/value/id");
    // ... and changes one block of it, without rewriting the page.
    let blocks = f.call("page_blocks", json!({"pageId": page_id})).await;
    let last = blocks["value"]["blocks"]
        .as_array()
        .and_then(|blocks| blocks.last())
        .cloned()
        .unwrap_or_else(|| panic!("no blocks: {blocks}"));
    f.call(
        "page_edit",
        json!({"pageId": page_id, "edits": [{
            "op": "insertAfter",
            "blockId": last["id"],
            "hash": last["hash"],
            "markdown": "Risks are tracked in the Risks collection."
        }]}),
    )
    .await;

    // 3. It builds a collection for the risks and fills it in.
    f.call(
        "collection_write",
        json!({
            "projectId": project,
            "name": "Risks",
            "titleName": "Risk",
            "fields": [
                {"name": "Severity", "type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]},
                {"name": "Owner", "type": "person"},
                {"name": "Plan", "type": "link", "target": "page"}
            ],
            "views": [{"name": "By severity", "groupBy": "Severity"}]
        }),
    )
    .await;
    let records = f
        .call(
            "record_write",
            json!({
                "projectId": project,
                "collection": "Risks",
                "clientRequestId": "awd91-risks",
                "records": [
                    {"title": "The proof line is wrong", "fields": {"Severity": "High", "Owner": "me", "Plan": page_id}},
                    {"title": "The release slips", "fields": {"Severity": "Low"}}
                ]
            }),
        )
        .await;
    assert_eq!(records["value"]["created"], 2, "{records}");

    // 4. It draws the flow on a whiteboard linked to the plan page.
    let board = f
        .call(
            "whiteboard_create",
            json!({
                "title": "Release flow",
                "projectId": project,
                "linkTo": {"targetKind": "page", "targetId": page_id},
                "operations": [
                    {"op": "add", "type": "rectangle", "id": "write", "text": "Write"},
                    {"op": "add", "type": "rectangle", "id": "check", "text": "Check"},
                    {"op": "add", "type": "ellipse", "id": "release", "text": "Release"},
                    {"op": "connect", "from": "write", "to": "check"},
                    {"op": "connect", "from": "check", "to": "release"},
                    {"op": "layout", "ids": ["write", "check", "release"], "direction": "right", "gap": 80}
                ]
            }),
        )
        .await;
    let board_id = string(&board, "/value/whiteboard/id");

    // 5. It makes the tasks, chains them, and links them to the plan.
    let tasks = f
        .call(
            "task_create_many",
            json!({
                "projectId": project,
                "clientRequestId": "awd91-tasks",
                "tasks": [
                    {"ref": "write", "title": "Write the proof line"},
                    {"ref": "check", "title": "Check the proof line"},
                    {"ref": "release", "title": "Release the proof line"}
                ]
            }),
        )
        .await;
    let keys = tasks["value"]["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("no tasks: {tasks}"))
        .iter()
        .map(|task| task["key"].as_str().expect("task key").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 3, "{tasks}");
    f.call(
        "task_depend",
        json!({"taskId": keys[1], "blockedBy": [keys[0]]}),
    )
    .await;
    f.call(
        "task_depend",
        json!({"taskId": keys[2], "blockedBy": [keys[1]]}),
    )
    .await;
    for key in &keys {
        f.call("task_link", json!({"taskId": key, "pageId": page_id}))
            .await;
    }
    let ready = f
        .call("task_ready_list", json!({"projectId": project}))
        .await;
    let ready_keys = ready.to_string();
    assert!(ready_keys.contains(&keys[0]), "{ready}");
    assert!(!ready_keys.contains(&keys[2]), "{ready}");

    // 6. It opens a thread for the first task, from the goal, and asks for
    // a run there. The person approves spending their account.
    let thread = f
        .call(
            "thread_start",
            json!({"conversationId": channel, "fromMessageId": goal_message, "name": "Write the proof line"}),
        )
        .await;
    let thread_id = string(&thread, "/value/id");
    f.call(
        "task_link",
        json!({"taskId": keys[0], "conversationId": thread_id}),
    )
    .await;
    f.call(
        "task_claim",
        json!({"taskId": keys[0], "note": "Running the first step."}),
    )
    .await;
    let requested = f
        .call(
            "run_request",
            json!({
                "conversationId": thread_id,
                "repositoryId": f.seed.repository_id.to_string(),
                "sponsorActorId": f.seed.human_actor_id.to_string(),
                "prompt": format!("{}: append the proof line.", keys[0]),
                "clientRequestId": "awd91-run"
            }),
        )
        .await;
    let sponsorship = string(&requested, "/approvalId");
    let approved = f
        .web(
            "POST",
            &format!("/api/v1/agent-approvals/{sponsorship}/respond"),
            json!({"decision": "approve"}),
        )
        .await;
    let run = approved["agentRunId"]
        .as_str()
        .unwrap_or_else(|| panic!("no run: {approved}"))
        .parse::<Uuid>()
        .expect("run UUID");

    // The demo harness asks before it changes a file; the person allows it.
    wait_for_state(&f.pool, run, "blocked").await;
    let question = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM agent_approvals WHERE agent_run_id = $1 AND status = 'pending'",
    )
    .bind(run)
    .fetch_one(&f.pool)
    .await
    .expect("the harness's question");
    f.web(
        "POST",
        &format!("/api/v1/agent-approvals/{question}/respond"),
        json!({"decision": "approve"}),
    )
    .await;
    wait_for_state(&f.pool, run, "completed").await;

    // 7. It reads the run and reports back in the thread, then settles the
    // thread and moves the task on.
    let finished = f.call("run_get", json!({"runId": run.to_string()})).await;
    assert_eq!(finished["value"]["stage"], "finished", "{finished}");
    f.call(
        "message_send",
        json!({
            "conversationId": thread_id,
            "text": format!("{} is done: the run appended the proof line. {} is next.", keys[0], keys[1]),
            "clientRequestId": "awd91-report"
        }),
    )
    .await;
    f.call(
        "thread_update",
        json!({"threadId": thread_id, "status": "done", "outcome": "The proof line is written."}),
    )
    .await;
    let opened = f.call("task_open", json!({"taskId": keys[0]})).await;
    let mut version = opened["value"]["task"]["version"]
        .as_i64()
        .unwrap_or_else(|| panic!("no version: {opened}"));
    for status in ["Ready", "In Progress", "Review"] {
        let moved = f
            .call(
                "task_update",
                json!({"taskId": keys[0], "status": status, "expectedVersion": version}),
            )
            .await;
        version = moved["version"]
            .as_i64()
            .unwrap_or_else(|| panic!("no version: {moved}"));
    }

    // What it made is there, made by the agent and not the person.
    let agent = f.agent;
    assert_eq!(
        f.author(
            "SELECT created_by_actor_id FROM project_pages WHERE id = $1",
            &page_id
        )
        .await,
        agent
    );
    assert_eq!(
        f.author(
            "SELECT created_by_actor_id FROM whiteboards WHERE id = $1",
            &board_id
        )
        .await,
        agent
    );
    let records_by_agent = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collection_records WHERE project_id = $1 AND created_by_actor_id = $2",
    )
    .bind(f.seed.project_id)
    .bind(agent)
    .fetch_one(&f.pool)
    .await
    .expect("records");
    assert_eq!(records_by_agent, 2);
    let made_by_agent = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_items WHERE project_id = $1 AND created_by_actor_id = $2",
    )
    .bind(f.seed.project_id)
    .bind(agent)
    .fetch_one(&f.pool)
    .await
    .expect("tasks");
    assert_eq!(made_by_agent, 3);
    let requester =
        sqlx::query_scalar::<_, Uuid>("SELECT requesting_actor_id FROM agent_runs WHERE id = $1")
            .bind(run)
            .fetch_one(&f.pool)
            .await
            .expect("run");
    assert_eq!(requester, agent);
    let thread_messages = f
        .call("conversation_read", json!({"conversationId": thread_id}))
        .await;
    assert!(
        thread_messages
            .to_string()
            .contains("appended the proof line"),
        "{thread_messages}"
    );

    f.close().await;
}

/// AWD-111, run control: the agent stops the run it asked for, but can't
/// start it again on the person's account; the person's app can. A launch
/// in the queue is cancelled by whoever queued it, not by an agent.
#[tokio::test]
async fn an_agent_stops_its_run_and_only_people_start_it_again() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();
    let thread = f
        .call(
            "thread_start",
            json!({"conversationId": f.seed.channel_id.to_string(), "name": "Stop and retry", "text": "Trying a run."}),
        )
        .await;
    let thread_id = string(&thread, "/value/id");
    let requested = f
        .call(
            "run_request",
            json!({
                "conversationId": thread_id,
                "repositoryId": f.seed.repository_id.to_string(),
                "sponsorActorId": f.seed.human_actor_id.to_string(),
                "prompt": "Append the proof line.",
                "clientRequestId": "awd111-run"
            }),
        )
        .await;
    let sponsorship = string(&requested, "/approvalId");
    let approved = f
        .web(
            "POST",
            &format!("/api/v1/agent-approvals/{sponsorship}/respond"),
            json!({"decision": "approve"}),
        )
        .await;
    let run = approved["agentRunId"]
        .as_str()
        .unwrap_or_else(|| panic!("no run: {approved}"))
        .to_owned();
    let run_id = run.parse::<Uuid>().expect("run UUID");
    wait_for_state(&f.pool, run_id, "blocked").await;

    // The agent stops it: the harness's open question goes with it.
    let stopped = f
        .call(
            "run_stop",
            json!({"runId": run, "clientRequestId": "awd111-stop"}),
        )
        .await;
    assert_eq!(stopped["value"]["stage"], "interrupted", "{stopped}");
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM agent_approvals WHERE agent_run_id = $1 AND status = 'pending'",
    )
    .bind(run_id)
    .fetch_one(&f.pool)
    .await
    .expect("open questions");
    assert_eq!(pending, 0);
    // Stopping it again changes nothing.
    f.call(
        "run_stop",
        json!({"runId": run, "clientRequestId": "awd111-stop-again"}),
    )
    .await;

    // Starting it again spends the person's account, so the agent can't.
    let refused = tool(
        &f.app,
        &f.token,
        &f.session,
        "run_retry",
        json!({"runId": run, "clientRequestId": "awd111-agent-retry"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // The person's own app can, and the new run is theirs.
    let app = connect_app(&f.app, &f.browser, "Owner's app", "full").await;
    let app_session = initialize(&f.app, &app).await.expect("app session");
    let retried = tool(
        &f.app,
        &app,
        &app_session,
        "run_retry",
        json!({"runId": run, "clientRequestId": "awd111-retry"}),
    )
    .await;
    assert_eq!(retried["outcome"], "ok", "{retried}");
    let new_run = string(&retried, "/value/id");
    assert_ne!(new_run, run);
    let (requester, predecessor, through_app) = sqlx::query_as::<_, (Uuid, Option<Uuid>, bool)>(
        "SELECT requesting_actor_id, predecessor_run_id, made_through_app FROM agent_runs WHERE id = $1",
    )
    .bind(new_run.parse::<Uuid>().expect("new run UUID"))
    .fetch_one(&f.pool)
    .await
    .expect("retried run");
    assert_eq!(requester, f.seed.human_actor_id);
    assert_eq!(predecessor, Some(run_id));
    // Made in the app's call, so if it waits in the queue it starts only
    // within the app's limit as it is then, as run_start's runs do.
    assert!(through_app);

    // A launch the person queued: the agent can't cancel it, they can.
    let task = f
        .call(
            "task_create",
            json!({"projectId": project, "title": "Launch later"}),
        )
        .await;
    let key = string(&task, "/key");
    let far = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    let launched = tool(
        &f.app,
        &app,
        &app_session,
        "task_launch",
        json!({"taskIds": [key], "startWhen": "at", "startAt": far, "prompt": "Later.", "clientRequestId": "awd111-launch"}),
    )
    .await;
    assert_eq!(launched["outcome"], "ok", "{launched}");
    let launch = string(&launched, "/value/launches/0/id");
    let not_theirs = tool(
        &f.app,
        &f.token,
        &f.session,
        "launch_cancel",
        json!({"launchId": launch}),
    )
    .await;
    assert_eq!(not_theirs["outcome"], "permission_denied", "{not_theirs}");
    let cancelled = tool(
        &f.app,
        &app,
        &app_session,
        "launch_cancel",
        json!({"launchId": launch}),
    )
    .await;
    assert_eq!(cancelled["value"]["state"], "cancelled", "{cancelled}");
    let again = tool(
        &f.app,
        &app,
        &app_session,
        "launch_cancel",
        json!({"launchId": launch}),
    )
    .await;
    assert_eq!(again["outcome"], "conflict", "{again}");

    f.close().await;
}

/// AWD-111: an agent keeps views of its own, shares one with the project,
/// and can't see or change a person's own views.
#[tokio::test]
async fn an_agent_saves_shares_and_deletes_task_views() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();
    let views_uri = format!("/api/v1/projects/{project}/views");
    let names = |listed: &Value| -> Vec<String> {
        listed["views"]
            .as_array()
            .expect("views")
            .iter()
            .map(|view| view["name"].as_str().unwrap_or_default().to_owned())
            .collect()
    };

    // A view of the agent's own: the person doesn't see it.
    let mine = f
        .call(
            "view_write",
            json!({"projectId": project, "name": "My open work", "query": {"assigneeActorId": "me", "includeDone": "false"}, "pinned": true}),
        )
        .await;
    assert_eq!(mine["value"]["created"], true, "{mine}");
    assert_eq!(mine["value"]["view"]["shared"], false, "{mine}");
    assert_eq!(mine["value"]["view"]["pinned"], true, "{mine}");
    let mine_id = string(&mine, "/value/view/id");
    assert_eq!(
        f.author(
            "SELECT owner_actor_id FROM saved_views WHERE id = $1",
            &mine_id
        )
        .await,
        f.agent
    );
    let listed = f.web("GET", &views_uri, Value::Null).await;
    assert!(!names(&listed).contains(&"My open work".to_owned()));
    // The same name again is a conflict, so a retried call makes no copy.
    let again = tool(
        &f.app,
        &f.token,
        &f.session,
        "view_write",
        json!({"projectId": project, "name": "my open work"}),
    )
    .await;
    assert_eq!(again["outcome"], "conflict", "{again}");
    // task_list reads it.
    f.call(
        "task_list",
        json!({"projectId": project, "view": "My open work"}),
    )
    .await;

    // Shared, it shows for the person, without the agent's pin.
    let shared = f
        .call(
            "view_write",
            json!({"projectId": project, "view": "My open work", "name": "Planner's queue", "shared": true, "layout": "board", "expectedVersion": mine["value"]["view"]["version"]}),
        )
        .await;
    assert_eq!(shared["value"]["view"]["shared"], true, "{shared}");
    assert_eq!(shared["value"]["view"]["layout"], "board", "{shared}");
    let listed = f.web("GET", &views_uri, Value::Null).await;
    let seen = listed["views"]
        .as_array()
        .expect("views")
        .iter()
        .find(|view| view["name"] == "Planner's queue")
        .unwrap_or_else(|| panic!("shared view: {listed}"))
        .clone();
    assert_eq!(seen["pinned"], false, "{seen}");
    // An old version is a conflict.
    let stale = tool(
        &f.app,
        &f.token,
        &f.session,
        "view_write",
        json!({"projectId": project, "view": mine_id, "name": "Stale", "expectedVersion": mine["value"]["view"]["version"]}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // The person's own view stays theirs: the agent can't find it.
    let private = f
        .web(
            "POST",
            &views_uri,
            json!({"name": "Owner's private", "query": {"priority": "urgent"}}),
        )
        .await;
    let private_id = private["id"].as_str().expect("view id").to_owned();
    for arguments in [
        json!({"projectId": project, "view": private_id, "name": "Taken"}),
        json!({"projectId": project, "view": "Owner's private", "pinned": true}),
        json!({"projectId": project, "view": private_id, "delete": true}),
    ] {
        let refused = tool(&f.app, &f.token, &f.session, "view_write", arguments).await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
    }
    let listed = f.web("GET", &views_uri, Value::Null).await;
    assert!(names(&listed).contains(&"Owner's private".to_owned()));

    // Two views the agent can see with one name: a new one by that name is
    // still a conflict, and the name alone picks neither.
    f.call("view_write", json!({"projectId": project, "name": "Twins"}))
        .await;
    f.web("POST", &views_uri, json!({"name": "Twins", "shared": true}))
        .await;
    for (arguments, outcome) in [
        (json!({"projectId": project, "name": "Twins"}), "conflict"),
        (
            json!({"projectId": project, "view": "Twins", "layout": "board"}),
            "not_found",
        ),
    ] {
        let refused = tool(&f.app, &f.token, &f.session, "view_write", arguments).await;
        assert_eq!(refused["outcome"], outcome, "{refused}");
    }
    let twins = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM saved_views WHERE project_id = $1 AND name = 'Twins'",
    )
    .bind(f.seed.project_id)
    .fetch_one(&f.pool)
    .await
    .expect("twins");
    assert_eq!(twins, 2);

    // A query that names something outside the project is refused.
    let foreign = tool(
        &f.app,
        &f.token,
        &f.session,
        "view_write",
        json!({"projectId": project, "name": "Elsewhere", "query": {"milestoneId": Uuid::now_v7().to_string()}}),
    )
    .await;
    assert_eq!(foreign["outcome"], "invalid_request", "{foreign}");

    // Built-ins can be unpinned but not deleted; the shared view can go.
    let builtin = tool(
        &f.app,
        &f.token,
        &f.session,
        "view_write",
        json!({"projectId": project, "view": "blocked", "delete": true}),
    )
    .await;
    assert_eq!(builtin["outcome"], "conflict", "{builtin}");
    let unpinned = f
        .call(
            "view_write",
            json!({"projectId": project, "view": "blocked", "pinned": false}),
        )
        .await;
    assert_eq!(unpinned["value"]["view"]["pinned"], false, "{unpinned}");
    let deleted = f
        .call(
            "view_write",
            json!({"projectId": project, "view": "Planner's queue", "delete": true}),
        )
        .await;
    assert_eq!(deleted["value"]["deleted"], true, "{deleted}");
    let listed = f.web("GET", &views_uri, Value::Null).await;
    assert!(!names(&listed).contains(&"Planner's queue".to_owned()));

    f.close().await;
}

/// AWD-111: an agent keeps a project's task templates: makes one, changes
/// part of it, makes tasks from it, and retires it.
#[tokio::test]
async fn an_agent_writes_task_templates_and_uses_them() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();

    let made = f
        .call(
            "template_write",
            json!({
                "projectId": project,
                "name": "Bug report",
                "title": "Fix this",
                "description": "What happened, and what should have.",
                "priority": "high",
                "labels": ["bug"],
                "acceptanceCriteria": ["Reproduced in a test", "Fixed"],
                "subtasks": [{"title": "Write a failing test"}, {"title": "Fix it", "description": "Smallest change."}]
            }),
        )
        .await;
    assert_eq!(made["value"]["created"], true, "{made}");
    let template = &made["value"]["template"];
    let template_id = template["id"].as_str().expect("template id").to_owned();
    assert_eq!(template["subtasks"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        f.author(
            "SELECT created_by_actor_id FROM task_templates WHERE id = $1",
            &template_id
        )
        .await,
        f.agent
    );

    // The same name again is refused.
    let twin = tool(
        &f.app,
        &f.token,
        &f.session,
        "template_write",
        json!({"projectId": project, "name": "bug report", "title": "Again"}),
    )
    .await;
    assert_eq!(twin["outcome"], "invalid_request", "{twin}");

    // Changing one field keeps the rest.
    let changed = f
        .call(
            "template_write",
            json!({"projectId": project, "template": "Bug report", "priority": "urgent", "expectedVersion": template["version"]}),
        )
        .await;
    let template = &changed["value"]["template"];
    assert_eq!(template["priority"], "urgent", "{changed}");
    assert_eq!(template["title"], "Fix this");
    assert_eq!(template["labels"], json!(["bug"]));
    assert_eq!(template["subtasks"].as_array().map(Vec::len), Some(2));
    // An old version is a conflict.
    let stale = tool(
        &f.app,
        &f.token,
        &f.session,
        "template_write",
        json!({"projectId": project, "template": template_id, "title": "Stale", "expectedVersion": made["value"]["template"]["version"]}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // Tasks made from it get its subtasks.
    let task = f
        .call(
            "task_from_template",
            json!({"projectId": project, "template": "Bug report", "title": "Fix: login loop"}),
        )
        .await;
    assert_eq!(task["value"]["subtasks"], 2, "{task}");

    // The person sees the same template in the app.
    let listed = f
        .web(
            "GET",
            &format!("/api/v1/projects/{project}/task-templates"),
            Value::Null,
        )
        .await;
    assert!(
        listed["templates"]
            .as_array()
            .expect("templates")
            .iter()
            .any(|row| row["id"] == template_id.as_str() && row["priority"] == "urgent"),
        "{listed}"
    );

    // Archived, it can't be used until it's restored.
    let archived = f
        .call(
            "template_write",
            json!({"projectId": project, "template": "Bug report", "archived": true}),
        )
        .await;
    assert_eq!(
        archived["value"]["template"]["archived"], true,
        "{archived}"
    );
    let refused = tool(
        &f.app,
        &f.token,
        &f.session,
        "task_from_template",
        json!({"projectId": project, "template": "Bug report"}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    f.call(
        "template_write",
        json!({"projectId": project, "template": template_id, "archived": false}),
    )
    .await;
    f.call(
        "task_from_template",
        json!({"projectId": project, "template": "Bug report"}),
    )
    .await;

    f.close().await;
}

/// AWD-111: an agent schedules a template to make tasks, changes and
/// pauses the schedule, and deletes it. The schedule acts as the agent.
#[tokio::test]
async fn an_agent_schedules_recurring_tasks() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();
    f.call(
        "template_write",
        json!({"projectId": project, "name": "Weekly review", "title": "Review the week"}),
    )
    .await;

    // A schedule needs a template and a frequency, and starts running.
    for arguments in [
        json!({"projectId": project, "template": "Weekly review"}),
        json!({"projectId": project, "template": "Weekly review", "frequency": "daily", "enabled": false}),
    ] {
        let refused = tool(&f.app, &f.token, &f.session, "recurring_write", arguments).await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    }
    let none = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recurring_tasks")
        .fetch_one(&f.pool)
        .await
        .expect("schedules");
    assert_eq!(none, 0);

    let made = f
        .call(
            "recurring_write",
            json!({
                "projectId": project,
                "template": "Weekly review",
                "frequency": "weekly",
                "weekdays": [5],
                "timeOfDay": "16:30",
                "timezone": "Europe/Amsterdam",
                "assignee": "me",
                "dueInDays": 2
            }),
        )
        .await;
    assert_eq!(made["value"]["created"], true, "{made}");
    let schedule = &made["value"]["recurring"];
    let schedule_id = schedule["id"].as_str().expect("schedule id").to_owned();
    assert_eq!(schedule["weekdays"], json!([5]), "{made}");
    assert_eq!(schedule["assigneeActorId"], f.agent.to_string());
    assert!(schedule["nextRunAt"].is_string(), "{made}");
    // It acts as the agent that set it up.
    assert_eq!(
        f.author(
            "SELECT created_by_actor_id FROM recurring_tasks WHERE id = $1",
            &schedule_id
        )
        .await,
        f.agent
    );

    // Changing the time keeps the day; an old version is a conflict.
    let changed = f
        .call(
            "recurring_write",
            json!({"projectId": project, "recurring": schedule_id, "timeOfDay": "08:00", "clearDueInDays": true}),
        )
        .await;
    let schedule = &changed["value"]["recurring"];
    assert_eq!(schedule["timeOfDay"], "08:00", "{changed}");
    assert_eq!(schedule["weekdays"], json!([5]), "{changed}");
    assert_eq!(schedule["timezone"], "Europe/Amsterdam");
    assert!(schedule["dueInDays"].is_null(), "{changed}");
    let stale = tool(
        &f.app,
        &f.token,
        &f.session,
        "recurring_write",
        json!({"projectId": project, "recurring": schedule_id, "every": 2, "expectedVersion": made["value"]["recurring"]["version"]}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // Paused, it has no next run; archiving its template keeps it paused.
    let paused = f
        .call(
            "recurring_write",
            json!({"projectId": project, "recurring": schedule_id, "enabled": false}),
        )
        .await;
    assert_eq!(paused["value"]["recurring"]["enabled"], false, "{paused}");
    assert!(paused["value"]["recurring"]["nextRunAt"].is_null());
    f.call(
        "template_write",
        json!({"projectId": project, "template": "Weekly review", "archived": true}),
    )
    .await;
    let refused = tool(
        &f.app,
        &f.token,
        &f.session,
        "recurring_write",
        json!({"projectId": project, "recurring": schedule_id, "enabled": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    // The person sees it in the app; then the agent deletes it.
    let listed = f
        .web(
            "GET",
            &format!("/api/v1/projects/{project}/task-templates"),
            Value::Null,
        )
        .await;
    assert!(
        listed["recurring"]
            .as_array()
            .expect("recurring")
            .iter()
            .any(|row| row["id"] == schedule_id.as_str()),
        "{listed}"
    );
    let deleted = f
        .call(
            "recurring_write",
            json!({"projectId": project, "recurring": schedule_id, "delete": true}),
        )
        .await;
    assert_eq!(deleted["value"]["deleted"], true, "{deleted}");
    let gone = tool(
        &f.app,
        &f.token,
        &f.session,
        "recurring_write",
        json!({"projectId": project, "recurring": schedule_id, "enabled": false}),
    )
    .await;
    assert_eq!(gone["outcome"], "not_found", "{gone}");

    f.close().await;
}

#[tokio::test]
async fn an_agent_forwards_messages_and_removes_files() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();
    let channel = f.seed.channel_id.to_string();
    let posted = f
        .web(
            "POST",
            &format!("/api/v1/channels/{channel}/messages"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "The release moves to Thursday."}]}}),
        )
        .await;
    let original = string(&posted, "/message/id");
    let thread = f
        .call(
            "thread_start",
            json!({"conversationId": channel, "name": "Release notes", "text": "Collecting notes."}),
        )
        .await;
    let thread_id = string(&thread, "/value/id");

    // Forwarding: quoted where it goes, sent as the agent, once per request.
    let arguments = json!({
        "messageId": original,
        "conversationId": thread_id,
        "text": "For the notes",
        "clientRequestId": "awd111-forward"
    });
    let forwarded = f.call("message_forward", arguments.clone()).await;
    let forward_id = string(&forwarded, "/value/messageId");
    assert_eq!(forwarded["value"]["idempotentReplay"], false, "{forwarded}");
    assert_eq!(
        f.author(
            "SELECT author_actor_id FROM messages WHERE id = $1",
            &forward_id
        )
        .await,
        f.agent
    );
    assert_eq!(
        f.author(
            "SELECT forwarded_message_id FROM messages WHERE id = $1",
            &forward_id
        )
        .await,
        original.parse::<Uuid>().expect("original id")
    );
    let again = f.call("message_forward", arguments).await;
    assert_eq!(again["value"]["idempotentReplay"], true, "{again}");
    assert_eq!(string(&again, "/value/messageId"), forward_id);
    let read = f
        .call("conversation_read", json!({"conversationId": thread_id}))
        .await;
    let shown = read["value"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["messageId"] == forward_id.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("the forward: {read}"));
    assert_eq!(shown["text"], "For the notes", "{shown}");
    assert_eq!(
        shown["forwarded"]["text"], "The release moves to Thursday.",
        "{shown}"
    );

    // An agent's reply quotes what it answers, as a person's does.
    let reply = f
        .call(
            "message_send",
            json!({"conversationId": channel, "text": "Noted", "replyToMessageId": original}),
        )
        .await;
    let reply_id = string(&reply, "/value/messageId");
    let read = f
        .call("conversation_read", json!({"conversationId": channel}))
        .await;
    let replied = read["value"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["messageId"] == reply_id.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("the reply: {read}"));
    assert_eq!(
        replied["replyTo"]["text"], "The release moves to Thursday.",
        "{replied}"
    );

    // Not into the conversation it's already in.
    let same = tool(
        &f.app,
        &f.token,
        &f.session,
        "message_forward",
        json!({"messageId": original, "conversationId": channel}),
    )
    .await;
    assert_eq!(same["outcome"], "invalid_request", "{same}");

    // Removing files: an upload it hasn't sent, and a task's own file.
    let upload = f
        .call(
            "file_upload",
            json!({"conversationId": channel, "name": "draft.txt", "text": "Draft"}),
        )
        .await;
    let upload_id = string(&upload, "/value/fileId");
    let removed = f.call("file_delete", json!({"fileId": upload_id})).await;
    assert_eq!(removed["value"]["removed"], true, "{removed}");
    assert_eq!(
        f.author(
            "SELECT removed_by_actor_id FROM files WHERE id = $1",
            &upload_id
        )
        .await,
        f.agent
    );
    let gone = tool(
        &f.app,
        &f.token,
        &f.session,
        "file_delete",
        json!({"fileId": upload_id}),
    )
    .await;
    assert_eq!(gone["outcome"], "not_found", "{gone}");

    let task = f
        .call(
            "task_create",
            json!({"projectId": project, "title": "Ship the notes"}),
        )
        .await;
    let key = string(&task, "/key");
    let on_task = f
        .call(
            "file_upload",
            json!({"taskId": key, "name": "notes.txt", "text": "Notes"}),
        )
        .await;
    let task_file = string(&on_task, "/value/fileId");
    f.call("file_delete", json!({"fileId": task_file})).await;
    let listed = f.call("file_list", json!({"taskId": key})).await;
    assert!(!listed.to_string().contains(&task_file), "{listed}");

    // A file sent in a message goes with the message, not on its own.
    let sent_upload = f
        .call(
            "file_upload",
            json!({"conversationId": channel, "name": "sent.txt", "text": "Sent"}),
        )
        .await;
    let sent_file = string(&sent_upload, "/value/fileId");
    f.call(
        "message_send",
        json!({"conversationId": channel, "attachmentIds": [sent_file]}),
    )
    .await;
    let kept = tool(
        &f.app,
        &f.token,
        &f.session,
        "file_delete",
        json!({"fileId": sent_file}),
    )
    .await;
    assert_eq!(kept["outcome"], "permission_denied", "{kept}");
    f.close().await;
}

#[tokio::test]
async fn people_retry_an_agents_reply_and_agents_empty_the_whiteboard_trash() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();
    let channel = f.seed.channel_id.to_string();

    // The agent clears out a board it drew, for good.
    let board = f
        .call(
            "whiteboard_create",
            json!({"title": "Scratch", "projectId": project}),
        )
        .await;
    let board_id = string(&board, "/value/whiteboard/id");
    f.call(
        "whiteboard_update",
        json!({"whiteboardId": board_id, "deleted": true}),
    )
    .await;
    let emptied = f
        .call("whiteboard_trash_empty", json!({"projectId": project}))
        .await;
    assert_eq!(emptied["value"]["deleted"], 1, "{emptied}");
    let left = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM whiteboards WHERE id = $1")
        .bind(board_id.parse::<Uuid>().expect("board id"))
        .fetch_one(&f.pool)
        .await
        .expect("boards");
    assert_eq!(left, 0);

    // A person asks the agent; it lets the request go; the person, through
    // their app, asks it again.
    f.web(
        "POST",
        &format!("/api/v1/channels/{channel}/messages"),
        json!({"content": {"version": 1, "nodes": [
            {"type": "mention", "actorId": f.agent.to_string()},
            {"type": "text", "text": " can you check the release notes?"}
        ]}}),
    )
    .await;
    let request = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM reply_requests WHERE agent_actor_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(f.agent)
    .fetch_one(&f.pool)
    .await
    .expect("a reply request for the agent")
    .to_string();
    let claimed = f.call("reply_claim", json!({"requestId": request})).await;
    f.call(
        "reply_release",
        json!({
            "requestId": request,
            "generation": claimed["value"]["request"]["generation"],
            "reason": "Not now."
        }),
    )
    .await;

    // Asking again is a person's call, as on the web.
    let refused = tool(
        &f.app,
        &f.token,
        &f.session,
        "reply_retry",
        json!({"requestId": request}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let app = connect_app(&f.app, &f.browser, "Owner's app", "full").await;
    let session = initialize(&f.app, &app).await.expect("owner MCP");
    let retried = tool(
        &f.app,
        &app,
        &session,
        "reply_retry",
        json!({"requestId": request}),
    )
    .await;
    assert_eq!(retried["outcome"], "ok", "{retried}");
    assert_eq!(retried["value"]["status"], "pending", "{retried}");
    let status = sqlx::query_scalar::<_, String>("SELECT status FROM reply_requests WHERE id = $1")
        .bind(request.parse::<Uuid>().expect("request id"))
        .fetch_one(&f.pool)
        .await
        .expect("request");
    assert_eq!(status, "pending");
    // Only a request that ended can be asked again.
    let again = tool(
        &f.app,
        &app,
        &session,
        "reply_retry",
        json!({"requestId": request}),
    )
    .await;
    assert_eq!(again["outcome"], "conflict", "{again}");
    f.close().await;
}

#[tokio::test]
async fn people_save_project_templates_and_start_projects_from_them() {
    let Some(f) = fixture().await else {
        return;
    };
    let project = f.seed.project_id.to_string();
    let app = connect_app(&f.app, &f.browser, "Owner's app", "full").await;
    let session = initialize(&f.app, &app).await.expect("owner MCP");
    let person = |name: &'static str, arguments: Value| {
        let app = app.clone();
        let session = session.clone();
        let router = f.app.clone();
        async move { tool(&router, &app, &session, name, arguments).await }
    };

    let saved = person(
        "project_template_write",
        json!({"projectId": project, "name": "Release setup", "description": "How we ship"}),
    )
    .await;
    assert_eq!(saved["outcome"], "ok", "{saved}");
    assert_eq!(saved["value"]["done"], "saved", "{saved}");
    assert_eq!(saved["value"]["template"]["canEdit"], true, "{saved}");
    let template_id = string(&saved, "/value/template/id");
    let statuses = saved["value"]["template"]["statuses"].clone();
    assert!(
        !statuses.as_array().expect("statuses").is_empty(),
        "{saved}"
    );
    let again = person(
        "project_template_write",
        json!({"projectId": project, "name": "release setup"}),
    )
    .await;
    assert_eq!(again["outcome"], "conflict", "{again}");

    // Agents read templates, and only read them.
    let listed = f
        .call("project_template_list", json!({"projectId": project}))
        .await;
    let line = &listed["value"]["templates"][0];
    assert_eq!(line["name"], "Release setup", "{listed}");
    assert_eq!(line["canEdit"], false, "{listed}");
    let refused = tool(
        &f.app,
        &f.token,
        &f.session,
        "project_template_write",
        json!({"projectId": project, "template": template_id, "archived": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // A stale version is refused; then the person archives it, and an
    // archived template starts nothing.
    let stale = person(
        "project_template_write",
        json!({"projectId": project, "template": "Release setup", "archived": true, "expectedVersion": 0}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let archived = person(
        "project_template_write",
        json!({"projectId": project, "template": "Release setup", "archived": true}),
    )
    .await;
    assert_eq!(
        archived["value"]["template"]["archived"], true,
        "{archived}"
    );
    let blocked = person(
        "project_create",
        json!({"key": "SHIP", "name": "Ship", "template": template_id}),
    )
    .await;
    assert_eq!(blocked["outcome"], "conflict", "{blocked}");
    let restored = person(
        "project_template_write",
        json!({"projectId": project, "template": "Release setup", "archived": false, "name": "Ship setup"}),
    )
    .await;
    assert_eq!(
        restored["value"]["template"]["name"], "Ship setup",
        "{restored}"
    );
    let audited = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE target_kind = 'project_template' AND channel = 'mcp'",
    )
    .fetch_one(&f.pool)
    .await
    .expect("audit");
    assert_eq!(audited, 3);

    // A project started from it gets its workflow.
    let created = person(
        "project_create",
        json!({"key": "SHIP", "name": "Ship", "template": "Ship setup"}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let new_project =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM projects WHERE project_key = 'SHIP'")
            .fetch_one(&f.pool)
            .await
            .expect("new project");
    let names = sqlx::query_scalar::<_, String>(
        "SELECT name FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position",
    )
    .bind(new_project)
    .fetch_all(&f.pool)
    .await
    .expect("statuses");
    assert_eq!(json!(names), statuses);
    f.close().await;
}

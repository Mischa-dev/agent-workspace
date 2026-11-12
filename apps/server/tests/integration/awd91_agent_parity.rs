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

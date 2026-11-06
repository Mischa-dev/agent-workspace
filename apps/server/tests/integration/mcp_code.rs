//! Phase 4 read tools (docs/plans/mcp-tools.md): repository_list,
//! pull_request_list and agent_list. The costly bugs are silent: a pull
//! request from a private thread listed to the whole project, "needs me"
//! pointing at someone else's work, or a token, webhook or server path in a
//! result an AI client reads.

use std::collections::BTreeMap;

use agent_workspace_server::{
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    seed::{DevSeed, dev_seed},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::{
    agent_interface::{
        Browser, PASSWORD, connect_app, create_agent, initialize, login, schema_pool, send,
        test_app, tool,
    },
    mcp_admin::{Setup, create_project, setup},
    mcp_task_board::seed_pull_request,
    mcp_tasks::{add_member, space},
    message_edits::add_person,
};

/// The dev seed (project AW with a worker, profile and repository) with
/// its person signed in as "owner".
pub(super) async fn seeded(prefix: &str) -> Option<(Setup, DevSeed, tempfile::TempDir)> {
    seeded_through(prefix, i64::MAX).await
}

async fn seeded_through(prefix: &str, version: i64) -> Option<(Setup, DevSeed, tempfile::TempDir)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    let mut migrations = sqlx::migrate!("./migrations");
    migrations
        .migrations
        .to_mut()
        .retain(|migration| migration.version <= version);
    migrations.run(&pool).await.expect("apply migrations");
    let temporary = tempfile::tempdir().expect("temporary root");
    let data_root = temporary.path().join("data");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data_root.join("codex").to_string_lossy().into_owned(),
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
    .expect("bootstrap owner");
    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'authenticated', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("authenticate profile");
    let app = test_app(pool.clone(), &database_url);
    // Signing in needs the current schema (sessions, two-factor), so a
    // database held at an older migration gets no browser session.
    let browser = if version == i64::MAX {
        login(&app).await
    } else {
        Browser {
            cookie: String::new(),
            csrf: String::new(),
        }
    };
    Some((
        Setup {
            app,
            browser,
            organization_id: seed.organization_id,
            pool,
        },
        seed,
        temporary,
    ))
}

async fn execute(pool: &PgPool, sql: &str, binds: &[Uuid]) {
    let mut query = sqlx::query(sql);
    for bind in binds {
        query = query.bind(*bind);
    }
    query
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

/// A thread under `channel` about the seed repository.
pub(super) async fn thread(pool: &PgPool, seed: &DevSeed, channel: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name) VALUES ($1, $2, 'thread', $3, $4, $5)",
    )
    .bind(id)
    .bind(seed.project_id)
    .bind(channel)
    .bind(seed.repository_id)
    .bind(name)
    .execute(pool)
    .await
    .expect("thread");
    id
}

/// A queued run by the seed person in `thread`, with its branch. Returns
/// the run and its workspace.
pub(super) async fn run(pool: &PgPool, seed: &DevSeed, thread: Uuid, branch: &str) -> (Uuid, Uuid) {
    let workspace = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repository_workspaces (id, project_id, repository_id, conversation_id,
                                           branch_name, base_commit)
        VALUES ($1, $2, $3, $4, $5, repeat('b', 40))
        "#,
    )
    .bind(workspace)
    .bind(seed.project_id)
    .bind(seed.repository_id)
    .bind(thread)
    .bind(branch)
    .execute(pool)
    .await
    .expect("workspace");
    let run = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, repository_workspace_id, state, prompt,
            authorization_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued', 'list test', '{}'::jsonb)
        "#,
    )
    .bind(run)
    .bind(thread)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .bind(workspace)
    .execute(pool)
    .await
    .expect("run");
    (run, workspace)
}

/// Another pull request on the same repository and installation as `first`.
async fn pull_request(pool: &PgPool, first: Uuid, number: i64, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO github_pull_requests (
            id, project_id, repository_id, github_installation_id, github_repository_id,
            pull_request_number, node_id, html_url, head_ref, head_sha, base_ref, title,
            author_login, draft, state, external_created_at, external_updated_at
        )
        SELECT $1, project_id, repository_id, github_installation_id, github_repository_id,
               $2, 'PR_' || $1::text,
               regexp_replace(html_url, '/pull/[0-9]+$', '/pull/' || $2::text),
               'branch-' || $2::text, head_sha, base_ref, $3, author_login, false, 'open',
               now() - interval '1 hour', now() - make_interval(mins => $2::int)
        FROM github_pull_requests WHERE id = $4
        "#,
    )
    .bind(id)
    .bind(number)
    .bind(title)
    .bind(first)
    .execute(pool)
    .await
    .expect("pull request");
    id
}

/// What a run that opened `pull_request` leaves behind: the workspace link
/// and the run_artifact link to its thread.
async fn opened_by(
    pool: &PgPool,
    seed: &DevSeed,
    pull_request: Uuid,
    run: (Uuid, Uuid),
    thread: Uuid,
) {
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_workspace_links (
            project_id, github_pull_request_id, repository_workspace_id, agent_run_id, commit_sha
        ) VALUES ($1, $2, $3, $4, repeat('c', 40))
        "#,
    )
    .bind(seed.project_id)
    .bind(pull_request)
    .bind(run.1)
    .bind(run.0)
    .execute(pool)
    .await
    .expect("workspace link");
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_conversation_links (
            project_id, github_pull_request_id, conversation_id, source, created_by_actor_id
        ) VALUES ($1, $2, $3, 'run_artifact', $4)
        "#,
    )
    .bind(seed.project_id)
    .bind(pull_request)
    .bind(thread)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("thread link");
}

async fn link_task(pool: &PgPool, seed: &DevSeed, pull_request: Uuid, task: Uuid) {
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_work_item_links (
            project_id, github_pull_request_id, work_item_id, source, confirmed,
            created_by_actor_id
        ) VALUES ($1, $2, $3, 'explicit', true, $4)
        "#,
    )
    .bind(seed.project_id)
    .bind(pull_request)
    .bind(task)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("task link");
}

async fn task_key(pool: &PgPool, task: Uuid) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT project.project_key || '-' || item.number FROM work_items item JOIN projects project ON project.id = item.project_id WHERE item.id = $1",
    )
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("task key")
}

fn ids(result: &Value) -> Vec<String> {
    result["value"]["pullRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("pull requests: {result}"))
        .iter()
        .map(|line| line["id"].as_str().expect("id").to_owned())
        .collect()
}

fn line(result: &Value, id: Uuid) -> &Value {
    result["value"]["pullRequests"]
        .as_array()
        .and_then(|lines| lines.iter().find(|line| line["id"] == id.to_string()))
        .unwrap_or_else(|| panic!("{id} in {result}"))
}

#[tokio::test]
async fn pull_requests_follow_their_thread_and_reach_the_right_person() {
    let Some((setup, seed, temporary)) = seeded("mcp_code_prs").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let (dana, dana_browser) = add_person(&setup, &[&aw], "dana").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &aw, mara, "member").await;
    let owner_app = connect_app(app, &setup.browser, "Claude Code", "full").await;
    let reader_app = connect_app(app, &setup.browser, "Reader", "read").await;
    let dana_app = connect_app(app, &dana_browser, "Dana Code", "full").await;
    let mut sessions = Vec::new();
    for token in [&owner_app, &reader_app, &dana_app, &mara_token] {
        sessions.push((
            token.clone(),
            initialize(app, token).await.expect("MCP session"),
        ));
    }
    let call = |who: usize, name: &'static str, arguments: Value| {
        let (token, session) = sessions[who].clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    const OWNER: usize = 0;
    const READER: usize = 1;
    const DANA: usize = 2;
    const MARA: usize = 3;

    // repository_list explains why Codex can't run yet.
    let listed = call(OWNER, "repository_list", json!({"projectId": "AW"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let repository = &listed["value"]["repositories"][0];
    assert_eq!(repository["name"], "Mischa-dev/agent-workspace-m0-spike");
    assert_eq!(repository["readyForRuns"], false, "{listed}");
    assert!(
        repository["notReadyBecause"]
            .as_str()
            .is_some_and(|reason| reason.contains("prepare")),
        "{listed}"
    );
    assert_eq!(repository["github"], "not_connected");
    execute(
        pool,
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), version = version + 1 WHERE id = $1",
        &[seed.repository_id],
    )
    .await;

    // A: the owner's run in a public thread, failing "build", on task T1.
    // B: no run; on T2, assigned to Dana; approved, mergeable, fresh, and
    //    asking Mischa-dev to review.
    // C: the owner's run in a channel only the owner is in.
    // D: the owner's run started from T3, with no task link.
    // E: merged.
    let (a, _) = seed_pull_request(&setup, &aw, seed.repository_id, 1).await;
    let installation = sqlx::query_scalar::<_, Uuid>(
        "SELECT github_installation_id FROM github_pull_requests WHERE id = $1",
    )
    .bind(a)
    .fetch_one(pool)
    .await
    .expect("installation");
    sqlx::query(
        r#"
        INSERT INTO github_installation_repositories (
            project_id, github_installation_id, repository_id, github_repository_id,
            access_state, last_verified_at
        ) SELECT project_id, $2, id, github_repository_id, 'selected', now()
          FROM repositories WHERE id = $1
        "#,
    )
    .bind(seed.repository_id)
    .bind(installation)
    .execute(pool)
    .await
    .expect("installation repository");
    let b = pull_request(pool, a, 2, "Approved work").await;
    let c = pull_request(pool, a, 3, "Private work").await;
    let d = pull_request(pool, a, 4, "Task work").await;
    let e = pull_request(pool, a, 5, "Merged work").await;
    execute(
        pool,
        "UPDATE github_pull_requests SET checks_state = 'failure', sync_state = 'fresh' WHERE id = $1",
        &[a],
    )
    .await;
    for (name, status, conclusion) in [
        ("build", "completed", Some("failure")),
        ("lint", "completed", Some("success")),
        ("e2e", "in_progress", None),
    ] {
        sqlx::query(
            r#"
            INSERT INTO github_pull_request_check_runs (
                id, project_id, github_pull_request_id, github_check_run_id, name, status, conclusion
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(seed.project_id)
        .bind(a)
        .bind((Uuid::now_v7().as_u128() % 1_000_000) as i64 + 1)
        .bind(name)
        .bind(status)
        .bind(conclusion)
        .execute(pool)
        .await
        .expect("check run");
    }
    execute(
        pool,
        r#"UPDATE github_pull_requests SET checks_state = 'success', review_state = 'approved',
               mergeability_state = 'ready', sync_state = 'fresh', last_success_at = now(),
               requested_reviewers = '["Mischa-dev"]'::jsonb
           WHERE id = $1"#,
        &[b],
    )
    .await;
    execute(
        pool,
        "UPDATE github_pull_requests SET state = 'merged', merged_at = now(), merge_commit_sha = repeat('d', 40) WHERE id = $1",
        &[e],
    )
    .await;

    let mut tasks = Vec::new();
    for title in ["Fix the build", "Approved change", "Run from a task"] {
        let created = call(
            OWNER,
            "task_create",
            json!({"projectId": "AW", "title": title}),
        )
        .await;
        tasks.push(
            created["workItemId"]
                .as_str()
                .unwrap_or_else(|| panic!("task: {created}"))
                .parse::<Uuid>()
                .expect("task id"),
        );
    }
    let (t1, t2, t3) = (tasks[0], tasks[1], tasks[2]);
    sqlx::query("UPDATE work_items SET assignee_actor_id = $2 WHERE id = $1")
        .bind(t2)
        .bind(dana)
        .execute(pool)
        .await
        .expect("assign Dana");

    let thread_a = thread(pool, &seed, seed.channel_id, "Build fix").await;
    let run_a = run(pool, &seed, thread_a, "codex/a").await;
    opened_by(pool, &seed, a, run_a, thread_a).await;
    link_task(pool, &seed, a, t1).await;
    link_task(pool, &seed, b, t2).await;

    let secret_channel = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name, visibility) VALUES ($1, $2, 'channel', 'secret', 'members')",
    )
    .bind(secret_channel)
    .bind(seed.project_id)
    .execute(pool)
    .await
    .expect("secret channel");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1, $2, $3, $3)",
    )
    .bind(seed.project_id)
    .bind(secret_channel)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("owner in secret");
    let thread_c = thread(pool, &seed, secret_channel, "Secret plan").await;
    let run_c = run(pool, &seed, thread_c, "codex/c").await;
    opened_by(pool, &seed, c, run_c, thread_c).await;

    let thread_d = thread(pool, &seed, seed.channel_id, "Task run").await;
    let run_d = run(pool, &seed, thread_d, "codex/d").await;
    opened_by(pool, &seed, d, run_d, thread_d).await;
    sqlx::query(
        r#"
        INSERT INTO work_item_worker_invocations (
            id, project_id, work_item_id, conversation_id, requesting_actor_id,
            idempotency_key, request_hash, repository_id, agent_run_id
        ) VALUES ($1, $2, $3, $4, $5, 'start-d', sha256('d'::bytea), $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(seed.project_id)
    .bind(t3)
    .bind(thread_d)
    .bind(seed.human_actor_id)
    .bind(seed.repository_id)
    .bind(run_d.0)
    .execute(pool)
    .await
    .expect("invocation");

    // The owner sees every open one, newest first; E is merged.
    let all = call(OWNER, "pull_request_list", json!({})).await;
    assert_eq!(all["outcome"], "ok", "{all}");
    assert_eq!(all["value"]["total"], 4, "{all}");
    let listed = ids(&all);
    for open in [a, b, c, d] {
        assert!(listed.contains(&open.to_string()), "{all}");
    }
    assert!(!listed.contains(&e.to_string()), "{all}");
    let merged = call(OWNER, "pull_request_list", json!({"state": "merged"})).await;
    assert_eq!(ids(&merged), [e.to_string()], "{merged}");
    let first = line(&all, a);
    assert_eq!(first["failingChecks"], json!(["build"]), "{first}");
    assert_eq!(first["pendingChecks"], 1, "{first}");
    assert_eq!(first["runId"], run_a.0.to_string(), "{first}");
    assert_eq!(first["thread"]["id"], thread_a.to_string(), "{first}");
    assert_eq!(
        first["tasks"][0]["key"],
        task_key(pool, t1).await,
        "{first}"
    );
    assert_eq!(first["needsYou"], json!(["checks_failing"]), "{first}");
    assert_eq!(
        line(&all, d)["tasks"][0]["key"],
        task_key(pool, t3).await,
        "{all}"
    );

    // Filters.
    let failing = call(OWNER, "pull_request_list", json!({"checks": "failing"})).await;
    assert_eq!(ids(&failing), [a.to_string()], "{failing}");
    let approved = call(OWNER, "pull_request_list", json!({"review": "approved"})).await;
    assert_eq!(ids(&approved), [b.to_string()], "{approved}");
    let by_task = call(
        OWNER,
        "pull_request_list",
        json!({"taskId": task_key(pool, t1).await}),
    )
    .await;
    assert_eq!(ids(&by_task), [a.to_string()], "{by_task}");
    let by_run = call(
        OWNER,
        "pull_request_list",
        json!({"taskId": task_key(pool, t3).await}),
    )
    .await;
    assert_eq!(ids(&by_run), [d.to_string()], "{by_run}");
    let by_name = call(
        OWNER,
        "pull_request_list",
        json!({"repositoryId": "mischa-dev/agent-workspace-m0-spike"}),
    )
    .await;
    assert_eq!(by_name["value"]["total"], 4, "{by_name}");
    let nowhere = call(
        OWNER,
        "pull_request_list",
        json!({"repositoryId": "Mischa-dev/nothing-here"}),
    )
    .await;
    assert_eq!(nowhere["outcome"], "not_found", "{nowhere}");
    let page = call(OWNER, "pull_request_list", json!({"limit": 3})).await;
    assert_eq!(ids(&page).len(), 3, "{page}");
    let cursor = page["value"]["nextCursor"].as_str().expect("cursor");
    let rest = call(
        OWNER,
        "pull_request_list",
        json!({"limit": 3, "cursor": cursor}),
    )
    .await;
    assert_eq!(ids(&rest).len(), 1, "{rest}");
    assert!(rest["value"].get("nextCursor").is_none(), "{rest}");

    // needsMe: the owner's failing run, and the review request only once
    // their GitHub username is known.
    let needs = call(OWNER, "pull_request_list", json!({"needsMe": true})).await;
    assert_eq!(ids(&needs), [a.to_string()], "{needs}");
    assert!(
        needs["value"]["note"]
            .as_str()
            .is_some_and(|note| note.contains("/settings/account")),
        "{needs}"
    );
    let (status, bad) = send(
        app,
        setup.browser.request(
            "PATCH",
            "/api/v1/account/profile",
            None,
            json!({"githubLogin": "not a login"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    let (status, profile) = send(
        app,
        setup.browser.request(
            "PATCH",
            "/api/v1/account/profile",
            None,
            json!({"githubLogin": "mischa-dev"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    assert_eq!(profile["githubLogin"], "mischa-dev");
    assert_eq!(profile["displayName"], "Owner");
    let needs = call(OWNER, "pull_request_list", json!({"needsMe": true})).await;
    let mut needed = ids(&needs);
    needed.sort();
    let mut expected = vec![a.to_string(), b.to_string()];
    expected.sort();
    assert_eq!(needed, expected, "{needs}");
    assert_eq!(line(&needs, b)["needsYou"], json!(["review_requested"]));
    assert!(needs["value"].get("note").is_none(), "{needs}");

    // Dana: never C, not by listing, by ID or through a task; B is hers
    // (her task) and ready to merge; A isn't hers.
    let dana_all = call(DANA, "pull_request_list", json!({})).await;
    assert_eq!(dana_all["value"]["total"], 3, "{dana_all}");
    assert!(!ids(&dana_all).contains(&c.to_string()), "{dana_all}");
    assert!(!dana_all.to_string().contains("Private work"), "{dana_all}");
    assert!(!dana_all.to_string().contains(&run_c.0.to_string()));
    let hidden = call(
        DANA,
        "pull_request_get",
        json!({"identifier": c.to_string()}),
    )
    .await;
    assert_eq!(hidden["outcome"], "not_found", "{hidden}");
    let dana_needs = call(DANA, "pull_request_list", json!({"needsMe": true})).await;
    assert_eq!(ids(&dana_needs), [b.to_string()], "{dana_needs}");
    assert_eq!(
        line(&dana_needs, b)["needsYou"],
        json!(["ready_to_merge"]),
        "{dana_needs}"
    );
    let dana_repositories = call(DANA, "repository_list", json!({})).await;
    let dana_repository = &dana_repositories["value"]["repositories"][0];
    assert_eq!(
        dana_repository["openPullRequests"], 3,
        "{dana_repositories}"
    );
    // Runs in threads Dana can't see aren't counted for her.
    assert_eq!(dana_repository["activeRuns"], 2, "{dana_repositories}");

    // Read-only apps read; an agent has nothing that needs it.
    let read = call(READER, "pull_request_list", json!({})).await;
    assert_eq!(read["value"]["total"], 4, "{read}");
    let mara_needs = call(MARA, "pull_request_list", json!({"needsMe": true})).await;
    assert_eq!(mara_needs["outcome"], "ok", "{mara_needs}");
    assert!(ids(&mara_needs).is_empty(), "{mara_needs}");
    assert!(mara_needs["value"].get("note").is_none(), "{mara_needs}");
    let mara_all = call(MARA, "pull_request_list", json!({})).await;
    assert!(!ids(&mara_all).contains(&c.to_string()), "{mara_all}");

    // Bad input.
    let draft = call(OWNER, "pull_request_list", json!({"state": "draft"})).await;
    assert_eq!(draft["outcome"], "invalid_request", "{draft}");
    assert_eq!(draft["field"], "state", "{draft}");
    let zero = call(OWNER, "pull_request_list", json!({"limit": 0})).await;
    assert_eq!(zero["field"], "limit", "{zero}");
    let unknown_task = call(OWNER, "pull_request_list", json!({"taskId": "AW-999"})).await;
    assert_eq!(unknown_task["field"], "taskId", "{unknown_task}");

    // Repositories: ready now, connected, and counted for the owner.
    let listed = call(OWNER, "repository_list", json!({})).await;
    let repository = &listed["value"]["repositories"][0];
    assert_eq!(repository["readyForRuns"], true, "{listed}");
    assert!(repository.get("notReadyBecause").is_none(), "{listed}");
    assert_eq!(repository["github"], "connected", "{listed}");
    assert_eq!(repository["openPullRequests"], 4, "{listed}");
    assert_eq!(repository["activeRuns"], 3, "{listed}");
    assert!(
        repository["settingsUrl"]
            .as_str()
            .is_some_and(|url| url.ends_with("/settings?tab=repositories")),
        "{listed}"
    );
    let archived = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url, default_branch,
            mirror_path, trusted, enabled, archived_at, archived_by_actor_id
        ) VALUES ($1, $2, 99, 'Mischa-dev', 'old', 'https://x-access-token:ghs_leak@github.com/Mischa-dev/old.git',
                  'main', '/srv/aw/mirrors/old.git', true, false, now(), $3)
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .fetch_one(pool)
    .await
    .expect("archived repository");
    let without = call(OWNER, "repository_list", json!({})).await;
    assert_eq!(
        without["value"]["repositories"].as_array().map(Vec::len),
        Some(1)
    );
    let with = call(OWNER, "repository_list", json!({"includeArchived": true})).await;
    let old = with["value"]["repositories"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == archived.to_string()))
        .unwrap_or_else(|| panic!("{with}"));
    assert_eq!(old["archived"], true);
    assert_eq!(old["notReadyBecause"], "It's archived.");
    assert_eq!(
        old["cloneUrl"], "https://github.com/Mischa-dev/old.git",
        "{with}"
    );

    // A project Dana isn't in.
    let other = create_project(&setup, "OTHER", "Other").await;
    let refused = call(DANA, "repository_list", json!({"projectId": other})).await;
    assert_eq!(refused["outcome"], "forbidden", "{refused}");

    // Nothing internal ever reaches a result.
    let (mirror_path, installation_id) = sqlx::query_as::<_, (String, Option<i64>)>(
        "SELECT mirror_path, github_app_installation_id FROM repositories WHERE id = $1",
    )
    .bind(seed.repository_id)
    .fetch_one(pool)
    .await
    .expect("seed repository");
    let installation_ids = sqlx::query_scalar::<_, i64>(
        "SELECT installation_id FROM github_installations WHERE project_id = $1",
    )
    .bind(seed.project_id)
    .fetch_all(pool)
    .await
    .expect("installations");
    let data_root = temporary.path().to_string_lossy().into_owned();
    for result in [&all, &with, &listed, &needs, &dana_all, &dana_repositories] {
        let text = result.to_string();
        for secret in [
            mirror_path.as_str(),
            "/srv/aw/mirrors",
            "ghs_leak",
            "x-access-token",
            data_root.as_str(),
            &installation.to_string(),
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        if let Some(id) = installation_id {
            assert!(!text.contains(&id.to_string()), "{id} in {text}");
        }
        for id in &installation_ids {
            assert!(!text.contains(&format!(":{id}")), "{id} in {text}");
        }
    }
}

#[tokio::test]
async fn agent_list_shows_work_and_never_secrets() {
    let Some(setup) = setup("mcp_code_agents").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = space(&setup).await;
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let (_, dana_browser) = add_person(&setup, &[&work], "dana").await;
    let owner_app = connect_app(app, &setup.browser, "Claude Code", "full").await;
    let dana_app = connect_app(app, &dana_browser, "Dana Code", "full").await;
    let mut sessions = Vec::new();
    for token in [&owner_app, &dana_app, &mara_token] {
        sessions.push((
            token.clone(),
            initialize(app, token).await.expect("MCP session"),
        ));
    }
    let call = |who: usize, name: &'static str, arguments: Value| {
        let (token, session) = sessions[who].clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    const OWNER: usize = 0;
    const DANA: usize = 1;
    const MARA: usize = 2;

    let briefed = call(MARA, "briefing", json!({})).await;
    assert_eq!(briefed["outcome"], "ok", "{briefed}");
    let created = call(
        OWNER,
        "task_create",
        json!({"projectId": "WORK", "title": "Offline mode"}),
    )
    .await;
    let task = created["workItemId"].as_str().expect("task").to_owned();
    let claimed = call(
        MARA,
        "task_claim",
        json!({"taskId": task, "note": "Drafting\u{7}the plan"}),
    )
    .await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");
    let dm = call(
        OWNER,
        "conversation_start",
        json!({"people": ["@mara"], "text": "Can you look at the offline plan?"}),
    )
    .await;
    assert_eq!(dm["outcome"], "ok", "{dm}");
    let connection = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM external_agent_connections WHERE actor_id = $1",
    )
    .bind(mara)
    .fetch_one(pool)
    .await
    .expect("connection");
    let (status, hooked) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/external-agents/{connection}/webhook"),
            None,
            json!({"url": "https://hook-secret-host.test/x"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hooked}");

    let owner_list = call(OWNER, "agent_list", json!({})).await;
    assert_eq!(owner_list["outcome"], "ok", "{owner_list}");
    let agent = &owner_list["value"]["agents"][0];
    assert_eq!(agent["actorId"], mara.to_string(), "{owner_list}");
    assert_eq!(agent["handle"], "mara");
    assert_eq!(agent["status"], "active");
    assert!(agent["lastCheckIn"].is_string(), "{owner_list}");
    assert_eq!(agent["claims"][0]["key"], "WORK-1", "{owner_list}");
    assert_eq!(
        agent["claims"][0]["note"], "Draftingthe plan",
        "{owner_list}"
    );
    assert_eq!(agent["replyRequests"]["waiting"], 1, "{owner_list}");
    let connection_view = &agent["connection"];
    assert_eq!(connection_view["wakeups"], true, "{owner_list}");
    assert_eq!(connection_view["connectsThrough"], json!(["token"]));
    assert_eq!(connection_view["access"], "full");
    assert_eq!(connection_view["canRequestRuns"], false);
    assert!(
        connection_view["manageUrl"]
            .as_str()
            .is_some_and(|url| url.ends_with("/messages/agents"))
    );
    let work_project = agent["projects"]
        .as_array()
        .and_then(|projects| projects.iter().find(|project| project["key"] == "WORK"))
        .unwrap_or_else(|| panic!("{owner_list}"));
    assert_eq!(work_project["role"], "member", "{owner_list}");

    // Dana shares WORK with Mara but isn't in the DM and doesn't manage
    // agents: work, yes; how it connects, no.
    let dana_list = call(DANA, "agent_list", json!({})).await;
    let dana_agent = &dana_list["value"]["agents"][0];
    assert_eq!(dana_agent["actorId"], mara.to_string(), "{dana_list}");
    assert_eq!(dana_agent["replyRequests"]["waiting"], 0, "{dana_list}");
    assert_eq!(dana_agent["claims"][0]["key"], "WORK-1", "{dana_list}");
    assert!(dana_agent.get("connection").is_none(), "{dana_list}");
    for project in dana_agent["projects"].as_array().expect("projects") {
        assert!(project.get("role").is_none(), "{dana_list}");
    }
    let searched = call(DANA, "agent_list", json!({"query": "@mar"})).await;
    assert_eq!(
        searched["value"]["agents"].as_array().map(Vec::len),
        Some(1)
    );
    let none = call(DANA, "agent_list", json!({"query": "nobody"})).await;
    assert_eq!(none["value"]["agents"], json!([]), "{none}");

    // No token, prefix, webhook or credential ID for anyone.
    let credentials = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM mcp_client_credentials WHERE external_agent_connection_id = $1",
    )
    .bind(connection)
    .fetch_all(pool)
    .await
    .expect("credentials");
    for result in [&owner_list, &dana_list] {
        let text = result.to_string();
        assert!(!text.contains(&mara_token), "{text}");
        assert!(!text.contains(&mara_token[..16]), "{text}");
        assert!(!text.contains("hook-secret-host"), "{text}");
        assert!(!text.contains(&connection.to_string()), "{text}");
        for credential in &credentials {
            assert!(!text.contains(&credential.to_string()), "{text}");
        }
    }

    // An agent never sees itself; a turned-off one shows only when asked.
    let own = call(MARA, "agent_list", json!({})).await;
    assert_eq!(own["outcome"], "ok", "{own}");
    assert!(!own.to_string().contains(&mara.to_string()), "{own}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM external_agent_connections WHERE id = $1",
    )
    .bind(connection)
    .fetch_one(pool)
    .await
    .expect("version");
    let (status, disabled) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{space}/external-agents/{connection}"),
            None,
            json!({"expectedVersion": version, "enabled": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    let after = call(OWNER, "agent_list", json!({})).await;
    assert_eq!(after["value"]["agents"], json!([]), "{after}");
    let with_disabled = call(OWNER, "agent_list", json!({"includeDisabled": true})).await;
    assert_eq!(
        with_disabled["value"]["agents"][0]["status"], "disabled",
        "{with_disabled}"
    );
    let too_many = call(OWNER, "agent_list", json!({"limit": 101})).await;
    assert_eq!(too_many["field"], "limit", "{too_many}");
}

/// Codex runs and approvals through MCP for people. The costly bugs: an agent
/// or a read-only app spending someone's Codex, runs from a private thread
/// showing up elsewhere, or a person answering an approval that needs to be
/// seen in full in the web app.
#[tokio::test]
async fn people_start_and_follow_runs_and_answer_approvals() {
    let Some((setup, seed, _temporary)) = seeded("mcp_runs").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let (dana, dana_browser) = add_person(&setup, &[&aw], "dana").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &aw, mara, "member").await;
    let owner_app = connect_app(app, &setup.browser, "Claude Code", "full").await;
    let reader_app = connect_app(app, &setup.browser, "Reader", "read").await;
    let dana_app = connect_app(app, &dana_browser, "Dana Code", "full").await;
    let mut sessions = Vec::new();
    for token in [&owner_app, &reader_app, &dana_app, &mara_token] {
        sessions.push((
            token.clone(),
            initialize(app, token).await.expect("MCP session"),
        ));
    }
    let call = |who: usize, name: &'static str, arguments: Value| {
        let (token, session) = sessions[who].clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    const OWNER: usize = 0;
    const READER: usize = 1;
    const DANA: usize = 2;
    const MARA: usize = 3;
    execute(
        pool,
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1",
        &[seed.repository_id],
    )
    .await;
    let work = thread(pool, &seed, seed.channel_id, "Fix the mirrors").await;

    // Agents and read-only apps can't spend Codex.
    for who in [MARA, READER] {
        let refused = call(
            who,
            "run_start",
            json!({"threadId": work, "prompt": "Fix it"}),
        )
        .await;
        assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    }
    // A person with no signed-in account of their own is told so, not asked
    // to retry later.
    let no_account = call(
        DANA,
        "run_start",
        json!({"threadId": work, "prompt": "Fix it"}),
    )
    .await;
    assert_eq!(no_account["outcome"], "conflict", "{no_account}");
    assert!(
        no_account["message"]
            .as_str()
            .is_some_and(|message| message.contains("signed-in provider account")),
        "{no_account}"
    );

    // A person starts a run on their own account; a retry doesn't start a second.
    let started = call(
        OWNER,
        "run_start",
        json!({"threadId": work, "prompt": "Make private repositories prepare", "clientRequestId": "run-1"}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    assert_eq!(started["value"]["stage"], "queued", "{started}");
    let run_id = started["value"]["id"].as_str().expect("run").to_owned();
    let again = call(
        OWNER,
        "run_start",
        json!({"threadId": work, "prompt": "Make private repositories prepare", "clientRequestId": "run-1"}),
    )
    .await;
    assert_eq!(again["value"]["id"], run_id.as_str(), "{again}");
    let via = sqlx::query_scalar::<_, bool>(
        "SELECT via_oauth_grant_id IS NOT NULL FROM messages WHERE conversation_id = $1",
    )
    .bind(work)
    .fetch_one(pool)
    .await
    .expect("prompt message");
    assert!(via, "the prompt is labelled with the app it came through");

    // Following it: run_get and run_list.
    // The test scheduler picks the run up at once and, with no Codex here,
    // fails it: run_get says so plainly.
    let got = call(DANA, "run_get", json!({"runId": run_id})).await;
    assert_eq!(got["outcome"], "ok", "{got}");
    assert!(
        ["queued", "working", "failed"]
            .contains(&got["value"]["stage"].as_str().unwrap_or_default()),
        "{got}"
    );
    let mine = call(OWNER, "run_list", json!({"mine": true, "state": "all"})).await;
    assert!(ids_of(&mine["value"]["runs"]).contains(&run_id), "{mine}");

    let dana_mine = call(DANA, "run_list", json!({"mine": true, "state": "all"})).await;
    assert!(
        !ids_of(&dana_mine["value"]["runs"]).contains(&run_id),
        "mine means runs you started: {dana_mine}"
    );

    // An approval Codex is waiting on: Dana (member) can answer it, the
    // person who started the run can't answer their own, and one that needs a
    // person stays in the web app.
    let asking = thread(pool, &seed, seed.channel_id, "Which branch").await;
    let (blocked_run, _) = run(pool, &seed, asking, "which-branch").await;
    for next in ["starting", "working", "blocked"] {
        sqlx::query("UPDATE agent_runs SET state = $2 WHERE id = $1")
            .bind(blocked_run)
            .bind(next)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("run to {next}: {error}"));
    }
    let approval = Uuid::now_v7();
    let human = Uuid::now_v7();
    for (id, human_required) in [(approval, false), (human, true)] {
        sqlx::query(
            r#"
            INSERT INTO agent_approvals (
                id, agent_run_id, requesting_actor_id, eligible_role, human_required,
                kind, summary, options, provider_request_id
            ) VALUES ($1, $2, $3, 'member', $4, 'question', 'Which branch?',
                      '["main", "develop"]'::jsonb, 'test-' || $1::text)
            "#,
        )
        .bind(id)
        .bind(blocked_run)
        .bind(seed.human_actor_id)
        .bind(human_required)
        .execute(pool)
        .await
        .expect("approval");
    }
    let waiting = call(DANA, "approval_list", json!({})).await;
    assert_eq!(waiting["outcome"], "ok", "{waiting}");
    let listed = ids_of(&waiting["value"]["approvals"]);
    assert!(listed.contains(&approval.to_string()), "{waiting}");
    assert!(!listed.contains(&human.to_string()), "{waiting}");
    assert_eq!(waiting["value"]["inWebApp"], 1, "{waiting}");
    let own = call(OWNER, "approval_list", json!({})).await;
    assert!(
        !ids_of(&own["value"]["approvals"]).contains(&approval.to_string()),
        "no answering approvals on your own run: {own}"
    );
    let agents = call(MARA, "approval_list", json!({})).await;
    assert_eq!(agents["outcome"], "permission_denied", "{agents}");
    let in_web = call(DANA, "approval_get", json!({"approvalId": human})).await;
    assert_ne!(in_web["outcome"], "found", "{in_web}");
    let read_only = call(
        READER,
        "approval_respond",
        json!({"approvalId": approval, "expectedVersion": 1, "answer": "main", "clientRequestId": "a-0"}),
    )
    .await;
    assert_ne!(read_only["outcome"], "applied", "{read_only}");
    let answered = call(
        DANA,
        "approval_respond",
        json!({"approvalId": approval, "expectedVersion": 1, "answer": "main", "clientRequestId": "a-1"}),
    )
    .await;
    assert_eq!(answered["outcome"], "applied", "{answered}");
    let audited = sqlx::query_scalar::<_, bool>(
        "SELECT mcp_oauth_grant_id IS NOT NULL FROM audit_events WHERE action = 'approval.external_responded' AND target_id = $1",
    )
    .bind(approval)
    .fetch_one(pool)
    .await
    .expect("audit");
    assert!(audited, "the answer records the app it came through");
    let _ = dana;
}

/// The browser's run controls follow the thread: a project member outside
/// a private channel can't interrupt a run in one of its threads or answer
/// its approval (the response would show the run and its pull request), and
/// the refusal reads like a run that doesn't exist.
#[tokio::test]
async fn run_controls_stay_inside_a_private_thread() {
    let Some((setup, seed, _temporary)) = seeded("web_run_visibility").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let (dana, dana_browser) = add_person(&setup, &[&aw], "dana").await;
    execute(
        pool,
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1",
        &[seed.repository_id],
    )
    .await;

    let secret_channel = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name, visibility) VALUES ($1, $2, 'channel', 'secret', 'members')",
    )
    .bind(secret_channel)
    .bind(seed.project_id)
    .execute(pool)
    .await
    .expect("secret channel");
    let join = "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1, $2, $3, $3)";
    execute(
        pool,
        join,
        &[seed.project_id, secret_channel, seed.human_actor_id],
    )
    .await;
    let secret = thread(pool, &seed, secret_channel, "Secret plan").await;
    let (blocked_run, _) = run(pool, &seed, secret, "codex/secret").await;
    for next in ["starting", "working", "blocked"] {
        sqlx::query("UPDATE agent_runs SET state = $2 WHERE id = $1")
            .bind(blocked_run)
            .bind(next)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("run to {next}: {error}"));
    }
    let approval = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, eligible_role, human_required,
            kind, summary, options, provider_request_id
        ) VALUES ($1, $2, $3, 'member', false, 'question', 'Which branch?',
                  '["main", "develop"]'::jsonb, 'test-' || $1::text)
        "#,
    )
    .bind(approval)
    .bind(blocked_run)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("approval");

    let interrupt = |browser: &Browser, key: &str| {
        browser.request(
            "POST",
            &format!("/api/v1/agent-runs/{blocked_run}/interrupt"),
            Some(key),
            Value::Null,
        )
    };
    let respond = |browser: &Browser, key: &str| {
        browser.request(
            "POST",
            &format!("/api/v1/agent-approvals/{approval}/respond"),
            Some(key),
            json!({"answer": "main"}),
        )
    };

    // Dana is in the project but not the channel: both read as missing.
    let (status, body) = send(app, interrupt(&dana_browser, "dana-interrupt-1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (missing_status, missing) = send(
        app,
        dana_browser.request(
            "POST",
            &format!("/api/v1/agent-runs/{}/interrupt", Uuid::now_v7()),
            Some("dana-interrupt-missing"),
            Value::Null,
        ),
    )
    .await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(body["error"], missing["error"], "{body} vs {missing}");
    let (status, body) = send(app, respond(&dana_browser, "dana-respond-1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (state, approval_status) = sqlx::query_as::<_, (String, String)>(
        "SELECT run.state, approval.status FROM agent_runs run JOIN agent_approvals approval ON approval.agent_run_id = run.id WHERE approval.id = $1",
    )
    .bind(approval)
    .fetch_one(pool)
    .await
    .expect("run and approval");
    assert_eq!(
        (state.as_str(), approval_status.as_str()),
        ("blocked", "pending")
    );

    // Once Dana is in the channel, the same calls reach the run.
    execute(pool, join, &[seed.project_id, secret_channel, dana]).await;
    let (status, body) = send(app, respond(&dana_browser, "dana-respond-2")).await;
    assert_ne!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = send(app, interrupt(&dana_browser, "dana-interrupt-2")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

fn ids_of(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn an_agent_cannot_read_or_answer_an_approval_in_a_hidden_thread() {
    let Some((setup, seed, _temporary)) = seeded("mcp_hidden_approval").await else {
        return;
    };
    let app = &setup.app;
    let (status, agent) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{}/external-agents", seed.project_id),
            None,
            json!({"connectionName": "Approver", "displayName": "Approver",
                   "handle": "approver", "role": "member",
                   "scopes": ["read", "write", "approval_respond"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{agent}");
    let token = agent["oneTimeCredential"]["token"].as_str().expect("token");
    let session = initialize(app, token).await.expect("session");
    let private_channel = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, project_id, kind, name, visibility) VALUES ($1, $2, 'channel', 'private', 'members')")
        .bind(private_channel).bind(seed.project_id).execute(&setup.pool).await.expect("private channel");
    let private_thread = thread(&setup.pool, &seed, private_channel, "Private work").await;
    execute(&setup.pool, "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1", &[seed.repository_id]).await;
    let (run_id, _) = run(&setup.pool, &seed, private_thread, "private-approval").await;
    for next in ["starting", "working", "blocked"] {
        sqlx::query("UPDATE agent_runs SET state = $2 WHERE id = $1")
            .bind(run_id)
            .bind(next)
            .execute(&setup.pool)
            .await
            .expect("run state");
    }
    let approval = Uuid::now_v7();
    sqlx::query("INSERT INTO agent_approvals (id, agent_run_id, requesting_actor_id, eligible_role, human_required, kind, summary, options, provider_request_id) VALUES ($1, $2, $3, 'member', false, 'question', 'Private release secret', '[\"main\"]'::jsonb, 'private-approval')")
        .bind(approval).bind(run_id).bind(seed.human_actor_id).execute(&setup.pool).await.expect("approval");
    let read = tool(
        app,
        token,
        &session,
        "approval_get",
        json!({"approvalId": approval}),
    )
    .await;
    let write = tool(app, token, &session, "approval_respond", json!({
        "approvalId": approval, "expectedVersion": 1, "answer": "main", "clientRequestId": "hidden-answer"
    })).await;
    assert_eq!(
        read["outcome"], "not_found",
        "private approval leaked: {read}; write: {write}"
    );
    assert_eq!(write["outcome"], "not_found", "{write}");
    let status: String = sqlx::query_scalar("SELECT status FROM agent_approvals WHERE id = $1")
        .bind(approval)
        .fetch_one(&setup.pool)
        .await
        .expect("approval state");
    assert_eq!(status, "pending");
}

#[tokio::test]
async fn legacy_bearer_run_starts_obey_the_mcp_run_cap() {
    use agent_workspace_server::mcp::{IssueCredentialCommand, issue_credential};
    let Some((setup, seed, _temporary)) = seeded("mcp_bearer_run_cap").await else {
        return;
    };
    execute(&setup.pool, "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1", &[seed.repository_id]).await;
    for index in 0..5 {
        let conversation = thread(
            &setup.pool,
            &seed,
            seed.channel_id,
            &format!("Busy {index}"),
        )
        .await;
        run(&setup.pool, &seed, conversation, &format!("busy-{index}")).await;
    }
    let next = thread(&setup.pool, &seed, seed.channel_id, "Sixth run").await;
    let issued = issue_credential(
        &setup.pool,
        IssueCredentialCommand {
            actor_id: seed.human_actor_id,
            project_id: seed.project_id,
            client_name: "Legacy app".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("credential");
    let session = initialize(&setup.app, &issued.token)
        .await
        .expect("session");
    let result = tool(
        &setup.app,
        &issued.token,
        &session,
        "run_start",
        json!({
            "threadId": next, "prompt": "Do more work", "clientRequestId": "sixth"
        }),
    )
    .await;
    assert_eq!(
        result["outcome"], "conflict",
        "bearer app bypassed the cap: {result}"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE conversation_id = $1")
            .bind(next)
            .fetch_one(&setup.pool)
            .await
            .expect("new runs");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn run_start_rechecks_visibility_after_waiting_for_conversation_locks() {
    let Some((setup, seed, _temporary)) = seeded("mcp_run_revoked").await else {
        return;
    };
    execute(&setup.pool, "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1", &[seed.repository_id]).await;
    let channel = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, project_id, kind, name, visibility) VALUES ($1, $2, 'channel', 'private', 'members')")
        .bind(channel).bind(seed.project_id).execute(&setup.pool).await.expect("channel");
    sqlx::query("INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1, $2, $3, $3)")
        .bind(seed.project_id).bind(channel).bind(seed.human_actor_id).execute(&setup.pool).await.expect("member");
    let work = thread(&setup.pool, &seed, channel, "Private run").await;
    let token = connect_app(&setup.app, &setup.browser, "Codex", "full").await;
    let session = initialize(&setup.app, &token).await.expect("session");
    let mut removal = setup.pool.begin().await.expect("removal");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *removal)
        .await
        .expect("pid");
    sqlx::query("SELECT id FROM conversations WHERE id = $1 FOR UPDATE")
        .bind(channel)
        .execute(&mut *removal)
        .await
        .expect("lock parent");
    sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND actor_id = $2")
        .bind(channel)
        .bind(seed.human_actor_id)
        .execute(&mut *removal)
        .await
        .expect("remove member");
    let app = setup.app.clone();
    let pending = tokio::spawn(async move {
        tool(
            &app,
            &token,
            &session,
            "run_start",
            json!({"threadId": work, "prompt": "Should never run", "clientRequestId": "revoked"}),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&setup.pool).await.expect("waiter");
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("start waits for removal");
    removal.commit().await.expect("revoke access");
    let result = pending.await.expect("start result");
    assert_eq!(result["outcome"], "not_found", "{result}");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE conversation_id = $1")
            .bind(work)
            .fetch_one(&setup.pool)
            .await
            .expect("runs");
    assert_eq!(
        count, 0,
        "a run started after its caller lost conversation access"
    );
}

#[tokio::test]
async fn concurrent_run_starts_in_one_thread_do_not_deadlock() {
    let Some((setup, seed, _temporary)) = seeded("mcp_run_deadlock").await else {
        return;
    };
    execute(&setup.pool, "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1", &[seed.repository_id]).await;
    let work = thread(&setup.pool, &seed, seed.channel_id, "Concurrent runs").await;
    let token = connect_app(&setup.app, &setup.browser, "Codex", "full").await;
    let sessions = [
        initialize(&setup.app, &token).await.expect("first session"),
        initialize(&setup.app, &token)
            .await
            .expect("second session"),
    ];
    // Hold the per-person cap lock until both commands have entered their
    // conversation guard. Shared conversation locks would then deadlock on
    // the first command's UPDATE while the second waits for the cap lock.
    let mut blocker = setup.pool.begin().await.expect("blocker");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("pid");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('run_cap:' || $1::text, 0))")
        .bind(seed.human_actor_id)
        .execute(&mut *blocker)
        .await
        .expect("cap lock");
    let mut pending = Vec::new();
    for (index, session) in sessions.into_iter().enumerate() {
        let app = setup.app.clone();
        let token = token.clone();
        pending.push(tokio::spawn(async move {
            tool(&app, &token, &session, "run_start", json!({"threadId": work, "prompt": "Start once", "clientRequestId": format!("concurrent-{index}")})).await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiters: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) OR EXISTS (SELECT 1 FROM pg_stat_activity first_waiter WHERE $1 = ANY(pg_blocking_pids(first_waiter.pid)) AND first_waiter.pid = ANY(pg_blocking_pids(pg_stat_activity.pid)))")
                .bind(pid).fetch_one(&setup.pool).await.expect("waiters");
            if waiters == 2 { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("both starts queued");
    blocker.commit().await.expect("release cap");
    for task in pending {
        let result = task.await.expect("run start result");
        assert!(
            matches!(result["outcome"].as_str(), Some("ok" | "conflict")),
            "run start deadlocked: {result}"
        );
    }
}

#[tokio::test]
async fn removing_a_run_prompt_message_scrubs_the_run_and_its_retries() {
    let Some((setup, seed, _temporary)) = seeded("mcp_prompt_removal").await else {
        return;
    };
    execute(&setup.pool, "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1", &[seed.repository_id]).await;
    let work = thread(&setup.pool, &seed, seed.channel_id, "Remove prompt").await;
    let token = connect_app(&setup.app, &setup.browser, "Codex", "full").await;
    let session = initialize(&setup.app, &token).await.expect("session");
    let secret = "secret-copied-into-run-prompt";
    let started = tool(
        &setup.app,
        &token,
        &session,
        "run_start",
        json!({"threadId": work, "prompt": secret, "clientRequestId": "sensitive-run"}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    let run_id = started["value"]["id"]
        .as_str()
        .expect("run")
        .parse::<Uuid>()
        .expect("uuid");
    // The fixture repository has no checkout; admission fails naturally.
    // Do not race the scheduler by forcing a competing state transition.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let state: String = sqlx::query_scalar("SELECT state FROM agent_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&setup.pool)
                .await
                .expect("run state");
            if state == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture run fails admission");
    let (status, retried) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/agent-runs/{run_id}/retry"),
            Some("retry-before-removal"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retried}");
    let prompt: Uuid = sqlx::query_scalar(
        "SELECT id FROM messages WHERE conversation_id = $1 AND plain_text = $2",
    )
    .bind(work)
    .bind(secret)
    .fetch_one(&setup.pool)
    .await
    .expect("prompt message");
    let edited = tool(
        &setup.app,
        &token,
        &session,
        "message_edit",
        json!({"messageId": prompt, "text": "Safe replacement", "clientRequestId": "edit-prompt"}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let removed = tool(
        &setup.app,
        &token,
        &session,
        "message_delete",
        json!({"messageId": prompt}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    assert_eq!(
        super::message_edits::copies(&setup.pool, "agent_runs", secret).await,
        0,
        "the copied prompt survives message removal"
    );
    let replay = tool(
        &setup.app,
        &token,
        &session,
        "run_start",
        json!({"threadId": work, "prompt": secret, "clientRequestId": "sensitive-run"}),
    )
    .await;
    assert_eq!(
        replay["outcome"], "conflict",
        "the removed prompt hash must be scrubbed: {replay}"
    );
    let got = tool(
        &setup.app,
        &token,
        &session,
        "run_get",
        json!({"runId": run_id}),
    )
    .await;
    assert_eq!(got["outcome"], "ok", "{got}");
    assert!(!got.to_string().contains(secret), "{got}");
    let (status, retry) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/agent-runs/{run_id}/retry"),
            Some("retry-after-removal"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a removed prompt cannot be executed: {retry}"
    );
}

#[tokio::test]
async fn removing_a_sponsored_prompt_scrubs_the_execution_request_too() {
    let Some((setup, seed, _temporary)) = seeded("mcp_sponsored_redact").await else {
        return;
    };
    execute(&setup.pool, "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1", &[seed.repository_id]).await;
    let work = thread(&setup.pool, &seed, seed.channel_id, "Sponsored prompt").await;
    let (status, agent) = send(&setup.app, setup.browser.request("POST", &format!("/api/v1/projects/{}/external-agents", seed.project_id), None, json!({
        "connectionName": "Requester", "displayName": "Requester", "handle": "requester", "role": "member", "scopes": ["read", "write", "worker_request"]
    }))).await;
    assert_eq!(status, StatusCode::OK, "{agent}");
    let token = agent["oneTimeCredential"]["token"].as_str().expect("token");
    let session = initialize(&setup.app, token).await.expect("session");
    let secret = "secret-copied-into-sponsored-request";
    let requested = tool(&setup.app, token, &session, "run_request", json!({
        "conversationId": work, "repositoryId": seed.repository_id, "sponsorActorId": seed.human_actor_id,
        "prompt": secret, "clientRequestId": "sponsor-secret"
    })).await;
    assert_eq!(requested["outcome"], "requested", "{requested}");
    let approval = requested["approvalId"].as_str().expect("approval");
    let (status, approved) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/agent-approvals/{approval}/respond"),
            Some("approve-secret"),
            json!({"decision": "approve"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let message: Uuid = sqlx::query_scalar(
        "SELECT id FROM messages WHERE conversation_id = $1 AND plain_text = $2",
    )
    .bind(work)
    .bind(secret)
    .fetch_one(&setup.pool)
    .await
    .expect("prompt");
    let removed = tool(
        &setup.app,
        token,
        &session,
        "message_delete",
        json!({"messageId": message}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    for table in [
        "agent_runs",
        "external_execution_requests",
        "idempotency_keys",
        "outbox_events",
    ] {
        assert_eq!(
            super::message_edits::copies(&setup.pool, table, secret).await,
            0,
            "{table} retains the prompt"
        );
    }
}

#[tokio::test]
async fn prompt_redaction_migration_backfills_edited_messages_and_retry_chains() {
    let Some((setup, seed, _temporary)) = seeded_through("prompt_backfill", 52).await else {
        return;
    };
    execute(&setup.pool, "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1", &[seed.repository_id]).await;
    let work = thread(&setup.pool, &seed, seed.channel_id, "Old run").await;
    let message = Uuid::now_v7();
    let root = Uuid::now_v7();
    let retry = Uuid::now_v7();
    let mut transaction = setup.pool.begin().await.expect("old transaction");
    sqlx::query(r#"INSERT INTO messages (id, conversation_id, author_actor_id, content_version, content, plain_text) VALUES ($1, $2, $3, 1, '{"version":1,"nodes":[]}', 'edited replacement')"#)
        .bind(message).bind(work).bind(seed.human_actor_id).execute(&mut *transaction).await.expect("old message");
    for (id, predecessor) in [(root, None), (retry, Some(root))] {
        sqlx::query("INSERT INTO agent_runs (id, conversation_id, execution_worker_id, requesting_actor_id, execution_profile_id, repository_id, predecessor_run_id, state, prompt, authorization_snapshot) VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued', 'old removed secret', '{}')")
            .bind(id).bind(work).bind(seed.execution_worker_id).bind(seed.human_actor_id).bind(seed.execution_profile_id).bind(seed.repository_id).bind(predecessor)
            .execute(&mut *transaction).await.expect("old run");
        if predecessor.is_none() {
            sqlx::query("UPDATE agent_runs SET state = 'failed' WHERE id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .expect("old failure");
        }
    }
    sqlx::query("SELECT set_config('aw.message_command', $1, true)")
        .bind(message.to_string())
        .execute(&mut *transaction)
        .await
        .expect("allow removal");
    sqlx::query("UPDATE messages SET plain_text = '', removed_at = now(), removed_by_actor_id = $2 WHERE id = $1")
        .bind(message).bind(seed.human_actor_id).execute(&mut *transaction).await.expect("old removal");
    transaction.commit().await.expect("old data committed");
    sqlx::migrate!("./migrations")
        .run(&setup.pool)
        .await
        .expect("upgrade populated database");
    let runs = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT prompt_message_id, prompt, state FROM agent_runs WHERE id IN ($1, $2)",
    )
    .bind(root)
    .bind(retry)
    .fetch_all(&setup.pool)
    .await
    .expect("backfilled runs");
    assert_eq!(runs.len(), 2);
    for (source, prompt, state) in runs {
        assert_eq!(source, message);
        assert_eq!(prompt, "[Message removed]");
        assert!(matches!(state.as_str(), "failed" | "canceled"), "{state}");
    }
}

/// A run's linked pull request (title, link, number, state, checks) comes
/// with it only for a caller whose role, or app limit, keeps "see code" in
/// the run's project; the run itself stays (AWD-74).
#[tokio::test]
async fn a_runs_pull_request_needs_see_code() {
    let Some((setup, seed, _temporary)) = seeded("mcp_code_run_pr").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let thread = thread(pool, &seed, seed.channel_id, "Offline work").await;
    // A run needs its repository prepared and available, as the other run
    // tests here set up.
    execute(
        pool,
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1",
        &[seed.repository_id],
    )
    .await;
    let (run_id, workspace) = run(pool, &seed, thread, "feature").await;
    let (pull_request, _) = seed_pull_request(&setup, &aw, seed.repository_id, 41).await;
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_requests (
            id, project_id, github_installation_id, repository_id, repository_workspace_id,
            agent_run_id, requesting_actor_id, head_ref, head_sha, base_ref, title, body,
            state, completed_at, github_pull_request_id
        )
        SELECT $1, pr.project_id, pr.github_installation_id, pr.repository_id, $2, $3, $4,
               pr.head_ref, pr.head_sha, pr.base_ref, pr.title, '', 'completed', now(), pr.id
        FROM github_pull_requests pr WHERE pr.id = $5
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(run_id)
    .bind(seed.human_actor_id)
    .bind(pull_request)
    .execute(pool)
    .await
    .expect("the run opened the pull request");
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &owner_app).await.expect("owner's app");
    let get = json!({"runId": run_id});

    let opened = tool(app, &owner_app, &session, "run_get", get.clone()).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(opened.to_string().contains("Offline mode"), "{opened}");

    // The owner's apps no longer see code: the run, without its pull request.
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['tasks.view', 'runs.launch'] WHERE actor_id = $1",
    )
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("the owner's apps no longer see code");
    let opened = tool(app, &owner_app, &session, "run_get", get).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(opened["value"]["id"].is_string(), "{opened}");
    assert!(!opened.to_string().contains("Offline mode"), "{opened}");
    let listed = tool(
        app,
        &owner_app,
        &session,
        "run_list",
        json!({"projectId": aw, "state": "all"}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(!listed.to_string().contains("Offline mode"), "{listed}");
}

/// A run's page (run_steps, and the browser's run list and page, which
/// share it) names its task only for a caller who sees tasks in the run's
/// project, and its branch, commits, changed files and diff only for one who
/// sees code; the run itself stays (AWD-74).
#[tokio::test]
async fn a_runs_task_and_code_follow_see_tasks_and_see_code() {
    let Some((setup, seed, _temporary)) = seeded("mcp_code_run_task").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let thread = thread(pool, &seed, seed.channel_id, "Task work").await;
    execute(
        pool,
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1",
        &[seed.repository_id],
    )
    .await;
    let (run_id, _) = run(pool, &seed, thread, "secret-branch").await;
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &owner_app).await.expect("owner's app");
    let created = tool(
        app,
        &owner_app,
        &session,
        "task_create",
        json!({"projectId": seed.project_id.to_string(), "title": "Secret run task"}),
    )
    .await;
    let task = created["workItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("task: {created}"))
        .parse::<Uuid>()
        .expect("task id");
    sqlx::query(
        r#"
        INSERT INTO work_item_conversation_links
            (project_id, work_item_id, conversation_id, created_by_actor_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(seed.project_id)
    .bind(task)
    .bind(thread)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("the task's thread");
    let steps = json!({"runId": run_id, "includeDiff": true});

    let seen = tool(app, &owner_app, &session, "run_steps", steps.clone()).await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    assert_eq!(
        seen["value"]["run"]["taskTitle"], "Secret run task",
        "{seen}"
    );
    assert_eq!(seen["value"]["run"]["branch"], "secret-branch", "{seen}");

    // The owner's apps see neither tasks nor code now.
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['runs.launch'] WHERE actor_id = $1",
    )
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("the owner's apps see neither");
    let limited = tool(app, &owner_app, &session, "run_steps", steps).await;
    assert_eq!(limited["outcome"], "ok", "{limited}");
    let run = &limited["value"]["run"];
    assert_eq!(run["id"], run_id.to_string(), "{limited}");
    assert!(run["taskKey"].is_string(), "{limited}");
    assert!(run["taskTitle"].is_null(), "{limited}");
    assert!(run["branch"].is_null(), "{limited}");
    assert!(run["files"].is_null(), "{limited}");
    assert_eq!(
        limited["value"]["diff"]["state"], "unavailable",
        "{limited}"
    );
    assert!(
        !limited.to_string().contains("Secret run task"),
        "{limited}"
    );
    assert!(!limited.to_string().contains("secret-branch"), "{limited}");
    // run_get and run_list leave out the branch too, with no pull request.
    let opened = tool(
        app,
        &owner_app,
        &session,
        "run_get",
        json!({"runId": run_id}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(opened["value"]["branch"].is_null(), "{opened}");
    assert!(!opened.to_string().contains("secret-branch"), "{opened}");
    let listed = tool(
        app,
        &owner_app,
        &session,
        "run_list",
        json!({"projectId": seed.project_id.to_string(), "state": "all"}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(!listed.to_string().contains("secret-branch"), "{listed}");
}

/// A thread's run (opening the thread, and the run an interrupt answers
/// with, which share conversations::hide_pull_request) shows its branch and
/// its base and published commits only to someone who sees code in the
/// project, with no pull request involved; the run itself stays (AWD-74).
#[tokio::test]
async fn a_threads_run_shows_its_branch_only_to_those_who_see_code() {
    let Some((setup, seed, _temporary)) = seeded("mcp_code_thread_run").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let thread = thread(pool, &seed, seed.channel_id, "Branch work").await;
    execute(
        pool,
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1",
        &[seed.repository_id],
    )
    .await;
    let (run_id, workspace) = run(pool, &seed, thread, "hidden-branch").await;
    // Working, so that an interrupt stops it.
    for next in ["starting", "working"] {
        sqlx::query("UPDATE agent_runs SET state = $2 WHERE id = $1")
            .bind(run_id)
            .bind(next)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("run to {next}: {error}"));
    }
    let base = "b".repeat(40);
    let published = "c".repeat(40);
    // Published as the workspace's publication shape wants: commit, time and
    // who, together.
    sqlx::query(
        "UPDATE repository_workspaces SET published_commit = $2, published_at = now(), published_by_actor_id = $3 WHERE id = $1",
    )
    .bind(workspace)
    .bind(&published)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("published commit");
    let (bea, bea_browser) = add_person(&setup, &[&aw], "bea").await;
    let timeline = format!("/api/v1/conversations/{thread}/timeline");
    let open = |browser: &Browser| browser.request("GET", &timeline, None, Value::Null);
    let interrupt = |browser: &Browser, key: &str| {
        browser.request(
            "POST",
            &format!("/api/v1/agent-runs/{run_id}/interrupt"),
            Some(key),
            Value::Null,
        )
    };
    let shows_code = |body: &Value| {
        let text = body.to_string();
        (
            text.contains("hidden-branch"),
            text.contains(&base),
            text.contains(&published),
        )
    };

    // As a plain Member, Bea sees the run's branch and commits.
    let (status, seen) = send(app, open(&bea_browser)).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert!(seen.to_string().contains(&run_id.to_string()), "{seen}");
    assert_eq!(shows_code(&seen), (true, true, true), "{seen}");

    // A role that may stop runs but not see code.
    let (status, role) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{}/roles", setup.organization_id),
            None,
            json!({"name": "No code", "baseRole": "member", "permissions": ["tasks.view", "channels.post", "runs.interrupt"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let version: i64 = sqlx::query_scalar(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(bea)
    .fetch_one(pool)
    .await
    .expect("membership version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{aw}/members/{bea}"),
            Some("bea-no-code"),
            json!({"roleId": role["id"], "expectedVersion": version}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");

    // The thread's active run, the run her interrupt answers with, and the
    // thread's latest finished run: no branch or commits.
    let (status, active) = send(app, open(&bea_browser)).await;
    assert_eq!(status, StatusCode::OK, "{active}");
    assert!(active.to_string().contains(&run_id.to_string()), "{active}");
    assert_eq!(shows_code(&active), (false, false, false), "{active}");
    let (status, stopped) = send(app, interrupt(&bea_browser, "bea-interrupt")).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert!(
        stopped.to_string().contains(&run_id.to_string()),
        "{stopped}"
    );
    assert_eq!(shows_code(&stopped), (false, false, false), "{stopped}");
    let (status, finished) = send(app, open(&bea_browser)).await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert!(
        finished.to_string().contains(&run_id.to_string()),
        "{finished}"
    );
    assert_eq!(shows_code(&finished), (false, false, false), "{finished}");

    // The owner sees code: the same answers carry the branch and commits.
    let (status, owner_stopped) = send(app, interrupt(&setup.browser, "owner-interrupt")).await;
    assert_eq!(status, StatusCode::OK, "{owner_stopped}");
    assert_eq!(
        shows_code(&owner_stopped),
        (true, true, true),
        "{owner_stopped}"
    );
    let (status, owner_finished) = send(app, open(&setup.browser)).await;
    assert_eq!(status, StatusCode::OK, "{owner_finished}");
    assert_eq!(
        shows_code(&owner_finished),
        (true, true, true),
        "{owner_finished}"
    );
}

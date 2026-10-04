//! Phase 4 groundwork (docs/plans/mcp-tools.md): who may see a pull request,
//! how the middleware picks a call's project, and the per-person run cap.
//! The costly bugs are silent: a PR opened from a private thread listed to
//! the whole project, a secondary argument (a repository) deciding the
//! project a task is looked up in, or two calls at the cap both starting a
//! run.

use std::collections::BTreeMap;

use agent_workspace_server::{
    config::AppConfig,
    runs::{MCP_ACTIVE_RUN_CAP, run_cap},
    seed::dev_seed,
};
use serde_json::{Value, json};
use sqlx::{Executor, postgres::PgPoolOptions};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, schema_pool, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_task_board::{insert_repository, owner, seed_pull_request},
    mcp_tasks::{add_member, space},
};

/// A second pull request on the same repository and installation as
/// `first`.
async fn another_pull_request(setup: &Setup, first: Uuid, number: i64) -> (Uuid, String) {
    let id = Uuid::now_v7();
    let url = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO github_pull_requests (
            id, project_id, repository_id, github_installation_id, github_repository_id,
            pull_request_number, node_id, html_url, head_ref, head_sha, base_ref, title,
            author_login, draft, state, external_created_at, external_updated_at
        )
        SELECT $1, project_id, repository_id, github_installation_id, github_repository_id,
               $2, 'PR_' || $1::text,
               regexp_replace(html_url, '/pull/[0-9]+$', '/pull/' || $2::text),
               'other', head_sha, base_ref, 'Private work', author_login, false, 'open',
               now(), now()
        FROM github_pull_requests WHERE id = $3
        RETURNING html_url
        "#,
    )
    .bind(id)
    .bind(number)
    .bind(first)
    .fetch_one(&setup.pool)
    .await
    .expect("second pull request");
    (id, url)
}

async fn link_to_thread(setup: &Setup, pull_request: Uuid, thread: &str, source: &str, by: Uuid) {
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_conversation_links (
            project_id, github_pull_request_id, conversation_id, source, created_by_actor_id
        )
        SELECT project_id, id, $2::uuid, $3, $4 FROM github_pull_requests WHERE id = $1
        "#,
    )
    .bind(pull_request)
    .bind(thread)
    .bind(source)
    .bind(by)
    .execute(&setup.pool)
    .await
    .expect("thread link");
}

async fn visible_to(setup: &Setup, pull_request: Uuid, viewer: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT pull_request_visible_to($1, $2)")
        .bind(pull_request)
        .bind(viewer)
        .fetch_one(&setup.pool)
        .await
        .expect("visibility")
}

#[tokio::test]
async fn a_pull_request_from_a_private_thread_stays_private() {
    let Some(setup) = setup("phase4_pr_visibility").await else {
        return;
    };
    let app = &setup.app;
    let repo = create_project(&setup, "REPO", "Repo").await;
    let owner_id = owner(&setup, &repo).await;
    let repository = insert_repository(&setup, &repo, "Mischa-dev/aw", true, true, None).await;
    let (open, open_url) = seed_pull_request(&setup, &repo, repository, 7).await;
    let (private, private_url) = another_pull_request(&setup, open, 8).await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &repo, mara, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let mara_call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    // A thread in a channel only the owner is in, and one everyone sees.
    let mut threads = Vec::new();
    for name in ["secret", "design"] {
        let channel = call("channel_create", json!({"projectId": "REPO", "name": name})).await;
        let channel_id = channel["value"]["id"].as_str().expect("channel").to_owned();
        let sent = call(
            "message_send",
            json!({"conversationId": channel_id, "text": format!("About {name}")}),
        )
        .await;
        let thread = call(
            "thread_start",
            json!({"conversationId": channel_id, "fromMessageId": sent["value"]["messageId"]}),
        )
        .await;
        assert_eq!(thread["outcome"], "ok", "{thread}");
        threads.push((
            channel_id,
            thread["value"]["id"].as_str().expect("thread").to_owned(),
        ));
    }
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1::uuid")
        .bind(&threads[0].0)
        .execute(&setup.pool)
        .await
        .expect("make secret private");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1::uuid, $2::uuid, $3, $3)",
    )
    .bind(&repo)
    .bind(&threads[0].0)
    .bind(owner_id)
    .execute(&setup.pool)
    .await
    .expect("owner stays in secret");

    // The private PR came from a run in the secret thread. An explicit link
    // to the public thread must not make it public.
    link_to_thread(&setup, private, &threads[0].1, "run_artifact", owner_id).await;
    link_to_thread(&setup, private, &threads[1].1, "explicit", owner_id).await;
    // A PR whose run happened in a public thread stays public.
    link_to_thread(&setup, open, &threads[1].1, "run_artifact", owner_id).await;
    assert!(visible_to(&setup, private, owner_id).await);
    assert!(!visible_to(&setup, private, mara).await);
    assert!(visible_to(&setup, open, mara).await);

    // pull_request_get: hidden reads exactly like missing.
    for identifier in [private.to_string(), private_url.clone()] {
        let hidden = mara_call("pull_request_get", json!({"identifier": identifier})).await;
        assert_eq!(hidden["outcome"], "not_found", "{hidden}");
        let seen = call("pull_request_get", json!({"identifier": identifier})).await;
        assert_eq!(seen["outcome"], "found", "{seen}");
    }
    let missing = mara_call(
        "pull_request_get",
        json!({"identifier": Uuid::now_v7().to_string()}),
    )
    .await;
    let hidden = mara_call(
        "pull_request_get",
        json!({"identifier": private.to_string()}),
    )
    .await;
    assert_eq!(hidden, missing);
    let public = mara_call("pull_request_get", json!({"identifier": open_url})).await;
    assert_eq!(public["outcome"], "found", "{public}");

    // task_link: nobody links a PR they can't see.
    let created = call(
        "task_create",
        json!({"projectId": "REPO", "title": "Offline mode"}),
    )
    .await;
    assert!(created["workItemId"].is_string(), "{created}");
    for pull_request in [private.to_string(), private_url.clone()] {
        let refused = mara_call(
            "task_link",
            json!({"taskId": "REPO-1", "pullRequestId": pull_request}),
        )
        .await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
    }
    let mara_links = mara_call(
        "task_link",
        json!({"taskId": "REPO-1", "pullRequestId": open.to_string()}),
    )
    .await;
    assert_eq!(mara_links["outcome"], "ok", "{mara_links}");
    let owner_links = call(
        "task_link",
        json!({"taskId": "REPO-1", "pullRequestId": private.to_string()}),
    )
    .await;
    assert_eq!(owner_links["outcome"], "ok", "{owner_links}");

    // task_open lists only the linked PRs the reader can see, and the
    // owner can't be tricked into a link that names the hidden one either.
    let mara_opens = mara_call("task_open", json!({"taskId": "REPO-1"})).await;
    assert_eq!(mara_opens["outcome"], "ok", "{mara_opens}");
    let text = mara_opens.to_string();
    assert!(text.contains(&open_url), "{mara_opens}");
    assert!(!text.contains(&private_url), "{mara_opens}");
    assert!(!text.contains(&private.to_string()), "{mara_opens}");
    let owner_opens = call("task_open", json!({"taskId": "REPO-1"})).await;
    assert!(
        owner_opens.to_string().contains(&private_url),
        "{owner_opens}"
    );
}

/// A moved task's "still linked" comment is read by everyone in the target
/// project, so it names only links every source-project member can see; the
/// private ones the mover can see are only counted.
#[tokio::test]
async fn a_moved_task_does_not_name_private_links_in_the_target_project() {
    let Some(setup) = setup("phase4_move_private_links").await else {
        return;
    };
    let app = &setup.app;
    let source = create_project(&setup, "SRC", "Source").await;
    let target = create_project(&setup, "DST", "Target").await;
    let owner_id = owner(&setup, &source).await;
    let repository = insert_repository(&setup, &source, "Mischa-dev/aw", true, true, None).await;
    let (open, open_url) = seed_pull_request(&setup, &source, repository, 7).await;
    let (private, private_url) = another_pull_request(&setup, open, 8).await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &target, mara, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let mut threads = Vec::new();
    for name in ["secret", "design"] {
        let channel = call("channel_create", json!({"projectId": "SRC", "name": name})).await;
        let channel_id = channel["value"]["id"].as_str().expect("channel").to_owned();
        let sent = call(
            "message_send",
            json!({"conversationId": channel_id, "text": format!("About {name} plans")}),
        )
        .await;
        let thread = call(
            "thread_start",
            json!({"conversationId": channel_id, "fromMessageId": sent["value"]["messageId"]}),
        )
        .await;
        assert_eq!(thread["outcome"], "ok", "{thread}");
        let thread_id = thread["value"]["id"].as_str().expect("thread").to_owned();
        let thread_name =
            sqlx::query_scalar::<_, String>("SELECT name FROM conversations WHERE id = $1::uuid")
                .bind(&thread_id)
                .fetch_one(&setup.pool)
                .await
                .expect("thread name");
        threads.push((channel_id, thread_id, thread_name));
    }
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1::uuid")
        .bind(&threads[0].0)
        .execute(&setup.pool)
        .await
        .expect("make secret private");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1::uuid, $2::uuid, $3, $3)",
    )
    .bind(&source)
    .bind(&threads[0].0)
    .bind(owner_id)
    .execute(&setup.pool)
    .await
    .expect("owner stays in secret");
    link_to_thread(&setup, private, &threads[0].1, "run_artifact", owner_id).await;
    link_to_thread(&setup, open, &threads[1].1, "run_artifact", owner_id).await;

    let created = call(
        "task_create",
        json!({"projectId": "SRC", "title": "Offline mode"}),
    )
    .await;
    assert!(created["workItemId"].is_string(), "{created}");
    for link in [
        json!({"taskId": "SRC-1", "conversationId": threads[0].1}),
        json!({"taskId": "SRC-1", "conversationId": threads[1].1}),
        json!({"taskId": "SRC-1", "pullRequestId": private.to_string()}),
        json!({"taskId": "SRC-1", "pullRequestId": open.to_string()}),
    ] {
        let linked = call("task_link", link).await;
        assert_eq!(linked["outcome"], "ok", "{linked}");
    }

    let moved = call(
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let new_key = moved["value"]["moved"][0]["to"]
        .as_str()
        .expect("new key")
        .to_owned();

    let mara_opens = tool(
        app,
        &mara_token,
        &mara_session,
        "task_open",
        json!({"taskId": new_key}),
    )
    .await;
    assert_eq!(mara_opens["outcome"], "ok", "{mara_opens}");
    let comments = mara_opens["value"]["comments"].to_string();
    assert!(comments.contains("Still linked on SRC-1"), "{comments}");
    assert!(comments.contains(&threads[1].2), "{comments}");
    assert!(comments.contains(&open_url), "{comments}");
    assert!(!comments.contains(&threads[0].2), "{comments}");
    assert!(!comments.contains(&private_url), "{comments}");
    assert!(
        comments.contains("and 1 private conversation"),
        "{comments}"
    );
    assert!(
        comments.contains("and 1 private pull request"),
        "{comments}"
    );
}

#[tokio::test]
async fn the_resource_a_call_acts_on_picks_its_project() {
    let Some(setup) = setup("phase4_scoping").await else {
        return;
    };
    let app = &setup.app;
    let alpha = create_project(&setup, "ALPHA", "Alpha").await;
    let beta = create_project(&setup, "BETA", "Beta").await;
    let in_alpha = insert_repository(&setup, &alpha, "Acme/shared", true, true, None).await;
    let in_beta = insert_repository(&setup, &beta, "Acme/shared", true, true, None).await;
    let only_beta = insert_repository(&setup, &beta, "Acme/beta-only", true, true, None).await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &alpha, mara, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let mara_call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let created = call(
        "task_create",
        json!({"projectId": "ALPHA", "title": "Offline mode"}),
    )
    .await;
    assert!(created["workItemId"].is_string(), "{created}");

    // owner/name resolves where it's unique.
    let only = call("repository_get", json!({"repositoryId": "acme/beta-only"})).await;
    assert_eq!(only["outcome"], "found", "{only}");
    assert_eq!(only["value"]["id"], only_beta.to_string(), "{only}");
    // In two projects and no task or project to decide: left for the tool
    // to refuse.
    let ambiguous = call("repository_get", json!({"repositoryId": "acme/shared"})).await;
    assert_ne!(ambiguous["outcome"], "found", "{ambiguous}");

    // The task decides: owner/name resolves inside the task's project, so
    // the same name in another project isn't picked.
    let linked = call(
        "task_link",
        json!({"taskId": "ALPHA-1", "repositoryId": "acme/shared"}),
    )
    .await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    assert_eq!(
        linked["value"]["targetId"],
        in_alpha.to_string(),
        "{linked}"
    );

    // A repository in a project Mara isn't in no longer picks the project
    // ahead of the task: the task's project is checked, and the repository
    // simply isn't there (it used to be refused as a project she can't
    // reach, giving away that it exists).
    let elsewhere = mara_call(
        "task_link",
        json!({"taskId": "ALPHA-1", "repositoryId": in_beta.to_string()}),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "not_found", "{elsewhere}");
}

#[tokio::test]
async fn the_run_cap_counts_every_active_run_and_holds_a_lock() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("phase4_cap_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
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
    sqlx::query(
        "UPDATE execution_profiles SET account_state = 'authenticated', version = version + 1 WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("authenticate profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), version = version + 1 WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare repository");
    let hidden_channel = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name, visibility) VALUES ($1, $2, 'channel', 'cap-hidden', 'members')",
    )
    .bind(hidden_channel)
    .bind(seed.project_id)
    .execute(&pool)
    .await
    .expect("hidden channel");
    let thread = |parent: Uuid, name: String| {
        let pool = pool.clone();
        let repository = seed.repository_id;
        let project = seed.project_id;
        async move {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name) VALUES ($1, $2, 'thread', $3, $4, $5)",
            )
            .bind(id)
            .bind(project)
            .bind(parent)
            .bind(repository)
            .bind(name)
            .execute(&pool)
            .await
            .expect("thread");
            id
        }
    };
    let run = |conversation: Uuid, state: &'static str| {
        let pool = pool.clone();
        let seed = &seed;
        async move {
            let id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO agent_runs (
                    id, conversation_id, execution_worker_id, requesting_actor_id,
                    execution_profile_id, repository_id, state, prompt,
                    authorization_snapshot, failure_reason, finished_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, 'cap test', '{}'::jsonb,
                        CASE WHEN $7 = 'failed' THEN 'A synthetic failure.' END,
                        CASE WHEN $7 = 'failed' THEN now() END)
                "#,
            )
            .bind(id)
            .bind(conversation)
            .bind(seed.execution_worker_id)
            .bind(seed.human_actor_id)
            .bind(seed.execution_profile_id)
            .bind(seed.repository_id)
            .bind(state)
            .execute(&pool)
            .await
            .expect("run");
            id
        }
    };

    let mut transaction = pool.begin().await.expect("transaction");
    let empty = run_cap(&mut transaction, seed.human_actor_id)
        .await
        .expect("cap");
    transaction.rollback().await.expect("rollback");
    assert_eq!(empty.active, 0);
    assert!(!empty.reached());

    // A finished run doesn't count; a run waiting on an approval does, and so
    // does one in a thread the person can no longer see (it isn't named).
    let failed = thread(seed.channel_id, "cap failed".to_owned()).await;
    run(failed, "failed").await;
    let mut visible = Vec::new();
    for (index, state) in ["queued", "created", "blocked", "queued"]
        .into_iter()
        .enumerate()
    {
        let conversation = thread(seed.channel_id, format!("cap {index}")).await;
        visible.push(run(conversation, state).await);
    }
    let hidden_thread = thread(hidden_channel, "cap hidden".to_owned()).await;
    let hidden = run(hidden_thread, "queued").await;

    let mut transaction = pool.begin().await.expect("transaction");
    let full = run_cap(&mut transaction, seed.human_actor_id)
        .await
        .expect("cap");
    assert_eq!(full.active, MCP_ACTIVE_RUN_CAP);
    assert!(full.reached());
    assert_eq!(full.visible_run_ids.len(), 4, "{:?}", full.visible_run_ids);
    assert!(!full.visible_run_ids.contains(&hidden));
    visible.reverse();
    assert_eq!(full.visible_run_ids, visible);
    let other = run_cap(
        &mut pool.begin().await.expect("other"),
        seed.collaborator_actor_id,
    )
    .await
    .expect("someone else's cap");
    assert_eq!(other.active, 0);

    // While one transaction holds the person's cap, a second waits.
    let mut second = pool.begin().await.expect("second transaction");
    sqlx::query("SET LOCAL lock_timeout = '200ms'")
        .execute(&mut *second)
        .await
        .expect("lock timeout");
    assert!(
        run_cap(&mut second, seed.human_actor_id).await.is_err(),
        "the cap lock should be held"
    );
    second.rollback().await.expect("rollback second");
    transaction.rollback().await.expect("release");
    let mut third = pool.begin().await.expect("third transaction");
    sqlx::query("SET LOCAL lock_timeout = '200ms'")
        .execute(&mut *third)
        .await
        .expect("lock timeout");
    assert!(run_cap(&mut third, seed.human_actor_id).await.is_ok());
    third.rollback().await.expect("rollback third");

    drop(pool);
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop schema");
}

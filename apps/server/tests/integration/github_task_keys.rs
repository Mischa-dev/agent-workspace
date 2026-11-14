//! AWD-59: a pull request that names a task's key (AWD-59) in its branch,
//! title or body is suggested on that task, unconfirmed. The costly bugs are
//! silent: a key from another project, or for an archived or missing task,
//! linking anyway; a suggestion overriding a person's confirmed link; or a
//! dismissed one coming back on the next sync.

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::create_project,
    mcp_code::seeded,
    mcp_task_board::seed_pull_request,
};

async fn key_of(pool: &PgPool, task: Uuid) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT project.project_key || '-' || item.number FROM work_items item JOIN projects project ON project.id = item.project_id WHERE item.id = $1",
    )
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("task key")
}

async fn suggest(
    pool: &PgPool,
    project: Uuid,
    pull_request: Uuid,
    branch: &[&str],
    body: &[&str],
    actor: Uuid,
) -> i32 {
    sqlx::query_scalar::<_, i32>("SELECT github_suggest_task_links($1, $2, $3, $4, $5)")
        .bind(project)
        .bind(pull_request)
        .bind(branch)
        .bind(body)
        .bind(actor)
        .fetch_one(pool)
        .await
        .expect("suggest task links")
}

async fn links(pool: &PgPool, pull_request: Uuid) -> Vec<(Uuid, String, bool)> {
    sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT work_item_id, source, confirmed FROM github_pull_request_work_item_links WHERE github_pull_request_id = $1 ORDER BY source, work_item_id",
    )
    .bind(pull_request)
    .fetch_all(pool)
    .await
    .expect("links")
}

async fn names_open_task(pool: &PgPool, project: Uuid, payload: serde_json::Value) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT github_job_names_open_task($1, $2)")
        .bind(project)
        .bind(payload)
        .fetch_one(pool)
        .await
        .expect("names open task")
}

#[tokio::test]
async fn a_pull_request_naming_a_task_key_is_suggested_on_that_task_only() {
    let Some((setup, seed, _temporary)) = seeded("github_task_keys").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let owner = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &owner).await.expect("owner's app");
    let mut tasks = Vec::new();
    for title in ["Fix the board", "Write the guide", "Old idea"] {
        let created = tool(
            app,
            &owner,
            &session,
            "task_create",
            json!({"projectId": aw, "title": title}),
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
    let (board, guide, old) = (tasks[0], tasks[1], tasks[2]);
    let (board_key, guide_key, old_key) = (
        key_of(pool, board).await,
        key_of(pool, guide).await,
        key_of(pool, old).await,
    );
    // A task's archive state is its record's (T1.3).
    sqlx::query(
        "UPDATE collection_records SET archived_at = now(), archived_by_actor_id = $2, version = version + 1 WHERE id = $1",
    )
    .bind(old)
    .bind(seed.human_actor_id)
        .execute(pool)
        .await
        .expect("archive the old idea");
    sqlx::query("UPDATE work_items SET version = version + 1 WHERE id = $1")
        .bind(old)
        .execute(pool)
        .await
        .expect("bump the old idea");
    // Another project's task with the same number must never match.
    let beta = create_project(&setup, "BETA", "Beta").await;
    let beta_task = tool(
        app,
        &owner,
        &session,
        "task_create",
        json!({"projectId": beta, "title": "Elsewhere"}),
    )
    .await;
    let beta_key = key_of(
        pool,
        beta_task["workItemId"]
            .as_str()
            .unwrap_or_else(|| panic!("beta task: {beta_task}"))
            .parse()
            .expect("beta task id"),
    )
    .await;

    let (pull_request, _) = seed_pull_request(&setup, &aw, seed.repository_id, 59).await;
    let integration = sqlx::query_scalar::<_, Uuid>(
        "SELECT installation.integration_actor_id FROM github_pull_requests pr JOIN github_installations installation ON installation.id = pr.github_installation_id WHERE pr.id = $1",
    )
    .bind(pull_request)
    .fetch_one(pool)
    .await
    .expect("integration actor");

    // The branch names the board; the body names the guide (in lower case),
    // the archived idea, a task that doesn't exist, and another project's.
    let lower_guide = guide_key.to_lowercase();
    let added = suggest(
        pool,
        seed.project_id,
        pull_request,
        &[&board_key],
        &[&lower_guide, &old_key, "AW-99999", &beta_key, &board_key],
        integration,
    )
    .await;
    assert_eq!(added, 2);
    let mut expected = vec![
        (board, "branch_key".to_owned(), false),
        (guide, "body_marker".to_owned(), false),
    ];
    expected.sort_by(|a, b| (&a.1, a.0).cmp(&(&b.1, b.0)));
    assert_eq!(links(pool, pull_request).await, expected);
    // A second sync suggests nothing new.
    assert_eq!(
        suggest(
            pool,
            seed.project_id,
            pull_request,
            &[&board_key],
            &[&guide_key],
            integration
        )
        .await,
        0
    );

    // A dismissed suggestion stays dismissed.
    sqlx::query("DELETE FROM github_pull_request_work_item_links WHERE github_pull_request_id = $1 AND work_item_id = $2")
        .bind(pull_request)
        .bind(guide)
        .execute(pool)
        .await
        .expect("drop the suggestion");
    sqlx::query(
        "INSERT INTO github_pull_request_dismissed_links (project_id, github_pull_request_id, work_item_id, dismissed_by_actor_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(seed.project_id)
    .bind(pull_request)
    .bind(guide)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("dismiss the suggestion");
    assert_eq!(
        suggest(
            pool,
            seed.project_id,
            pull_request,
            &[],
            &[&guide_key],
            integration
        )
        .await,
        0
    );
    assert_eq!(
        links(pool, pull_request).await,
        vec![(board, "branch_key".to_owned(), false)]
    );

    // Unlinking the board's suggestion through task_link dismisses it, so the
    // next sync leaves it off; linking it again confirms it.
    let unlinked = tool(
        app,
        &owner,
        &session,
        "task_link",
        json!({"taskId": board.to_string(), "pullRequestId": pull_request.to_string(), "remove": true}),
    )
    .await;
    assert_eq!(unlinked["value"]["changed"], true, "{unlinked}");
    assert_eq!(
        suggest(
            pool,
            seed.project_id,
            pull_request,
            &[&board_key],
            &[],
            integration
        )
        .await,
        0
    );
    assert!(links(pool, pull_request).await.is_empty());
    let relinked = tool(
        app,
        &owner,
        &session,
        "task_link",
        json!({"taskId": board.to_string(), "pullRequestId": pull_request.to_string()}),
    )
    .await;
    assert_eq!(relinked["value"]["changed"], true, "{relinked}");
    assert_eq!(
        links(pool, pull_request).await,
        vec![(board, "explicit".to_owned(), true)]
    );
    let dismissed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM github_pull_request_dismissed_links WHERE github_pull_request_id = $1 AND work_item_id = $2",
    )
    .bind(pull_request)
    .bind(board)
    .fetch_one(pool)
    .await
    .expect("dismissals");
    assert_eq!(dismissed, 0);

    // A person's confirmed link is never turned into a suggestion.
    let linked = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO github_pull_requests (
            id, project_id, repository_id, github_installation_id, github_repository_id,
            pull_request_number, node_id, html_url, head_ref, head_sha, base_ref, title,
            author_login, draft, state, external_created_at, external_updated_at
        )
        SELECT $1, project_id, repository_id, github_installation_id, github_repository_id,
               60, 'PR_' || $1::text, regexp_replace(html_url, '/pull/[0-9]+$', '/pull/60'),
               head_ref, head_sha, base_ref, title, author_login, false, 'open', now(), now()
        FROM github_pull_requests WHERE id = $2
        "#,
    )
    .bind(linked)
    .bind(pull_request)
    .execute(pool)
    .await
    .expect("a second pull request");
    sqlx::query(
        "INSERT INTO github_pull_request_work_item_links (project_id, github_pull_request_id, work_item_id, source, confirmed, created_by_actor_id) VALUES ($1, $2, $3, 'explicit', true, $4)",
    )
    .bind(seed.project_id)
    .bind(linked)
    .bind(board)
    .bind(seed.human_actor_id)
    .execute(pool)
    .await
    .expect("confirmed link");
    assert_eq!(
        suggest(
            pool,
            seed.project_id,
            linked,
            &[&board_key],
            &[],
            integration
        )
        .await,
        0
    );
    assert_eq!(
        links(pool, linked).await,
        vec![(board, "explicit".to_owned(), true)]
    );

    // A pull request the app doesn't track is only read from GitHub when its
    // keys name one of this project's open tasks.
    let project = seed.project_id;
    assert!(names_open_task(pool, project, json!({"task_keys": [lower_guide]})).await);
    assert!(!names_open_task(pool, project, json!({"task_keys": [old_key]})).await);
    assert!(!names_open_task(pool, project, json!({"task_keys": [beta_key, "AW-99999"]})).await);
    assert!(!names_open_task(pool, project, json!({"task_keys": ["AW-01", "AW-1x", ""]})).await);
    assert!(!names_open_task(pool, project, json!({"task_keys": board_key})).await);
    assert!(!names_open_task(pool, project, json!({"number": 59})).await);

    // The task shows a suggestion as not confirmed yet.
    sqlx::query(
        "DELETE FROM github_pull_request_work_item_links WHERE github_pull_request_id = $1",
    )
    .bind(pull_request)
    .execute(pool)
    .await
    .expect("clear the links");
    assert_eq!(
        suggest(
            pool,
            seed.project_id,
            pull_request,
            &[&board_key],
            &[],
            integration
        )
        .await,
        1
    );
    let (status, read) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{aw}/work-items/{board}"),
            None,
            serde_json::Value::Null,
        ),
    )
    .await;
    assert_eq!(status, 200, "{read}");
    let suggested = read["linkedPullRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("linked pull requests: {read}"))
        .iter()
        .find(|line| line["number"] == 59)
        .unwrap_or_else(|| panic!("the suggested pull request: {read}"));
    assert_eq!(suggested["confirmed"], false, "{read}");
}

#[tokio::test]
async fn a_suggested_pull_request_is_confirmed_or_dismissed_on_the_task() {
    let Some((setup, seed, _temporary)) = seeded("github_task_key_confirm").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let owner = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &owner).await.expect("owner's app");
    let created = tool(
        app,
        &owner,
        &session,
        "task_create",
        json!({"projectId": aw, "title": "Fix the board"}),
    )
    .await;
    let task: Uuid = created["workItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("task: {created}"))
        .parse()
        .expect("task id");
    let key = key_of(pool, task).await;
    let (pull_request, _) = seed_pull_request(&setup, &aw, seed.repository_id, 59).await;
    let integration = sqlx::query_scalar::<_, Uuid>(
        "SELECT installation.integration_actor_id FROM github_pull_requests pr JOIN github_installations installation ON installation.id = pr.github_installation_id WHERE pr.id = $1",
    )
    .bind(pull_request)
    .fetch_one(pool)
    .await
    .expect("integration actor");
    assert_eq!(
        suggest(
            pool,
            seed.project_id,
            pull_request,
            &[&key],
            &[],
            integration
        )
        .await,
        1
    );

    // An agent reading the task sees the suggestion, marked as one.
    let opened = tool(app, &owner, &session, "task_open", json!({"taskId": key})).await;
    let listed = opened.to_string();
    assert!(listed.contains("\"suggested\":true"), "{opened}");

    let version = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
            .bind(task)
            .fetch_one(&pool)
            .await
            .expect("version")
    };
    let path = format!("/api/v1/projects/{aw}/work-items/{task}/pull-requests/{pull_request}");
    // A stale version is refused.
    let (status, _) = send(
        app,
        setup.browser.request(
            "DELETE",
            &path,
            Some("dismiss-stale"),
            json!({"expectedVersion": 0}),
        ),
    )
    .await;
    assert_eq!(status, 409);

    // Dismissing it from the task page keeps it off on the next sync.
    let (status, read) = send(
        app,
        setup.browser.request(
            "DELETE",
            &path,
            Some("dismiss"),
            json!({"expectedVersion": version(pool.clone()).await}),
        ),
    )
    .await;
    assert_eq!(status, 200, "{read}");
    assert!(links(pool, pull_request).await.is_empty());
    assert_eq!(
        suggest(
            pool,
            seed.project_id,
            pull_request,
            &[&key],
            &[],
            integration
        )
        .await,
        0
    );

    // Linking it from the task page confirms it, so automation can move the
    // task when it merges.
    let (status, read) = send(
        app,
        setup.browser.request(
            "POST",
            &path,
            Some("confirm"),
            json!({"expectedVersion": version(pool.clone()).await}),
        ),
    )
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(
        links(pool, pull_request).await,
        vec![(task, "explicit".to_owned(), true)]
    );
    let confirmed = read["linkedPullRequests"]
        .as_array()
        .and_then(|lines| lines.iter().find(|line| line["number"] == 59))
        .unwrap_or_else(|| panic!("the pull request: {read}"));
    assert_eq!(confirmed["confirmed"], true, "{read}");
    // Its card lists each check, failing ones first.
    for (id, name, conclusion) in [(1_i64, "build", "success"), (2, "lint", "failure")] {
        sqlx::query(
            "INSERT INTO github_pull_request_check_runs (id, project_id, github_pull_request_id, github_check_run_id, name, status, conclusion, details_url) VALUES ($1, $2, $3, $4, $5, 'completed', $6, 'https://github.com/checks')",
        )
        .bind(Uuid::now_v7())
        .bind(seed.project_id)
        .bind(pull_request)
        .bind(id)
        .bind(name)
        .bind(conclusion)
        .execute(pool)
        .await
        .expect("check run");
    }
    let (status, read) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{aw}/work-items/{task}"),
            None,
            serde_json::Value::Null,
        ),
    )
    .await;
    assert_eq!(status, 200, "{read}");
    let card = read["linkedPullRequests"]
        .as_array()
        .and_then(|lines| lines.iter().find(|line| line["number"] == 59))
        .unwrap_or_else(|| panic!("the pull request: {read}"));
    let names = card["checks"]
        .as_array()
        .unwrap_or_else(|| panic!("checks: {card}"))
        .iter()
        .map(|check| (check["name"].clone(), check["conclusion"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            (json!("lint"), json!("failure")),
            (json!("build"), json!("success"))
        ]
    );
    assert_eq!(card["headRef"], "feature", "{card}");

    let audited = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE target_id = $1 AND action IN ('work_item.pull_request_linked', 'work_item.pull_request_unlinked')",
    )
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("audit");
    assert_eq!(audited, 2);
}

#[tokio::test]
async fn merging_checks_the_pull_request_before_it_asks_github() {
    let Some((setup, seed, _temporary)) = seeded("github_task_key_merge").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let aw = seed.project_id.to_string();
    let (pull_request, _) = seed_pull_request(&setup, &aw, seed.repository_id, 61).await;
    let path = format!("/api/v1/projects/{aw}/pull-requests/{pull_request}/merge");
    let merge = |body: serde_json::Value| {
        let request = setup.browser.request("POST", &path, Some("merge"), body);
        send(app, request)
    };
    let set = |sql: &'static str| async move {
        sqlx::query(sql)
            .bind(pull_request)
            .execute(pool)
            .await
            .expect("change the pull request");
    };

    let (status, read) = merge(json!({"method": "octopus"})).await;
    assert_eq!(status, 400, "{read}");
    let (status, read) = merge(json!({"expectedHeadSha": "abc"})).await;
    assert_eq!(status, 400, "{read}");

    // The installation doesn't reach the repository yet.
    let (status, read) = merge(json!({})).await;
    assert_eq!(status, 400, "{read}");
    assert!(
        read.to_string().contains("doesn't reach that repository"),
        "{read}"
    );
    sqlx::query(
        "INSERT INTO github_installation_repositories (project_id, github_installation_id, repository_id, github_repository_id, access_state, last_verified_at) SELECT project_id, github_installation_id, repository_id, github_repository_id, 'selected', now() FROM github_pull_requests WHERE id = $1",
    )
    .bind(pull_request)
    .execute(pool)
    .await
    .expect("installation reaches the repository");

    // A head that moved since the caller looked is never merged.
    let (status, read) = merge(json!({"expectedHeadSha": "b".repeat(40)})).await;
    assert_eq!(status, 400, "{read}");
    assert!(read.to_string().contains("head moved"), "{read}");

    set("UPDATE github_pull_requests SET draft = true WHERE id = $1").await;
    let (status, read) = merge(json!({})).await;
    assert_eq!(status, 400, "{read}");
    assert!(read.to_string().contains("is a draft"), "{read}");
    set("UPDATE github_pull_requests SET draft = false, state = 'closed' WHERE id = $1").await;
    let (status, read) = merge(json!({})).await;
    assert_eq!(status, 400, "{read}");
    assert!(read.to_string().contains("isn't open"), "{read}");

    // Merged already (say an earlier try's answer was lost): asking again at
    // that head answers with the recorded merge, and nothing new is audited.
    set("UPDATE github_pull_requests SET state = 'merged', merged_at = now(), merge_commit_sha = repeat('d', 40) WHERE id = $1").await;
    let (status, read) = merge(json!({"expectedHeadSha": "a".repeat(40)})).await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["alreadyMerged"], true, "{read}");
    assert_eq!(read["mergeCommitSha"], "d".repeat(40), "{read}");
    let (status, read) = merge(json!({"expectedHeadSha": "b".repeat(40)})).await;
    assert_eq!(status, 400, "{read}");
    assert!(read.to_string().contains("head moved"), "{read}");
    set("UPDATE github_pull_requests SET state = 'closed', merged_at = NULL, merge_commit_sha = NULL WHERE id = $1")
        .await;

    // Everything checks out, so the next step is GitHub, which this server
    // doesn't reach.
    set("UPDATE github_pull_requests SET state = 'open' WHERE id = $1").await;
    let (status, read) =
        merge(json!({"expectedHeadSha": "a".repeat(40), "method": "squash"})).await;
    assert_eq!(status, 400, "{read}");
    assert!(read.to_string().contains("isn't set up"), "{read}");

    // The task page offers Merge to those who manage the project's code.
    let owner = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &owner).await.expect("owner's app");
    let created = tool(
        app,
        &owner,
        &session,
        "task_create",
        json!({"projectId": aw, "title": "Ship offline mode"}),
    )
    .await;
    let task = created["workItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("task: {created}"))
        .to_owned();
    // The agent tool checks the same things the button does.
    let merged = tool(
        app,
        &owner,
        &session,
        "pull_request_merge",
        json!({"pullRequestId": pull_request, "expectedHeadSha": "c".repeat(40)}),
    )
    .await;
    assert!(merged.to_string().contains("moved"), "{merged}");
    let (status, read) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{aw}/work-items/{task}"),
            None,
            json!(null),
        ),
    )
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["capabilities"]["mergePullRequests"], true, "{read}");
    // No merge reached GitHub, so nothing was recorded as merged.
    let audited = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE action = 'github.pull_request_merged'",
    )
    .fetch_one(pool)
    .await
    .expect("audit");
    assert_eq!(audited, 0);
}

//! task_list filters, task_reorder anchors and task_link to repositories and
//! pull requests (docs/plans/mcp-tools.md, Phase 2). The costly bugs are
//! silent: a filter or a count that reaches into a project the caller can't
//! see, a cursor reused across searches, a reorder that moves the wrong
//! neighbours or bumps tasks it didn't move, a link that changes the primary
//! repository Codex runs on without saying so, or a link written to an
//! archived task.

use std::time::Duration;

use agent_workspace_server::{
    mcp::{self, IssueCredentialCommand},
    policy::ActorKind,
    work_item_commands::{
        AuditAttribution, CommandActor, LinkWording, ThreadLinkOutcome, change_thread_link,
        lock_membership, lock_work_item,
    },
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
};

fn id(value: &Value) -> Uuid {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not an ID: {value}"))
        .parse()
        .expect("UUID")
}

fn message(result: &Value) -> &str {
    result["message"]
        .as_str()
        .unwrap_or_else(|| panic!("no message: {result}"))
}

fn keys(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("not a list: {rows}"))
        .iter()
        .map(|row| match row {
            Value::String(key) => key.clone(),
            row => row["key"]
                .as_str()
                .unwrap_or_else(|| panic!("no key: {row}"))
                .to_owned(),
        })
        .collect()
}

fn assert_refused(result: &Value, field: &str, says: &str) {
    assert_eq!(result["outcome"], "invalid_request", "{result}");
    assert_eq!(result["field"], field, "{result}");
    assert!(message(result).contains(says), "{result}");
}

async fn version(setup: &Setup, task: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("version")
}

async fn task_id(setup: &Setup, project: &str, number: i64) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_items WHERE project_id = $1::uuid AND number = $2",
    )
    .bind(project)
    .bind(number)
    .fetch_one(&setup.pool)
    .await
    .expect("task")
}

pub(super) async fn owner(setup: &Setup, project: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE project_id = $1::uuid AND role = 'owner'",
    )
    .bind(project)
    .fetch_one(&setup.pool)
    .await
    .expect("owner")
}

/// A repository row, as connecting one through GitHub leaves it.
pub(super) async fn insert_repository(
    setup: &Setup,
    project: &str,
    full_name: &str,
    trusted: bool,
    ready: bool,
    archived_by: Option<Uuid>,
) -> Uuid {
    let repository = Uuid::now_v7();
    let (owner, name) = full_name.split_once('/').expect("owner/name");
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url, default_branch,
            mirror_path, trusted, enabled, preparation_state, prepared_at,
            archived_at, archived_by_actor_id
        ) VALUES (
            $1, $2::uuid, $3, $4, $5, 'https://github.com/' || $4 || '/' || $5 || '.git', 'main',
            '/tmp/agent-workspace-test-' || $1::text || '.git', $6, $8::uuid IS NULL,
            CASE WHEN $7 THEN 'ready' ELSE 'pending' END, CASE WHEN $7 THEN now() END,
            CASE WHEN $8::uuid IS NOT NULL THEN now() END, $8
        )
        "#,
    )
    .bind(repository)
    .bind(project)
    .bind(i64::from(rand_number()))
    .bind(owner)
    .bind(name)
    .bind(trusted)
    .bind(ready)
    .bind(archived_by)
    .execute(&setup.pool)
    .await
    .expect("insert repository");
    repository
}

fn rand_number() -> u32 {
    // Distinct enough for GitHub repository IDs within one test schema.
    (Uuid::now_v7().as_u128() as u32 % 900_000_000) + 1_000
}

/// A pull request GitHub sync would have stored, on a repository with an
/// installation, as m5_github_foundation sets one up. Returns its ID and
/// GitHub URL.
pub(super) async fn seed_pull_request(
    setup: &Setup,
    project: &str,
    repository: Uuid,
    number: i64,
) -> (Uuid, String) {
    let integration = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'integration', 'active')")
        .bind(integration)
        .execute(&setup.pool)
        .await
        .expect("integration actor");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) VALUES ($1, $2::uuid, $3, 'member', 'GitHub', 'github_integration')",
    )
    .bind(Uuid::now_v7())
    .bind(project)
    .bind(integration)
    .execute(&setup.pool)
    .await
    .expect("integration membership");
    let installation = Uuid::now_v7();
    let events = vec![
        "push",
        "pull_request",
        "pull_request_review",
        "check_suite",
        "check_run",
        "repository",
    ];
    sqlx::query(
        r#"
        INSERT INTO github_installations (
            id, project_id, integration_actor_id, app_id, installation_id,
            account_id, account_login, target_type, repository_selection,
            metadata_permission, contents_permission, pull_requests_permission,
            checks_permission, subscribed_events, state, enabled, last_success_at
        ) VALUES (
            $1, $2::uuid, $3, 42, $4, 777, 'Mischa-dev', 'Organization', 'selected',
            'read', 'write', 'write', 'read', $5, 'available', true, now()
        )
        "#,
    )
    .bind(installation)
    .bind(project)
    .bind(integration)
    .bind(i64::from(rand_number()))
    .bind(&events)
    .execute(&setup.pool)
    .await
    .expect("installation");
    let (owner, name, github_repository_id) = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT owner, name, github_repository_id FROM repositories WHERE id = $1",
    )
    .bind(repository)
    .fetch_one(&setup.pool)
    .await
    .expect("repository");
    let pull_request = Uuid::now_v7();
    let url = format!("https://github.com/{owner}/{name}/pull/{number}");
    sqlx::query(
        r#"
        INSERT INTO github_pull_requests (
            id, project_id, repository_id, github_installation_id, github_repository_id,
            pull_request_number, node_id, html_url, head_ref, head_sha, base_ref, title,
            author_login, draft, state, external_created_at, external_updated_at
        ) VALUES (
            $1, $2::uuid, $3, $4, $5, $6, $7, $8, 'feature', repeat('a', 40), 'main',
            'Offline mode', 'mischa', false, 'open', now(), now()
        )
        "#,
    )
    .bind(pull_request)
    .bind(project)
    .bind(repository)
    .bind(installation)
    .bind(github_repository_id)
    .bind(number)
    .bind(format!("PR_{pull_request}"))
    .bind(&url)
    .execute(&setup.pool)
    .await
    .expect("pull request");
    (pull_request, url)
}

#[tokio::test]
async fn task_list_filters_count_and_stay_inside_reachable_projects() {
    let Some(setup) = setup("mcp_board_list").await else {
        return;
    };
    let app = &setup.app;
    let alpha = create_project(&setup, "ALPHA", "Alpha").await;
    let beta = create_project(&setup, "BETA", "Beta").await;
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
    let as_mara = |name: &'static str, arguments: Value| {
        let mara_token = mara_token.clone();
        let mara_session = mara_session.clone();
        async move { tool(app, &mara_token, &mara_session, name, arguments).await }
    };
    for (project, name) in [("ALPHA", "mobile"), ("BETA", "Mobile"), ("ALPHA", "Old")] {
        let label = call("label_write", json!({"projectId": project, "name": name})).await;
        assert_eq!(label["outcome"], "ok", "{label}");
    }
    let mut created = Vec::new();
    for arguments in [
        json!({"projectId": "ALPHA", "title": "Crash on launch", "priority": "urgent", "type": "Bug", "labels": ["mobile"]}),
        json!({"projectId": "ALPHA", "title": "Polish", "priority": "low"}),
        json!({"projectId": "ALPHA", "title": "Old idea"}),
        json!({"projectId": "BETA", "title": "Beta crash", "priority": "urgent", "labels": ["Mobile"]}),
    ] {
        let task = call("task_create", arguments).await;
        assert_eq!(task["outcome"], "applied", "{task}");
        created.push(id(&task["workItemId"]));
    }
    let by_mara = as_mara(
        "task_create",
        json!({"projectId": "ALPHA", "title": "Mara's", "priority": "medium"}),
    )
    .await;
    assert_eq!(by_mara["key"], "ALPHA-4", "{by_mara}");
    let archived_label = call(
        "label_write",
        json!({"projectId": "ALPHA", "label": "Old", "archived": true}),
    )
    .await;
    assert_eq!(archived_label["outcome"], "ok", "{archived_label}");
    let blocked = call(
        "task_mark_blocked",
        json!({"taskId": "ALPHA-2", "expectedVersion": 1, "reason": "Waiting on design", "clientRequestId": "block-2"}),
    )
    .await;
    assert_eq!(blocked["outcome"], "applied", "{blocked}");
    let claimed = as_mara("task_claim", json!({"taskId": "ALPHA-4"})).await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");
    let (status, archived) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{alpha}/work-items/{}/archive", created[2]),
            Some("board-archive-3"),
            json!({"expectedVersion": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");

    let list = |arguments: Value| {
        let call = &call;
        async move {
            let listed = call("task_list", arguments.clone()).await;
            assert_eq!(listed["outcome"], "ok", "{arguments}: {listed}");
            listed["value"].clone()
        }
    };
    let baseline = list(json!({})).await;
    let everything = keys(&baseline["tasks"]);
    assert_eq!(everything.len(), 4, "{baseline}");
    assert_eq!(baseline["total"], 4);
    assert!(!everything.contains(&"ALPHA-3".to_owned()), "{baseline}");

    // Several projects at once, by key, with a count across all pages.
    let urgent = list(json!({"projectIds": ["ALPHA", "BETA"], "priority": "urgent"})).await;
    assert_eq!(keys(&urgent["tasks"]), ["ALPHA-1", "BETA-1"], "{urgent}");
    assert_eq!(urgent["total"], 2);
    assert_eq!(
        keys(&list(json!({"priority": "none"})).await["tasks"]),
        Vec::<String>::new()
    );

    // Labels match by name in each project, and a task must carry all of
    // them. Unknown and archived labels are refused, not ignored.
    let labelled = list(json!({"labels": ["MOBILE"]})).await;
    assert_eq!(
        keys(&labelled["tasks"]),
        ["ALPHA-1", "BETA-1"],
        "{labelled}"
    );
    assert_eq!(labelled["total"], 2);
    assert_eq!(labelled["tasks"][0]["labels"], json!(["mobile"]));
    let unknown = call("task_list", json!({"labels": ["mobile", "nope"]})).await;
    assert_refused(&unknown, "labels", "nope");
    assert!(!message(&unknown).contains("ALPHA"), "{unknown}");
    assert_refused(
        &call(
            "task_list",
            json!({"projectId": "ALPHA", "labels": ["old"]}),
        )
        .await,
        "labels",
        "archived",
    );
    assert_refused(
        &call("task_list", json!({"labels": ["old"]})).await,
        "labels",
        "archived",
    );

    // Type, by name in any case; every line says its type.
    let bugs = list(json!({"type": "bug"})).await;
    assert_eq!(keys(&bugs["tasks"]), ["ALPHA-1"], "{bugs}");
    assert_eq!(bugs["tasks"][0]["type"], "Bug");
    let polish = list(json!({"query": "Polish"})).await;
    assert_eq!(polish["tasks"][0]["type"], "Task", "{polish}");
    assert_refused(
        &call("task_list", json!({"type": "Bugz"})).await,
        "type",
        "Types:",
    );

    // Blocked, claimed, created by, and updated since.
    assert_eq!(
        keys(&list(json!({"blocked": true})).await["tasks"]),
        ["ALPHA-2"]
    );
    let unblocked = keys(&list(json!({"blocked": false})).await["tasks"]);
    assert!(
        !unblocked.contains(&"ALPHA-2".to_owned()) && unblocked.contains(&"ALPHA-1".to_owned()),
        "{unblocked:?}"
    );
    assert_eq!(
        keys(&list(json!({"claimed": true})).await["tasks"]),
        ["ALPHA-4"]
    );
    assert_eq!(
        keys(&list(json!({"claimed": "@mara"})).await["tasks"]),
        ["ALPHA-4"]
    );
    let unclaimed = keys(&list(json!({"claimed": false})).await["tasks"]);
    assert!(!unclaimed.contains(&"ALPHA-4".to_owned()), "{unclaimed:?}");
    assert_eq!(unclaimed.len(), 3);
    let mine = as_mara("task_list", json!({"claimed": "me"})).await;
    assert_eq!(keys(&mine["value"]["tasks"]), ["ALPHA-4"], "{mine}");
    assert_refused(
        &call("task_list", json!({"claimed": "maybe"})).await,
        "claimed",
        "Use yes, no, me, or an @handle.",
    );
    assert_refused(
        &call("task_list", json!({"claimed": "@nobody"})).await,
        "claimed",
        "No one called @nobody",
    );
    assert_eq!(
        keys(&list(json!({"createdBy": "@mara"})).await["tasks"]),
        ["ALPHA-4"]
    );
    let by_owner = keys(&list(json!({"createdBy": "me"})).await["tasks"]);
    assert!(
        by_owner.contains(&"ALPHA-1".to_owned()) && !by_owner.contains(&"ALPHA-4".to_owned()),
        "{by_owner:?}"
    );
    assert_eq!(
        list(json!({"updatedSince": "2999-01-01"})).await["total"],
        0
    );
    assert_eq!(list(json!({"updatedSince": "24h"})).await["total"], 4);
    assert_refused(
        &call("task_list", json!({"updatedSince": "soon"})).await,
        "updatedSince",
        "24h or 7d",
    );

    // Archived tasks only with includeArchived, marked, at position 0.
    let with_archived = list(json!({"includeArchived": true})).await;
    assert_eq!(with_archived["total"], 5, "{with_archived}");
    let rows = with_archived["tasks"].as_array().expect("tasks");
    let old = rows
        .iter()
        .find(|row| row["key"] == "ALPHA-3")
        .unwrap_or_else(|| panic!("no ALPHA-3: {with_archived}"));
    assert_eq!(old["archived"], true);
    assert_eq!(old["boardPosition"], 0);
    let open = rows
        .iter()
        .find(|row| row["key"] == "ALPHA-1")
        .expect("ALPHA-1");
    assert!(open.get("archived").is_none(), "{open}");

    // Pages add up to the total, and a cursor only fits its own search.
    let mut paged = Vec::new();
    let mut cursor = Value::Null;
    let mut first_cursor = None;
    loop {
        let mut arguments = json!({"projectIds": ["ALPHA", "BETA"], "limit": 1});
        if !cursor.is_null() {
            arguments["cursor"] = cursor.clone();
        }
        let page = list(arguments).await;
        assert_eq!(page["total"], 4, "{page}");
        paged.extend(keys(&page["tasks"]));
        match page.get("nextCursor") {
            Some(next) => {
                first_cursor.get_or_insert_with(|| next.clone());
                cursor = next.clone();
            }
            None => break,
        }
        assert!(paged.len() <= 4, "{paged:?}");
    }
    let mut distinct = paged.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 4, "{paged:?}");
    let first_cursor = first_cursor.expect("a second page");
    assert_refused(
        &call(
            "task_list",
            json!({"projectIds": ["ALPHA", "BETA"], "limit": 1, "priority": "low", "cursor": first_cursor}),
        )
        .await,
        "cursor",
        "different search",
    );
    let legacy = list(json!({"projectIds": ["ALPHA", "BETA"], "limit": 1, "cursor": "1"})).await;
    assert_eq!(keys(&legacy["tasks"]), [paged[1].clone()], "{legacy}");

    // Plain-words refusals.
    assert_refused(
        &call("task_list", json!({"status": "Doing"})).await,
        "status",
        "Statuses:",
    );
    assert_eq!(
        keys(&list(json!({"status": "started"})).await["tasks"]),
        Vec::<String>::new()
    );
    assert_refused(
        &call(
            "task_list",
            json!({"projectId": "ALPHA", "projectIds": ["BETA"]}),
        )
        .await,
        "projectIds",
        "not both",
    );
    assert_refused(
        &call("task_list", json!({"projectIds": []})).await,
        "projectIds",
        "1 to 50",
    );
    assert_refused(
        &call("task_list", json!({"priority": "critical"})).await,
        "priority",
        "urgent",
    );

    // Mara reaches ALPHA only: naming BETA, by key or ID, is not found, and
    // her lists and counts never include it.
    for beta_reference in [json!("BETA"), json!(beta)] {
        let refused = as_mara(
            "task_list",
            json!({"projectIds": ["ALPHA", beta_reference]}),
        )
        .await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
    }
    let refused = as_mara("task_list", json!({"projectIds": ["ALPHA", "BETA"]})).await;
    assert!(message(&refused).contains("BETA"), "{refused}");
    let hers = as_mara("task_list", json!({})).await;
    assert_eq!(hers["value"]["total"], 3, "{hers}");
    for row in hers["value"]["tasks"].as_array().expect("tasks") {
        assert_eq!(row["project"], "Alpha", "{hers}");
    }
    let her_labelled = as_mara("task_list", json!({"labels": ["mobile"]})).await;
    assert_eq!(
        keys(&her_labelled["value"]["tasks"]),
        ["ALPHA-1"],
        "{her_labelled}"
    );

    // Names are lowercased by the database on both sides of the match. Rust
    // lowercases a final capital sigma to ς, the database to σ, so a name
    // lowercased in Rust would match no task.
    let road = call("label_write", json!({"projectId": "BETA", "name": "ΟΔΟΣ"})).await;
    assert_eq!(road["outcome"], "ok", "{road}");
    let tagged = call(
        "task_update",
        json!({"taskId": "BETA-1", "expectedVersion": version(&setup, created[3]).await, "addLabels": ["ΟΔΟΣ"]}),
    )
    .await;
    assert_eq!(tagged["outcome"], "applied", "{tagged}");
    let roads = list(json!({"labels": ["ΟΔΟΣ"]})).await;
    assert_eq!(keys(&roads["tasks"]), ["BETA-1"], "{roads}");
}

#[tokio::test]
async fn task_reorder_places_tasks_beside_neighbours_and_rebalances() {
    let Some(setup) = setup("mcp_board_reorder").await else {
        return;
    };
    let app = &setup.app;
    let order = create_project(&setup, "ORD", "Order").await;
    create_project(&setup, "OTHER", "Other").await;
    create_project(&setup, "NEW", "New").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    for (project, title) in [
        ("ORD", "One"),
        ("ORD", "Two"),
        ("ORD", "Three"),
        ("ORD", "Four"),
        ("ORD", "Five"),
        ("OTHER", "Elsewhere"),
        ("NEW", "Already here"),
    ] {
        let created = call("task_create", json!({"projectId": project, "title": title})).await;
        assert_eq!(created["outcome"], "applied", "{created}");
    }
    let mut tasks = Vec::new();
    for number in 1..=5 {
        tasks.push(task_id(&setup, &order, number).await);
    }
    let versions = || {
        let (tasks, setup) = (&tasks, &setup);
        async move {
            let mut found = Vec::new();
            for task in tasks {
                found.push(version(setup, *task).await);
            }
            found
        }
    };
    let column = || {
        let call = &call;
        async move {
            let listed = call(
                "task_list",
                json!({"projectId": "ORD", "status": "Backlog"}),
            )
            .await;
            assert_eq!(listed["outcome"], "ok", "{listed}");
            keys(&listed["value"]["tasks"])
        }
    };
    let reorder = |arguments: Value| call("task_reorder", arguments);
    assert_eq!(
        column().await,
        ["ORD-1", "ORD-2", "ORD-3", "ORD-4", "ORD-5"]
    );

    // Just above a neighbour: only the task placed gets a new version.
    let placed = reorder(json!({"taskIds": ["ORD-5"], "beforeTaskId": "ORD-2"})).await;
    assert_eq!(placed["outcome"], "ok", "{placed}");
    assert_eq!(
        keys(&placed["value"]["column"]),
        ["ORD-1", "ORD-5", "ORD-2", "ORD-3", "ORD-4"]
    );
    assert_eq!(placed["value"]["changed"], true);
    assert_eq!(placed["value"]["status"], "Backlog");
    assert_eq!(
        placed["value"]["tasks"],
        json!([{"id": tasks[4].to_string(), "key": "ORD-5", "version": 2, "boardPosition": 2}])
    );
    assert_eq!(versions().await, [1, 1, 1, 1, 2]);
    let activity = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM work_item_activity WHERE work_item_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tasks[4])
    .fetch_one(&setup.pool)
    .await
    .expect("activity");
    assert_eq!(
        activity,
        json!({"changedFields": ["position"], "position": 2})
    );

    // Several, just below a neighbour, in the order given.
    let placed = reorder(json!({"taskIds": ["ORD-1", "ORD-3"], "afterTaskId": "ORD-4"})).await;
    assert_eq!(
        keys(&placed["value"]["column"]),
        ["ORD-5", "ORD-2", "ORD-4", "ORD-1", "ORD-3"],
        "{placed}"
    );
    assert_eq!(versions().await, [2, 1, 2, 1, 2]);
    // Each live event names the neighbours of the finished order, so a board
    // applying them in turn ends up the same.
    let placement = sqlx::query_scalar::<_, Value>(
        "SELECT payload -> 'placement' FROM outbox_events WHERE event_type = 'work_item.updated' AND resource_id = $1 ORDER BY created_at DESC, event_id DESC LIMIT 1",
    )
    .bind(tasks[0])
    .fetch_one(&setup.pool)
    .await
    .expect("placement");
    assert_eq!(
        placement["afterId"],
        json!(tasks[3].to_string()),
        "{placement}"
    );
    assert_eq!(
        placement["beforeId"],
        json!(tasks[2].to_string()),
        "{placement}"
    );

    // The same call again, or a task already on top, changes nothing.
    let again = reorder(json!({"taskIds": ["ORD-1", "ORD-3"], "afterTaskId": "ORD-4"})).await;
    assert_eq!(again["value"]["changed"], false, "{again}");
    assert_eq!(again["value"]["tasks"][0]["version"], 2);
    assert_eq!(again["value"]["tasks"][1]["boardPosition"], 5);
    let top = reorder(json!({"taskIds": ["ORD-5"]})).await;
    assert_eq!(top["value"]["changed"], false, "{top}");
    assert_eq!(versions().await, [2, 1, 2, 1, 2]);

    // No room between neighbours: the column is renumbered in place, and
    // only the task placed changes version.
    for (rank, number) in [5, 2, 4, 1, 3].into_iter().enumerate() {
        sqlx::query(
            "UPDATE work_items SET board_rank = $3 WHERE project_id = $1::uuid AND number = $2",
        )
        .bind(&order)
        .bind(number)
        .bind(i64::try_from(rank + 1).expect("rank"))
        .execute(&setup.pool)
        .await
        .expect("squeeze ranks");
    }
    let squeezed = reorder(json!({"taskIds": ["ORD-3"], "beforeTaskId": "ORD-2"})).await;
    assert_eq!(
        keys(&squeezed["value"]["column"]),
        ["ORD-5", "ORD-3", "ORD-2", "ORD-4", "ORD-1"],
        "{squeezed}"
    );
    assert_eq!(
        column().await,
        ["ORD-5", "ORD-3", "ORD-2", "ORD-4", "ORD-1"]
    );
    assert_eq!(versions().await, [2, 1, 3, 1, 2]);
    let (distinct, lowest) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(DISTINCT board_rank), min(board_rank) FROM work_items WHERE project_id = $1::uuid AND archived_at IS NULL",
    )
    .bind(&order)
    .fetch_one(&setup.pool)
    .await
    .expect("ranks");
    assert_eq!(distinct, 5);
    assert!(lowest > 0);

    // The web board's move still lands where it says.
    let backlog = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_statuses WHERE project_id = $1::uuid AND name = 'Backlog'",
    )
    .bind(&order)
    .fetch_one(&setup.pool)
    .await
    .expect("backlog");
    let (status, moved) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{order}/work-items/{}/move", tasks[0]),
            Some("board-web-move"),
            json!({"expectedVersion": 2, "targetStatusId": backlog, "targetPosition": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(
        column().await,
        ["ORD-1", "ORD-5", "ORD-3", "ORD-2", "ORD-4"]
    );

    // Refusals say what's wrong, and write nothing.
    let ready = call(
        "task_update",
        json!({"taskId": "ORD-4", "expectedVersion": 1, "status": "Ready", "clientRequestId": "ord-4-ready"}),
    )
    .await;
    assert_eq!(ready["outcome"], "applied", "{ready}");
    let before = versions().await;
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1"], "beforeTaskId": "ORD-4"})).await,
        "beforeTaskId",
        "ORD-4 is in Ready, but these tasks are in Backlog.",
    );
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1", "ORD-2"], "afterTaskId": "ORD-2"})).await,
        "afterTaskId",
        "ORD-2 is one of the tasks you're moving.",
    );
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1"], "beforeTaskId": "ORD-2", "afterTaskId": "ORD-3"}))
            .await,
        "afterTaskId",
        "not both",
    );
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1"], "position": "top", "beforeTaskId": "ORD-2"})).await,
        "position",
        "not both",
    );
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1"], "beforeTaskId": "OTHER-1"})).await,
        "beforeTaskId",
        "OTHER-1 is in another project.",
    );
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1", "ORD-4"]})).await,
        "taskIds",
        "different columns (Backlog, Ready)",
    );
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1"], "position": "middle"})).await,
        "position",
        "top or bottom",
    );
    let missing = reorder(json!({"taskIds": ["ORD-1"], "afterTaskId": "ORD-99"})).await;
    assert_eq!(missing["outcome"], "not_found", "{missing}");
    assert_eq!(versions().await, before);
    let archived = call(
        "task_update",
        json!({"taskId": "ORD-5", "expectedVersion": 2, "archived": true, "clientRequestId": "ord-5-archive"}),
    )
    .await;
    assert_eq!(archived["outcome"], "applied", "{archived}");
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-5"]})).await,
        "taskIds",
        "ORD-5 is archived.",
    );
    assert_refused(
        &reorder(json!({"taskIds": ["ORD-1"], "afterTaskId": "ORD-5"})).await,
        "afterTaskId",
        "ORD-5 is archived.",
    );

    // A read-only connection can't reorder.
    let refused = tool(
        app,
        &reader,
        &reader_session,
        "task_reorder",
        json!({"taskIds": ["ORD-1"], "position": "bottom"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(column().await, ["ORD-1", "ORD-3", "ORD-2"]);

    // A moved task's old key places the task where it went.
    let moved = call(
        "task_move",
        json!({"taskId": "ORD-2", "toProjectId": "NEW"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let new_key = moved["value"]["moved"][0]["to"]
        .as_str()
        .expect("new key")
        .to_owned();
    let followed = reorder(json!({"taskIds": ["ORD-2"], "position": "top"})).await;
    assert_eq!(followed["outcome"], "ok", "{followed}");
    assert_eq!(
        followed["value"]["column"][0],
        new_key.as_str(),
        "{followed}"
    );
    assert_eq!(
        keys(&followed["value"]["column"]),
        [new_key, "NEW-1".to_owned()]
    );
}

#[tokio::test]
async fn task_link_repositories_pull_requests_and_threads() {
    let Some(setup) = setup("mcp_board_link").await else {
        return;
    };
    let app = &setup.app;
    let repo = create_project(&setup, "REPO", "Repo").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let owner_id = owner(&setup, &repo).await;
    let r1 = insert_repository(&setup, &repo, "Mischa-dev/aw", true, true, None).await;
    let r2 = insert_repository(&setup, &repo, "Mischa-dev/raw", false, false, None).await;
    let r3 = insert_repository(&setup, &repo, "Mischa-dev/old", true, true, Some(owner_id)).await;
    insert_repository(&setup, &repo, "Fork-org/aw", true, true, None).await;
    let r4 = insert_repository(&setup, &other, "Mischa-dev/elsewhere", true, true, None).await;
    let (p1, p1_url) = seed_pull_request(&setup, &repo, r1, 7).await;
    let (p2, _) = seed_pull_request(&setup, &other, r4, 8).await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &repo, mara, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let created = call(
        "task_create",
        json!({"projectId": "REPO", "title": "Offline mode"}),
    )
    .await;
    let task = id(&created["workItemId"]);
    let link = |extra: Value| {
        let mut arguments = json!({"taskId": "REPO-1"});
        for (key, value) in extra.as_object().expect("object") {
            arguments[key] = value.clone();
        }
        call("task_link", arguments)
    };
    let primary_of = || {
        let pool = setup.pool.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT repository_id FROM work_item_repository_links WHERE work_item_id = $1 AND role = 'primary'",
            )
            .bind(task)
            .fetch_all(&pool)
            .await
            .expect("primary")
        }
    };

    // A repository by owner/name, made primary: one version bump.
    let linked = link(json!({"repositoryId": "mischa-dev/aw", "role": "primary"})).await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    let value = &linked["value"];
    assert_eq!(value["kind"], "repository");
    assert_eq!(value["role"], "primary");
    assert_eq!(value["changed"], true);
    assert_eq!(value["version"], 2);
    assert_eq!(value["key"], "REPO-1");
    assert_eq!(id(&value["targetId"]), r1);
    let opened = call("task_open", json!({"taskId": "REPO-1"})).await;
    let repositories = &opened["value"]["task"]["linkedRepositories"];
    assert_eq!(repositories[0]["role"], "primary", "{opened}");
    assert_eq!(id(&repositories[0]["repository"]["id"]), r1);

    // Names are looked up in the task's project only, and must be clear.
    assert_refused(
        &link(json!({"repositoryId": "aw"})).await,
        "repositoryId",
        "Fork-org/aw, Mischa-dev/aw",
    );
    let unknown = link(json!({"repositoryId": "nope"})).await;
    assert_eq!(unknown["outcome"], "not_found", "{unknown}");
    assert!(
        message(&unknown).contains("Repositories here: Fork-org/aw, Mischa-dev/aw"),
        "{unknown}"
    );
    assert_refused(
        &link(json!({"pageId": Uuid::now_v7(), "role": "primary"})).await,
        "role",
        "only applies to repositoryId",
    );
    assert_refused(
        &link(json!({"repositoryId": "raw", "role": "main"})).await,
        "role",
        "primary or related",
    );

    // Only a trusted, ready repository can be primary; related is fine.
    let untrusted = link(json!({"repositoryId": "raw", "role": "primary"})).await;
    assert_eq!(untrusted["outcome"], "conflict", "{untrusted}");
    assert!(
        message(&untrusted).contains("it isn't trusted"),
        "{untrusted}"
    );
    let related = link(json!({"repositoryId": "raw"})).await;
    assert_eq!(related["value"]["role"], "related", "{related}");
    assert_eq!(related["value"]["version"], 3);
    let again =
        link(json!({"repositoryId": "mischa-dev/aw", "role": "primary", "expectedVersion": 1}))
            .await;
    assert_eq!(again["value"]["changed"], false, "{again}");
    assert_eq!(again["value"]["version"], 3);

    // Relinking without a role keeps the role a link has: the primary stays
    // primary, even once it's no longer eligible to be made primary.
    sqlx::query("UPDATE repositories SET trusted = false WHERE id = $1")
        .bind(r1)
        .execute(&setup.pool)
        .await
        .expect("distrust aw");
    let relinked = link(json!({"repositoryId": "mischa-dev/aw"})).await;
    assert_eq!(relinked["outcome"], "ok", "{relinked}");
    assert_eq!(relinked["value"]["changed"], false, "{relinked}");
    assert_eq!(relinked["value"]["version"], 3);
    assert_eq!(relinked["value"]["role"], "primary");
    assert_eq!(primary_of().await, [r1]);
    sqlx::query("UPDATE repositories SET trusted = true WHERE id = $1")
        .bind(r1)
        .execute(&setup.pool)
        .await
        .expect("trust aw");
    let related_again = link(json!({"repositoryId": "raw", "expectedVersion": 1})).await;
    assert_eq!(related_again["value"]["changed"], false, "{related_again}");
    assert_eq!(related_again["value"]["role"], "related");

    // A new primary demotes the old one, in the same change, and says so.
    sqlx::query(
        "UPDATE repositories SET trusted = true, preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(r2)
    .execute(&setup.pool)
    .await
    .expect("trust raw");
    let swapped = link(json!({"repositoryId": r2, "role": "primary", "expectedVersion": 3})).await;
    assert_eq!(swapped["value"]["version"], 4, "{swapped}");
    assert_eq!(
        swapped["value"]["notes"],
        json!(["Mischa-dev/aw was primary; it's now related."])
    );
    assert_eq!(primary_of().await, [r2]);

    // Archived repositories and other projects' repositories stay out.
    let archived_repository = link(json!({"repositoryId": "old"})).await;
    assert_eq!(
        archived_repository["outcome"], "conflict",
        "{archived_repository}"
    );
    assert!(message(&archived_repository).contains("archived"));
    let elsewhere = link(json!({"repositoryId": r4})).await;
    assert_ne!(elsewhere["outcome"], "ok", "{elsewhere}");
    let stray = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_repository_links WHERE repository_id = $1",
    )
    .bind(r4)
    .fetch_one(&setup.pool)
    .await
    .expect("links");
    assert_eq!(stray, 0);

    // Unlinking is audited as an MCP change, with its own activity.
    let removed = link(json!({"repositoryId": r2, "remove": true})).await;
    assert_eq!(removed["value"]["linked"], false, "{removed}");
    assert_eq!(removed["value"]["version"], 5);
    assert!(removed["value"].get("role").is_none(), "{removed}");
    let (activity, audits) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM work_item_activity
                WHERE work_item_id = $1 AND kind = 'repository_unlinked'),
               (SELECT count(*) FROM audit_events
                WHERE target_id = $1 AND channel = 'mcp'
                  AND action = 'work_item.mcp_repository_unlinked')
        "#,
    )
    .bind(task)
    .fetch_one(&setup.pool)
    .await
    .expect("unlink records");
    assert_eq!((activity, audits), (1, 1));
    assert_eq!(primary_of().await, Vec::<Uuid>::new());

    // A pull request by its GitHub URL, with what the automation will do.
    let pr = link(json!({"pullRequestId": format!("{p1_url}/?tab=checks")})).await;
    assert_eq!(pr["outcome"], "ok", "{pr}");
    assert_eq!(pr["value"]["kind"], "pull_request");
    assert_eq!(id(&pr["value"]["targetId"]), p1);
    assert_eq!(pr["value"]["version"], 6);
    assert!(
        message_list(&pr["value"]["notes"])
            .contains("REPO-1 moves to Verification when this pull request is merged"),
        "{pr}"
    );
    let opened = call("task_open", json!({"taskId": "REPO-1"})).await;
    assert_eq!(
        id(&opened["value"]["task"]["linkedPullRequests"][0]["id"]),
        p1,
        "{opened}"
    );
    // The older tool sees the same link and keeps its contract.
    let alias_arguments = json!({
        "taskId": "REPO-1",
        "expectedVersion": 6,
        "pullRequestId": p1,
        "clientRequestId": "pr-1"
    });
    let alias = call("task_link_pull_request", alias_arguments.clone()).await;
    assert_eq!(alias["outcome"], "applied", "{alias}");
    assert_eq!(alias["changedFields"], json!([]));
    assert_eq!(alias["version"], 6);
    let replay = call("task_link_pull_request", alias_arguments).await;
    assert_eq!(replay["idempotentReplay"], true, "{replay}");
    let unlinked = link(json!({"pullRequestId": p1, "remove": true})).await;
    assert_eq!(unlinked["value"]["changed"], true, "{unlinked}");
    let pr_links = || {
        let pool = setup.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM github_pull_request_work_item_links")
                .fetch_one(&pool)
                .await
                .expect("pull request links")
        }
    };
    assert_eq!(pr_links().await, 0);
    let other_pr = link(json!({"pullRequestId": p2})).await;
    assert_eq!(other_pr["outcome"], "not_found", "{other_pr}");
    assert!(
        message(&other_pr).contains("REPO-1's project"),
        "{other_pr}"
    );
    assert_eq!(pr_links().await, 0);

    // Pages: a stale version is refused only when something would change.
    let page = call(
        "page_write",
        json!({"projectId": "REPO", "title": "Offline notes", "body": "Plan."}),
    )
    .await;
    let page_id = page["value"]["id"].as_str().expect("page").to_owned();
    let current = version(&setup, task).await;
    let stale = link(json!({"pageId": page_id, "expectedVersion": 1})).await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert!(
        message(&stale).contains(&format!(
            "Someone changed REPO-1 first (now version {current})"
        )),
        "{stale}"
    );
    let paged = link(json!({"pageId": page_id, "expectedVersion": current})).await;
    assert_eq!(paged["value"]["changed"], true, "{paged}");
    assert_eq!(paged["value"]["version"], current + 1);
    let payload = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM work_item_activity WHERE work_item_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(task)
    .fetch_one(&setup.pool)
    .await
    .expect("payload");
    assert_eq!(
        payload,
        json!({"link": {"kind": "page", "id": page_id}, "linked": true})
    );
    let retried = link(json!({"pageId": page_id, "expectedVersion": current})).await;
    assert_eq!(retried["value"]["changed"], false, "{retried}");
    assert_eq!(retried["value"]["version"], current + 1);

    // Threads in the task's project bump the version like the web's links,
    // and a thread in a channel the caller isn't in stays out of reach.
    let mut threads = Vec::new();
    for name in ["design", "secret"] {
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
    let before = version(&setup, task).await;
    let threaded = link(json!({"conversationId": threads[0].1})).await;
    assert_eq!(threaded["value"]["version"], before + 1, "{threaded}");
    let thread_audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE target_id = $1 AND action = 'work_item.mcp_thread_linked'",
    )
    .bind(task)
    .fetch_one(&setup.pool)
    .await
    .expect("thread audit");
    assert_eq!(thread_audits, 1);
    let unthreaded = link(json!({"conversationId": threads[0].1, "remove": true})).await;
    assert_eq!(unthreaded["value"]["version"], before + 2, "{unthreaded}");
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1::uuid")
        .bind(&threads[1].0)
        .execute(&setup.pool)
        .await
        .expect("make secret private");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1::uuid, $2::uuid, $3, $3)",
    )
    .bind(&repo)
    .bind(&threads[1].0)
    .bind(owner_id)
    .execute(&setup.pool)
    .await
    .expect("owner stays in secret");
    let hidden = tool(
        app,
        &mara_token,
        &mara_session,
        "task_link",
        json!({"taskId": "REPO-1", "conversationId": threads[1].1}),
    )
    .await;
    assert_eq!(hidden["outcome"], "not_found", "{hidden}");
    let visible_to_owner = link(json!({"conversationId": threads[1].1})).await;
    assert_eq!(visible_to_owner["outcome"], "ok", "{visible_to_owner}");
    // task_link checks visibility before the command, but the web app's
    // thread links rely on change_thread_link alone, so the command itself
    // must refuse a thread its actor can't see, linking or unlinking.
    let project_id: Uuid = repo.parse().expect("project id");
    let secret: Uuid = threads[1].1.parse().expect("thread id");
    let actor = |actor_id, actor_kind| CommandActor {
        actor_id,
        actor_kind,
        project_id,
        audit: AuditAttribution::Mcp {
            authentication_class: "mcp_bearer",
            request_id: "thread-visibility".to_owned(),
            client_name: "test".to_owned(),
            mcp_credential_id: None,
            oauth_grant_id: None,
            external_agent_connection_id: None,
        },
    };
    let mut transaction = setup.pool.begin().await.expect("transaction");
    let current = lock_work_item(&mut transaction, project_id, task)
        .await
        .expect("lock task")
        .expect("task");
    for linked in [true, false] {
        let outcome = change_thread_link(
            &mut transaction,
            &actor(mara, ActorKind::ExternalAgent),
            &current,
            None,
            secret,
            linked,
            LinkWording::WEB,
        )
        .await
        .expect("change thread link");
        assert!(
            matches!(outcome, ThreadLinkOutcome::ThreadNotFound),
            "{outcome:?}"
        );
    }
    let owner_sees = change_thread_link(
        &mut transaction,
        &actor(owner_id, ActorKind::Human),
        &current,
        None,
        secret,
        true,
        LinkWording::WEB,
    )
    .await
    .expect("change thread link");
    assert!(
        matches!(owner_sees, ThreadLinkOutcome::Unchanged),
        "{owner_sees:?}"
    );
    transaction.rollback().await.expect("rollback");

    // The web app refuses new links to archived repositories, and link
    // changes on an archived task, except unlinking a thread: the thread
    // page offers Unlink on every task linked to it, archived ones too.
    let web_version = version(&setup, task).await;
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{repo}/work-items/{task}/repositories/{r3}"),
            Some("board-web-archived-repository"),
            json!({"expectedVersion": web_version, "role": "related"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let (status, archived) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{repo}/work-items/{task}/archive"),
            Some("board-web-archive"),
            json!({"expectedVersion": web_version}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    let on_archived = link(json!({"pageId": page_id, "remove": true})).await;
    assert_eq!(on_archived["outcome"], "conflict", "{on_archived}");
    assert!(message(&on_archived).contains("archived"), "{on_archived}");
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{repo}/work-items/{task}/repositories/{r1}"),
            Some("board-web-archived-task"),
            json!({"expectedVersion": web_version + 1, "role": "related"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let pool = &setup.pool;
    let thread_linked = move || async move {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_conversation_links WHERE work_item_id = $1 AND conversation_id = $2)",
        )
        .bind(task)
        .bind(secret)
        .fetch_one(pool)
        .await
        .expect("thread link")
    };
    assert!(thread_linked().await);
    let thread_link = format!(
        "/api/v1/projects/{repo}/work-items/{task}/threads/{}",
        threads[1].1
    );
    let (status, unlinked) = send(
        app,
        setup.browser.request(
            "DELETE",
            &thread_link,
            Some("board-web-archived-unlink"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unlinked}");
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &thread_link,
            Some("board-web-archived-link"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(!thread_linked().await);

    // A read-only connection can't link anything.
    let read_only = tool(
        app,
        &reader,
        &reader_session,
        "task_link",
        json!({"taskId": "REPO-1", "pageId": page_id}),
    )
    .await;
    assert_eq!(read_only["outcome"], "permission_denied", "{read_only}");
}

fn message_list(notes: &Value) -> String {
    notes
        .as_array()
        .unwrap_or_else(|| panic!("no notes: {notes}"))
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

/// An older project-scoped credential (a bearer token, or a grant from
/// before connections covered the whole account) stays in its project. Keys
/// of the person's other projects resolve to nothing there, so a refusal
/// names them back as written and never hands over their IDs.
#[tokio::test]
async fn a_project_scoped_credential_never_learns_ids_beyond_its_project() {
    let Some(setup) = setup("mcp_board_scoped").await else {
        return;
    };
    let app = &setup.app;
    let home = create_project(&setup, "HOME", "Home").await;
    let secret = create_project(&setup, "SECRET", "Secret").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    for project in ["HOME", "SECRET"] {
        let created = tool(
            app,
            &codex,
            &session,
            "task_create",
            json!({"projectId": project, "title": "Start"}),
        )
        .await;
        assert_eq!(created["outcome"], "applied", "{created}");
    }
    let secret_task = task_id(&setup, &secret, 1).await.to_string();
    let scoped = mcp::issue_credential(
        &setup.pool,
        IssueCredentialCommand {
            project_id: home.parse().expect("home ID"),
            actor_id: owner(&setup, &home).await,
            client_name: "scoped-test".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("issue credential")
    .token;
    let scoped_session = initialize(app, &scoped).await.expect("scoped MCP");
    let call = |name: &'static str, arguments: Value| {
        let scoped = scoped.clone();
        let session = scoped_session.clone();
        async move { tool(app, &scoped, &session, name, arguments).await }
    };

    let own = call("task_list", json!({"projectIds": ["HOME"]})).await;
    assert_eq!(own["outcome"], "ok", "{own}");
    assert_eq!(keys(&own["value"]["tasks"]), ["HOME-1"]);

    let listed = call("task_list", json!({"projectIds": ["HOME", "SECRET"]})).await;
    let reordered = call(
        "task_reorder",
        json!({"taskIds": ["HOME-1"], "beforeTaskId": "SECRET-1"}),
    )
    .await;
    let opened = call("task_open", json!({"taskId": "SECRET-1"})).await;
    for refused in [&listed, &reordered, &opened] {
        assert_eq!(refused["outcome"], "not_found", "{refused}");
        let text = refused.to_string();
        assert!(
            !text.contains(&secret) && !text.contains(&secret_task),
            "{refused}"
        );
    }
    assert!(message(&listed).starts_with("Project SECRET "), "{listed}");
    assert!(
        message(&reordered).starts_with("Task SECRET-1 "),
        "{reordered}"
    );
}

/// task_claim takes its turn on the project row before it locks the task, as
/// every task writer does. Locked the other way round, a claim deadlocked
/// with a status change or reorder whose column held the task: the writer
/// held the project and waited for the task, while the claim held the task
/// and its insert waited on the project row for its foreign key.
#[tokio::test]
async fn a_claim_waits_behind_a_write_to_its_column() {
    let Some(setup) = setup("mcp_board_claim").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let owner_id = owner(&setup, &work).await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let created = tool(
        app,
        &mara_token,
        &mara_session,
        "task_create",
        json!({"projectId": "WORK", "title": "Claim me"}),
    )
    .await;
    assert_eq!(created["outcome"], "applied", "{created}");
    let project: Uuid = work.parse().expect("project ID");
    let claimed = task_id(&setup, &work, 1).await;
    let column = sqlx::query_scalar::<_, Uuid>("SELECT status_id FROM work_items WHERE id = $1")
        .bind(claimed)
        .fetch_one(&setup.pool)
        .await
        .expect("status");

    // A write that moves a task into this column holds the project first.
    let mut writer = setup.pool.begin().await.expect("writer");
    let writer_pid = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .expect("writer pid");
    lock_membership(&mut writer, owner_id, project)
        .await
        .expect("lock project")
        .expect("owner membership");
    let claim = tokio::spawn({
        let app = app.clone();
        async move {
            tool(
                &app,
                &mara_token,
                &mara_session,
                "task_claim",
                json!({"taskId": "WORK-1"}),
            )
            .await
        }
    });
    let mut polls = 0;
    while !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))",
    )
    .bind(writer_pid)
    .fetch_one(&setup.pool)
    .await
    .expect("blocked sessions")
    {
        polls += 1;
        assert!(polls < 400, "the claim never waited for the writer");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // Then it locks the column, as place_in_column and task_reorder do. The
    // claim is waiting on the project, so it holds no task yet.
    sqlx::query("SET LOCAL lock_timeout = '10s'")
        .execute(&mut *writer)
        .await
        .expect("lock timeout");
    let locked = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM work_items
        WHERE project_id = $1 AND status_id = $2 AND archived_at IS NULL
        ORDER BY board_rank, id
        FOR UPDATE
        "#,
    )
    .bind(project)
    .bind(column)
    .fetch_all(&mut *writer)
    .await
    .expect("the column locks without a deadlock");
    assert_eq!(locked, [claimed]);
    writer.commit().await.expect("writer commits");
    let claim = claim.await.expect("claim");
    assert_eq!(claim["outcome"], "ok", "{claim}");
}

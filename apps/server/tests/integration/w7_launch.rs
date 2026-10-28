//! W7: launching tasks. The project's default environment is preselected,
//! editing the launch template changes the next prompt, queued launches
//! start as the account's slots free up with blocked tasks last, and a
//! launch queued by someone who lost access to the project never starts.

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    git_runtime::expected_repository_mirror_path,
    launch,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    seed::dev_seed,
};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::{Executor, PgPool};
use uuid::Uuid;

use crate::{
    m3_scheduler_http::{create_test_mirror, integration_pool, schema_pool, uuid_field},
    w2_harness_platform::{MEMBER_LOGIN, insert_member, login, send, send_get},
};

const OWNER_LOGIN: &str = "w7-launch-owner";
const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn launches_use_the_defaults_queue_in_order_and_recheck_access() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("w7_launch_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create launch schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run launch migrations");

    let temporary = tempfile::tempdir().expect("temporary launch root");
    let data_root = temporary.path().join("data");
    let mut config = AppConfig::from_values(&BTreeMap::from([
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
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_DEMO_EXECUTOR".to_owned(),
            "true".to_owned(),
        ),
    ]))
    .expect("launch configuration");
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let seed = dev_seed(&pool, &config).await.expect("seed launch fixture");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare launch Repository");
    let mirror = expected_repository_mirror_path(&data_root, seed.repository_id)
        .expect("canonical launch mirror");
    create_test_mirror(temporary.path(), &mirror);
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OWNER_LOGIN.to_owned(),
            display_name: "W7 Launch Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap launch owner");
    insert_member(&pool, seed.organization_id, seed.project_id).await;

    let supervisors = SupervisorRegistry::new(&config).expect("launch supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    };
    let application = router(state.clone());
    let (owner_cookie, owner_csrf) = login(&application, OWNER_LOGIN).await;
    let (member_cookie, member_csrf) = login(&application, MEMBER_LOGIN).await;
    let owner = (owner_cookie.as_str(), owner_csrf.as_str());
    let member = (member_cookie.as_str(), member_csrf.as_str());
    let project = format!("/api/v1/projects/{}", seed.project_id);

    // A demo account with two run slots.
    let accounts = format!("{project}/provider-accounts");
    let created = send(
        &application,
        owner,
        "POST",
        &accounts,
        "create-demo",
        json!({"adapterKind": "demo"}),
    )
    .await;
    assert!(created.0.is_success(), "create account: {}", created.1);
    let account = format!("{accounts}/{}", uuid_field(&created.2, "id"));
    let signed_in = send(
        &application,
        owner,
        "POST",
        &format!("{account}/login/api-key"),
        "demo-sign-in",
        json!({"apiKey": "demo-key-for-tests"}),
    )
    .await;
    assert!(signed_in.0.is_success(), "sign in: {}", signed_in.1);
    let used = send(
        &application,
        owner,
        "POST",
        &format!("{account}/use"),
        "use-demo",
        json!({}),
    )
    .await;
    assert!(used.0.is_success(), "use account: {}", used.1);
    let limited = send(
        &application,
        owner,
        "PATCH",
        &account,
        "two-slots",
        json!({"expectedVersion": used.2["version"], "maxConcurrentRuns": 2}),
    )
    .await;
    assert_eq!(limited.0, StatusCode::OK, "{}", limited.1);

    // The first environment becomes the default, and launching preselects it.
    let environment = send(
        &application,
        owner,
        "POST",
        &format!("{project}/environments"),
        "environment",
        json!({"name": "Demo here", "harness": "demo", "setupNotes": "Nothing to set up."}),
    )
    .await;
    assert!(environment.0.is_success(), "environment: {}", environment.1);
    assert_eq!(environment.2["isDefault"], true);
    let environment_id = uuid_field(&environment.2, "id");
    let members_view = send_get(&application, member, &format!("{project}/environments")).await;
    assert_eq!(members_view.0, StatusCode::OK, "{}", members_view.1);
    assert_eq!(members_view.2["canManage"], false);
    let refused = send(
        &application,
        member,
        "POST",
        &format!("{project}/environments"),
        "member-environment",
        json!({"name": "Mine", "harness": "demo"}),
    )
    .await;
    assert!(
        matches!(refused.0, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a member changed environments: {}",
        refused.0
    );

    let mut tasks = Vec::new();
    for title in [
        "Design it",
        "Build it",
        "Test it",
        "Ship it",
        "Member's task",
    ] {
        let created = send(
            &application,
            owner,
            "POST",
            &format!("{project}/work-items"),
            &format!("task-{title}"),
            json!({"title": title, "acceptanceCriteria": ["Checked by someone"]}),
        )
        .await;
        assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
        tasks.push((
            uuid_field(&created.2, "id"),
            created.2["key"].as_str().expect("key").to_owned(),
        ));
    }
    let (design, build, test, ship, members_task) =
        (tasks[0].0, tasks[1].0, tasks[2].0, tasks[3].0, tasks[4].0);
    // Build waits on design.
    sqlx::query(
        r#"
        INSERT INTO work_item_dependencies (
            project_id, blocking_project_id, blocking_work_item_id, blocked_work_item_id,
            created_by_actor_id
        ) VALUES ($1, $1, $2, $3, $4)
        "#,
    )
    .bind(seed.project_id)
    .bind(design)
    .bind(build)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("block build on design");

    // The plan preselects the default environment and previews the prompt.
    let plan_uri = format!("{project}/work-items/{build}/launch");
    let plan = send_get(&application, owner, &plan_uri).await;
    assert_eq!(plan.0, StatusCode::OK, "{}", plan.1);
    assert_eq!(plan.2["environmentId"], environment_id.to_string());
    assert_eq!(plan.2["blockers"][0]["key"], tasks[0].1.as_str());
    let prompt = plan.2["prompt"].as_str().expect("prompt");
    assert!(prompt.contains("Build it"), "{prompt}");
    assert!(prompt.contains("- [ ] Checked by someone"), "{prompt}");

    // Editing the template changes the next preview.
    let template = send(
        &application,
        owner,
        "PUT",
        "/api/v1/settings/values/launch.template",
        "template",
        json!({
            "scope": "project",
            "projectId": seed.project_id,
            "value": "Do {{task.key}} on {{harness}}.\n{{#harness codex_cloud}}\nOpen a pull request.\n{{/harness}}"
        }),
    )
    .await;
    assert_eq!(template.0, StatusCode::OK, "{}", template.1);
    let plan = send_get(&application, owner, &plan_uri).await;
    assert_eq!(
        plan.2["prompt"],
        format!("Do {} on Demo harness.", tasks[1].1),
        "{}",
        plan.1
    );

    // Four queued tasks on two slots: design and test start, build waits for
    // design, ship waits for a slot.
    let queued = send(
        &application,
        owner,
        "POST",
        &format!("{project}/launches"),
        "queue-four",
        json!({"workItemIds": [build, design, test, ship]}),
    )
    .await;
    assert_eq!(queued.0, StatusCode::CREATED, "{}", queued.1);
    let launches = queued.2["launches"].as_array().expect("launches");
    assert_eq!(launches.len(), 4);
    assert!(launches.iter().all(|launch| launch["state"] == "queued"));
    // A retry replays rather than queueing again.
    let replayed = send(
        &application,
        owner,
        "POST",
        &format!("{project}/launches"),
        "queue-four",
        json!({"workItemIds": [build, design, test, ship]}),
    )
    .await;
    assert_eq!(replayed.2["launches"].as_array().map(Vec::len), Some(4));

    assert_eq!(launch::drain(&state).await.expect("drain"), 2);
    assert_eq!(launch_state(&pool, design).await.0, "started");
    assert_eq!(launch_state(&pool, test).await.0, "started");
    let (build_state, build_reason) = launch_state(&pool, build).await;
    assert_eq!(build_state, "queued");
    assert!(build_reason.contains(&tasks[0].1), "{build_reason}");
    let (ship_state, ship_reason) = launch_state(&pool, ship).await;
    assert_eq!(ship_state, "queued");
    assert!(ship_reason.contains("free slot"), "{ship_reason}");
    // A started launch's run uses the environment's harness, in a thread
    // linked to the task, with the template's prompt.
    let (run_prompt, harness, linked) = sqlx::query_as::<_, (String, Option<String>, bool)>(
        r#"
        SELECT run.prompt, run.config ->> 'harness',
               EXISTS (SELECT 1 FROM work_item_conversation_links link
                       WHERE link.work_item_id = launch.work_item_id
                         AND link.conversation_id = run.conversation_id)
        FROM launches launch JOIN agent_runs run ON run.id = launch.agent_run_id
        WHERE launch.work_item_id = $1
        "#,
    )
    .bind(design)
    .fetch_one(&pool)
    .await
    .expect("design's run");
    assert_eq!(run_prompt, format!("Do {} on Demo harness.", tasks[0].1));
    assert_eq!(harness.as_deref(), Some("demo"));
    assert!(linked);

    // Test's run ends: the slot goes to ship, while build still waits.
    finish_run(&pool, test).await;
    assert_eq!(launch::drain(&state).await.expect("drain"), 1);
    assert_eq!(launch_state(&pool, ship).await.0, "started");
    assert_eq!(launch_state(&pool, build).await.0, "queued");

    // Design is done and its run ends: build starts last.
    finish_run(&pool, design).await;
    sqlx::query(
        r#"
        UPDATE work_items SET status_id = (
            SELECT id FROM work_item_statuses
            WHERE project_id = $1 AND semantic_category = 'completed'
            ORDER BY position LIMIT 1
        )
        WHERE id = $2
        "#,
    )
    .bind(seed.project_id)
    .bind(design)
    .execute(&pool)
    .await
    .expect("finish design");
    assert_eq!(launch::drain(&state).await.expect("drain"), 1);
    assert_eq!(launch_state(&pool, build).await.0, "started");

    // A launch queued by someone who then loses access never starts.
    let members_launch = send(
        &application,
        member,
        "POST",
        &format!("{project}/launches"),
        "member-queues",
        json!({"workItemIds": [members_task], "queue": true}),
    )
    .await;
    assert_eq!(
        members_launch.0,
        StatusCode::CREATED,
        "{}",
        members_launch.1
    );
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND handle = 'w2-member'",
    )
    .bind(seed.project_id)
    .execute(&pool)
    .await
    .expect("remove member");
    finish_run(&pool, ship).await;
    launch::drain(&state).await.expect("drain");
    let (members_state, members_reason) = launch_state(&pool, members_task).await;
    assert_eq!(members_state, "cancelled");
    assert!(
        members_reason.contains("can no longer start runs"),
        "{members_reason}"
    );
    let runs_for_member_task = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM agent_runs run
        JOIN work_item_conversation_links link ON link.conversation_id = run.conversation_id
        WHERE link.work_item_id = $1
        "#,
    )
    .bind(members_task)
    .fetch_one(&pool)
    .await
    .expect("count member runs");
    assert_eq!(runs_for_member_task, 0);

    // The queue view shows what happened, newest first.
    let list = send_get(&application, owner, &format!("{project}/launches")).await;
    assert_eq!(list.0, StatusCode::OK, "{}", list.1);
    assert_eq!(list.2["queued"].as_array().map(Vec::len), Some(0));
    assert!(
        list.2["recent"]
            .as_array()
            .expect("recent")
            .iter()
            .any(|launch| launch["state"] == "cancelled")
    );

    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop launch schema");
}

async fn launch_state(pool: &PgPool, work_item_id: Uuid) -> (String, String) {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, status_reason FROM launches WHERE work_item_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(work_item_id)
    .fetch_one(pool)
    .await
    .map(|(state, reason)| (state, reason.unwrap_or_default()))
    .expect("read launch")
}

/// Ends the run a task's launch started, as if it finished.
async fn finish_run(pool: &PgPool, work_item_id: Uuid) {
    let updated = sqlx::query(
        r#"
        UPDATE agent_runs SET state = 'canceled', finished_at = now()
        WHERE id = (SELECT agent_run_id FROM launches
                    WHERE work_item_id = $1 AND state = 'started')
        "#,
    )
    .bind(work_item_id)
    .execute(pool)
    .await
    .expect("finish run");
    assert_eq!(updated.rows_affected(), 1);
}

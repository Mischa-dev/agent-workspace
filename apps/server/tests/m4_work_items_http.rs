use std::{collections::BTreeMap, sync::Arc};

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

const LOGIN_NAME: &str = "m4-work-item-owner";
const PASSWORD: &str = "correct horse battery staple";
const COLLABORATOR_LOGIN_NAME: &str = "m4-work-item-collaborator";

#[tokio::test]
async fn work_item_foundation_is_scoped_idempotent_ordered_and_actor_attributed() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m4_work_items_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M4 Work Item schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M4 Work Item migrations");

    let temporary = tempfile::tempdir().expect("temporary M4 runtime root");
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
    .expect("M4 Work Item test configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M4 Work Item fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M4 Work Item Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M4 Work Item owner");
    bootstrap_development_member(
        &pool,
        BootstrapCommand {
            login_name: COLLABORATOR_LOGIN_NAME.to_owned(),
            display_name: "M4 Work Item Collaborator".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.collaborator_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M4 Work Item collaborator");

    let supervisors = SupervisorRegistry::new(&config).expect("M4 supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors: supervisors.clone(),
    });
    let base = format!("/api/v1/projects/{}/work-items", seed.project_id);

    let unauthenticated = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(&base)
                .body(Body::empty())
                .expect("build unauthenticated Work Item request"),
        )
        .await
        .expect("unauthenticated Work Item response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let cookie = login(&application).await;
    let csrf = csrf_from_cookie(&cookie);
    let collaborator_cookie = login_as(&application, COLLABORATOR_LOGIN_NAME).await;
    let collaborator_csrf = csrf_from_cookie(&collaborator_cookie);
    let navigation_uri = format!("/api/v1/projects/{}/navigation", seed.project_id);
    let current_navigation = get_json(&application, &cookie, &navigation_uri).await;
    assert_eq!(current_navigation.0, StatusCode::OK);
    let work_item_navigation = put_json(
        &application,
        &cookie,
        &csrf,
        &navigation_uri,
        json!({
            "location": { "kind": "work_items" },
            "expectedVersion": current_navigation.2["version"],
            "filters": { "priority": "high", "archived": false }
        }),
    )
    .await;
    assert_eq!(work_item_navigation.0, StatusCode::OK);
    assert_eq!(work_item_navigation.2["location"]["kind"], "work_items");
    assert_eq!(
        work_item_navigation.2["href"],
        format!("/projects/{}/work-items", seed.project_id)
    );
    assert_eq!(work_item_navigation.2["filters"]["priority"], "high");
    let workflow = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{}/work-item-workflow", seed.project_id),
    )
    .await;
    assert_eq!(workflow.0, StatusCode::OK);
    assert_eq!(
        workflow.2["types"]
            .as_array()
            .expect("workflow types array")
            .len(),
        6
    );
    assert_eq!(
        workflow.2["statuses"]
            .as_array()
            .expect("workflow statuses array")
            .len(),
        7
    );
    assert_eq!(
        workflow.2["transitions"]
            .as_array()
            .expect("workflow transitions array")
            .len(),
        17
    );
    assert_eq!(
        workflow.2["types"]
            .as_array()
            .expect("workflow types array")
            .iter()
            .filter(|item| item["isDefault"] == true)
            .count(),
        1
    );
    let status_id = |name: &str| {
        workflow.2["statuses"]
            .as_array()
            .expect("workflow statuses array")
            .iter()
            .find(|status| status["name"] == name)
            .and_then(|status| status["id"].as_str())
            .map(str::to_owned)
            .expect("named workflow status")
    };
    let ready_status_id = status_id("Ready");
    let in_progress_status_id = status_id("In Progress");
    let done_status_id = status_id("Done");
    assert_eq!(
        workflow.2["statuses"]
            .as_array()
            .expect("workflow statuses array")
            .iter()
            .filter(|item| item["isDefault"] == true)
            .count(),
        1
    );

    let request = json!({
        "title": "First Work Item",
        "description": "A bounded description.",
        "priority": "high",
        "assigneeActorId": seed.human_actor_id
    });
    let missing_csrf = post_json(
        &application,
        &cookie,
        None,
        Some("m4-create-one"),
        &base,
        request.clone(),
    )
    .await;
    assert_eq!(missing_csrf.0, StatusCode::FORBIDDEN);

    let created = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-create-one"),
        &base,
        request.clone(),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    assert_eq!(created.2["key"], "AW-1");
    assert_eq!(created.2["number"], 1);
    assert_eq!(created.2["priority"], "high");
    assert_eq!(
        created.2["assignee"]["actorId"],
        seed.human_actor_id.to_string()
    );
    assert_eq!(created.2["version"], 1);
    assert!(created.2.get("createdByActorId").is_none());

    let by_key = get_json(
        &application,
        &cookie,
        &format!(
            "/api/v1/projects/{}/work-items/by-key/AW-1",
            seed.project_id
        ),
    )
    .await;
    assert_eq!(by_key.0, StatusCode::OK);
    assert_eq!(by_key.2["id"], created.2["id"]);

    let assignees = get_json(
        &application,
        &cookie,
        &format!(
            "/api/v1/projects/{}/work-item-assignees?q=M2",
            seed.project_id
        ),
    )
    .await;
    assert_eq!(assignees.0, StatusCode::OK);
    assert_eq!(
        assignees.2["actors"].as_array().expect("assignees").len(),
        1
    );
    assert!(assignees.2["actors"][0].get("email").is_none());

    let replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-create-one"),
        &base,
        request,
    )
    .await;
    assert_eq!(replay.0, StatusCode::CREATED);
    assert_eq!(replay.2["id"], created.2["id"]);

    let mismatch = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-create-one"),
        &base,
        json!({"title": "Conflicting payload"}),
    )
    .await;
    assert_eq!(mismatch.0, StatusCode::CONFLICT);

    let forbidden_transition = patch_json(
        &application,
        &cookie,
        &csrf,
        "m4-invalid-transition",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({"expectedVersion": 1, "statusId": done_status_id}),
    )
    .await;
    assert_eq!(forbidden_transition.0, StatusCode::CONFLICT);

    let updated = patch_json(
        &application,
        &collaborator_cookie,
        &collaborator_csrf,
        "m4-update-one",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({
            "expectedVersion": 1,
            "title": "First Work Item updated",
            "statusId": ready_status_id,
            "priority": "urgent",
            "assigneeActorId": null
        }),
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK, "{}", updated.1);
    assert_eq!(updated.2["version"], 2);
    assert_eq!(updated.2["status"]["name"], "Ready");
    assert_eq!(updated.2["priority"], "urgent");
    assert!(updated.2["assignee"].is_null());
    let update_activity = get_json(
        &application,
        &cookie,
        &format!(
            "{base}/{}/timeline?limit=10",
            created.2["id"].as_str().expect("created ID")
        ),
    )
    .await;
    assert!(
        update_activity.2["entries"]
            .as_array()
            .expect("Work Item timeline entries")
            .iter()
            .any(|entry| {
                entry["activity"]["kind"] == "work_item_updated"
                    && entry["activity"]["actor"]["id"] == seed.collaborator_actor_id.to_string()
            })
    );

    let update_replay = patch_json(
        &application,
        &collaborator_cookie,
        &collaborator_csrf,
        "m4-update-one",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({
            "expectedVersion": 1,
            "title": "First Work Item updated",
            "statusId": ready_status_id,
            "priority": "urgent",
            "assigneeActorId": null
        }),
    )
    .await;
    assert_eq!(update_replay.0, StatusCode::OK);
    assert_eq!(update_replay.2["version"], 2);

    let stale_update = patch_json(
        &application,
        &cookie,
        &csrf,
        "m4-stale-update",
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
        json!({"expectedVersion": 1, "title": "Lose this edit"}),
    )
    .await;
    assert_eq!(stale_update.0, StatusCode::CONFLICT);
    assert_eq!(stale_update.2["error"]["code"], "version_conflict");
    assert_eq!(stale_update.2["error"]["details"]["currentVersion"], 2);
    assert_eq!(
        stale_update.2["error"]["details"]["current"]["title"],
        "First Work Item updated"
    );

    let worker_assignee = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-worker-assignee"),
        &base,
        json!({
            "title": "Invalid assignee",
            "assigneeActorId": seed.worker_actor_id
        }),
    )
    .await;
    assert_eq!(worker_assignee.0, StatusCode::BAD_REQUEST);

    let application_one = application.clone();
    let application_two = application.clone();
    let cookie_one = cookie.clone();
    let cookie_two = cookie.clone();
    let csrf_one = csrf.clone();
    let csrf_two = csrf.clone();
    let base_one = base.clone();
    let base_two = base.clone();
    let (second, third) = tokio::join!(
        post_json(
            &application_one,
            &cookie_one,
            Some(&csrf_one),
            Some("m4-create-two"),
            &base_one,
            json!({"title": "Second Work Item"}),
        ),
        post_json(
            &application_two,
            &cookie_two,
            Some(&csrf_two),
            Some("m4-create-three"),
            &base_two,
            json!({"title": "Third Work Item"}),
        )
    );
    assert_eq!(second.0, StatusCode::CREATED, "{}", second.1);
    assert_eq!(third.0, StatusCode::CREATED, "{}", third.1);
    let mut concurrent_numbers = [
        second.2["number"]
            .as_i64()
            .expect("second Work Item number"),
        third.2["number"].as_i64().expect("third Work Item number"),
    ];
    concurrent_numbers.sort();
    assert_eq!(concurrent_numbers, [2, 3]);

    let first_page = get_json(&application, &cookie, &format!("{base}?limit=1")).await;
    assert_eq!(first_page.0, StatusCode::OK);
    assert_eq!(
        first_page.2["items"]
            .as_array()
            .expect("first Work Item page")
            .len(),
        1
    );
    let cursor = first_page.2["nextCursor"]
        .as_str()
        .expect("next Work Item cursor");
    let second_page = get_json(
        &application,
        &cookie,
        &format!("{base}?limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(second_page.0, StatusCode::OK);
    assert_eq!(
        second_page.2["items"]
            .as_array()
            .expect("second Work Item page")
            .len(),
        1
    );
    assert_ne!(
        first_page.2["items"][0]["id"],
        second_page.2["items"][0]["id"]
    );
    let references = get_json(
        &application,
        &cookie,
        &format!(
            "/api/v1/projects/{}/work-item-references?q=Work&limit=2",
            seed.project_id
        ),
    )
    .await;
    assert_eq!(references.0, StatusCode::OK);
    assert_eq!(
        references.2["items"].as_array().expect("references").len(),
        2
    );
    assert!(references.2["items"][0].get("description").is_none());
    let search = get_json(&application, &cookie, &format!("{base}?q=First")).await;
    assert_eq!(
        search.2["items"]
            .as_array()
            .expect("search Work Item page")
            .len(),
        1
    );
    assert_eq!(search.2["items"][0]["id"], created.2["id"]);
    for foreign_filter in ["typeId", "statusId", "assigneeActorId"] {
        let response = get_json(
            &application,
            &cookie,
            &format!("{base}?{foreign_filter}={}", Uuid::now_v7()),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST, "{}", response.1);
    }

    let detail = get_json(
        &application,
        &cookie,
        &format!(
            "{base}/{}",
            created.2["id"].as_str().expect("created Work Item ID")
        ),
    )
    .await;
    assert_eq!(detail.0, StatusCode::OK);
    assert_eq!(detail.2["key"], "AW-1");

    let archived = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-archive-one"),
        &format!(
            "{base}/{}/archive",
            created.2["id"].as_str().expect("created ID")
        ),
        json!({"expectedVersion": 2}),
    )
    .await;
    assert_eq!(archived.0, StatusCode::OK, "{}", archived.1);
    assert_eq!(archived.2["version"], 3);
    assert!(archived.2["archivedAt"].is_string());
    let archived_list = get_json(&application, &cookie, &format!("{base}?archived=true")).await;
    assert_eq!(archived_list.0, StatusCode::OK);
    assert_eq!(
        archived_list.2["items"]
            .as_array()
            .expect("archived Work Item list")
            .len(),
        1
    );
    let restored = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-restore-one"),
        &format!(
            "{base}/{}/restore",
            created.2["id"].as_str().expect("created ID")
        ),
        json!({"expectedVersion": 3}),
    )
    .await;
    assert_eq!(restored.0, StatusCode::OK, "{}", restored.1);
    assert_eq!(restored.2["version"], 4);
    assert!(restored.2["archivedAt"].is_null());
    let no_op_restore = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-restore-no-op"),
        &format!(
            "{base}/{}/restore",
            created.2["id"].as_str().expect("created ID")
        ),
        json!({"expectedVersion": 4}),
    )
    .await;
    assert_eq!(no_op_restore.0, StatusCode::OK);
    assert_eq!(no_op_restore.2["version"], 4);

    let criteria_uri = format!(
        "{base}/{}/acceptance-criteria",
        created.2["id"].as_str().expect("created ID")
    );
    let first_criterion = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-criterion-one"),
        &criteria_uri,
        json!({"expectedVersion": 4, "text": "The browser renders the durable item"}),
    )
    .await;
    assert_eq!(
        first_criterion.0,
        StatusCode::CREATED,
        "{}",
        first_criterion.1
    );
    assert_eq!(first_criterion.2["version"], 5);
    assert_eq!(first_criterion.2["acceptanceCriteria"][0]["position"], 1);
    let criterion_id = first_criterion.2["acceptanceCriteria"][0]["id"]
        .as_str()
        .expect("criterion ID")
        .to_owned();
    let second_criterion = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-criterion-two"),
        &criteria_uri,
        json!({"expectedVersion": 5, "text": "The conflict keeps the local draft"}),
    )
    .await;
    assert_eq!(second_criterion.0, StatusCode::CREATED);
    assert_eq!(second_criterion.2["version"], 6);
    let second_criterion_id = second_criterion.2["acceptanceCriteria"][1]["id"]
        .as_str()
        .expect("second criterion ID")
        .to_owned();
    let completed_criterion = patch_json(
        &application,
        &cookie,
        &csrf,
        "m4-criterion-complete",
        &format!("{criteria_uri}/{criterion_id}"),
        json!({"expectedVersion": 6, "completed": true, "position": 2}),
    )
    .await;
    assert_eq!(
        completed_criterion.0,
        StatusCode::OK,
        "{}",
        completed_criterion.1
    );
    assert_eq!(completed_criterion.2["version"], 7);
    assert_eq!(
        completed_criterion.2["acceptanceCriteria"][1]["id"],
        criterion_id
    );
    assert_eq!(
        completed_criterion.2["acceptanceCriteria"][1]["completed"],
        true
    );
    assert_eq!(
        completed_criterion.2["acceptanceCriteria"][1]["completedBy"]["id"],
        seed.human_actor_id.to_string()
    );
    let removed_criterion = delete_json(
        &application,
        &cookie,
        &csrf,
        "m4-criterion-remove",
        &format!("{criteria_uri}/{second_criterion_id}"),
        json!({"expectedVersion": 7}),
    )
    .await;
    assert_eq!(
        removed_criterion.0,
        StatusCode::OK,
        "{}",
        removed_criterion.1
    );
    assert_eq!(removed_criterion.2["version"], 8);
    assert_eq!(
        removed_criterion.2["acceptanceCriteria"]
            .as_array()
            .expect("criteria")
            .len(),
        1
    );
    assert_eq!(removed_criterion.2["acceptanceCriteria"][0]["position"], 1);

    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE project_id = $1 AND id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("make seeded repository ready for Work Item linking");
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(seed.execution_profile_id)
        .execute(&pool)
        .await
        .expect("authenticate the seeded Execution Profile for Work with Worker");
    let linked_thread = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-create-linked-thread"),
        &format!("/api/v1/channels/{}/threads", seed.channel_id),
        json!({"name": "Work Item implementation", "originMessageId": null}),
    )
    .await;
    assert_eq!(linked_thread.0, StatusCode::CREATED, "{}", linked_thread.1);
    let linked_thread_id = linked_thread.2["conversation"]["id"]
        .as_str()
        .expect("linked Thread ID");
    let thread_link_uri = format!(
        "{base}/{}/threads/{linked_thread_id}",
        created.2["id"].as_str().expect("created ID")
    );
    let thread_link = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-thread-link"),
        &thread_link_uri,
        json!({}),
    )
    .await;
    assert_eq!(thread_link.0, StatusCode::OK, "{}", thread_link.1);
    assert_eq!(thread_link.2["version"], 9);
    assert_eq!(thread_link.2["linkedThreads"][0]["id"], linked_thread_id);
    assert_eq!(
        thread_link.2["linkedThreads"][0]["parentChannelId"],
        seed.channel_id.to_string()
    );
    assert_eq!(
        thread_link.2["linkedThreads"][0]["href"],
        format!("/projects/{}/threads/{linked_thread_id}", seed.project_id)
    );
    let thread_link_replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-thread-link"),
        &thread_link_uri,
        json!({}),
    )
    .await;
    assert_eq!(thread_link_replay.2["version"], 9);
    let channel_link = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-channel-link-rejected"),
        &format!(
            "{base}/{}/threads/{}",
            created.2["id"].as_str().expect("created ID"),
            seed.channel_id
        ),
        json!({}),
    )
    .await;
    assert_eq!(channel_link.0, StatusCode::NOT_FOUND);

    let first_repository_uri = format!(
        "{base}/{}/repositories/{}",
        created.2["id"].as_str().expect("created ID"),
        seed.repository_id
    );
    let primary_repository = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-primary-repository"),
        &first_repository_uri,
        json!({"expectedVersion": 9, "role": "primary"}),
    )
    .await;
    assert_eq!(
        primary_repository.0,
        StatusCode::OK,
        "{}",
        primary_repository.1
    );
    assert_eq!(primary_repository.2["version"], 10);
    assert_eq!(
        primary_repository.2["linkedRepositories"][0]["role"],
        "primary"
    );
    assert_eq!(
        primary_repository.2["linkedRepositories"][0]["cloneUrl"],
        "https://github.com/Mischa-dev/agent-workspace.git"
    );
    assert_eq!(
        primary_repository.2["linkedRepositories"][0]["trusted"],
        true
    );

    let second_repository_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, trusted, preparation_state, prepared_at
        ) VALUES ($1, $2, 987654321, 'Mischa-dev', 'agent-workspace-related',
                  'https://github.com/Mischa-dev/agent-workspace-related.git',
                  'main', $3, true, 'ready', now())
        "#,
    )
    .bind(second_repository_id)
    .bind(seed.project_id)
    .bind(format!(
        "/tmp/agent-workspace-test-{second_repository_id}.git"
    ))
    .execute(&pool)
    .await
    .expect("insert second ready repository");
    let second_repository_uri = format!(
        "{base}/{}/repositories/{second_repository_id}",
        created.2["id"].as_str().expect("created ID")
    );
    let related_repository = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-related-repository"),
        &second_repository_uri,
        json!({"expectedVersion": 10, "role": "related"}),
    )
    .await;
    assert_eq!(
        related_repository.0,
        StatusCode::OK,
        "{}",
        related_repository.1
    );
    assert_eq!(related_repository.2["version"], 11);
    let swapped_primary = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-swap-primary"),
        &format!("{second_repository_uri}/primary"),
        json!({"expectedVersion": 11}),
    )
    .await;
    assert_eq!(swapped_primary.0, StatusCode::OK, "{}", swapped_primary.1);
    assert_eq!(swapped_primary.2["version"], 12);
    assert_eq!(
        swapped_primary.2["linkedRepositories"][0]["id"],
        second_repository_id.to_string()
    );
    assert_eq!(
        swapped_primary.2["linkedRepositories"][0]["role"],
        "primary"
    );
    let unlinked_repository = delete_json(
        &application,
        &cookie,
        &csrf,
        "m4-unlink-repository",
        &second_repository_uri,
        json!({"expectedVersion": 12}),
    )
    .await;
    assert_eq!(
        unlinked_repository.0,
        StatusCode::OK,
        "{}",
        unlinked_repository.1
    );
    assert_eq!(unlinked_repository.2["version"], 13);
    let restored_primary = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-restore-primary"),
        &format!("{first_repository_uri}/primary"),
        json!({"expectedVersion": 13}),
    )
    .await;
    assert_eq!(restored_primary.0, StatusCode::OK, "{}", restored_primary.1);
    assert_eq!(restored_primary.2["version"], 14);
    assert_eq!(
        restored_primary.2["linkedRepositories"][0]["id"],
        seed.repository_id.to_string()
    );
    assert_eq!(
        restored_primary.2["linkedRepositories"][0]["role"],
        "primary"
    );

    let board = get_json(&application, &cookie, &format!("{base}/board?limit=10")).await;
    assert_eq!(board.0, StatusCode::OK, "{}", board.1);
    assert_eq!(board.2["columns"].as_array().map(Vec::len), Some(7));
    let moved = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-board-move"),
        &format!(
            "{base}/{}/move",
            created.2["id"].as_str().expect("created ID")
        ),
        json!({
            "expectedVersion": 14,
            "targetStatusId": in_progress_status_id,
            "targetPosition": 1
        }),
    )
    .await;
    assert_eq!(moved.0, StatusCode::OK, "{}", moved.1);
    assert_eq!(moved.2["version"], 15);
    assert_eq!(moved.2["status"]["name"], "In Progress");
    let moved_board = get_json(
        &application,
        &cookie,
        &format!("{base}/board?statusId={in_progress_status_id}&limit=10"),
    )
    .await;
    assert_eq!(moved_board.0, StatusCode::OK, "{}", moved_board.1);
    assert_eq!(
        moved_board.2["columns"][0]["items"][0]["id"],
        created.2["id"]
    );

    let worker_uri = format!(
        "{base}/{}/work-with-worker",
        created.2["id"].as_str().expect("created ID")
    );
    let worker_request = json!({
        "expectedVersion": 15,
        "channelId": seed.channel_id,
        "prompt": "Implement the smallest verified change."
    });
    let worker = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-work-with-worker"),
        &worker_uri,
        worker_request.clone(),
    )
    .await;
    assert_eq!(worker.0, StatusCode::ACCEPTED, "{}", worker.1);
    assert_eq!(worker.2["run"]["state"], "queued");
    assert_eq!(
        worker.2["run"]["repositoryId"],
        seed.repository_id.to_string()
    );
    let worker_thread_id = worker.2["threadId"].as_str().expect("worker Thread ID");
    let worker_run_id = worker.2["run"]["runId"].as_str().expect("worker run ID");
    let worker_replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-work-with-worker"),
        &worker_uri,
        worker_request,
    )
    .await;
    assert_eq!(worker_replay.0, StatusCode::ACCEPTED);
    assert_eq!(worker_replay.2["threadId"], worker_thread_id);
    assert_eq!(worker_replay.2["run"]["runId"], worker_run_id);
    let worker_prompt = sqlx::query_scalar::<_, String>(
        "SELECT prompt FROM agent_runs WHERE project_id = $1 AND id = $2",
    )
    .bind(seed.project_id)
    .bind(Uuid::parse_str(worker_run_id).expect("worker run UUID"))
    .fetch_one(&pool)
    .await
    .expect("read bounded worker prompt");
    assert!(worker_prompt.contains(created.2["id"].as_str().expect("Work Item ID")));
    assert!(worker_prompt.contains(created.2["key"].as_str().expect("Work Item key")));
    assert!(worker_prompt.ends_with("Implement the smallest verified change."));
    assert!(!worker_prompt.contains("Please review"));
    let after_worker = get_json(
        &application,
        &cookie,
        &format!("{base}/{}", created.2["id"].as_str().expect("created ID")),
    )
    .await;
    assert_eq!(after_worker.2["version"], 16);
    assert!(
        after_worker.2["linkedThreads"]
            .as_array()
            .expect("linked Threads")
            .iter()
            .any(|thread| thread["id"] == worker_thread_id)
    );

    let comment_uri = format!(
        "{base}/{}/comments",
        created.2["id"].as_str().expect("created ID")
    );
    let comment = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-comment-one"),
        &comment_uri,
        json!({
            "content": {
                "version": 1,
                "nodes": [
                    {"type": "text", "text": "Please review "},
                    {"type": "mention", "actorId": seed.human_actor_id},
                    {"type": "text", "text": " with "},
                    {"type": "work_item_ref", "workItemId": second.2["id"]}
                ]
            }
        }),
    )
    .await;
    assert_eq!(comment.0, StatusCode::CREATED, "{}", comment.1);
    assert_eq!(comment.2["sequence"], 17);
    assert_eq!(
        comment.2["comment"]["mentionedActors"][0]["id"],
        seed.human_actor_id.to_string()
    );
    assert_eq!(
        comment.2["comment"]["referencedWorkItems"][0]["id"],
        second.2["id"]
    );
    let comment_replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-comment-one"),
        &comment_uri,
        json!({
            "content": {
                "version": 1,
                "nodes": [
                    {"type": "text", "text": "Please review "},
                    {"type": "mention", "actorId": seed.human_actor_id},
                    {"type": "text", "text": " with "},
                    {"type": "work_item_ref", "workItemId": second.2["id"]}
                ]
            }
        }),
    )
    .await;
    assert_eq!(
        comment_replay.2["comment"]["id"],
        comment.2["comment"]["id"]
    );
    let conversation_message = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("m4-conversation-work-item-ref"),
        &format!("/api/v1/channels/{}/messages", seed.channel_id),
        json!({
            "content": {
                "version": 1,
                "nodes": [
                    {"type": "text", "text": "Track this in "},
                    {"type": "work_item_ref", "work_item_id": second.2["id"]}
                ]
            }
        }),
    )
    .await;
    assert_eq!(
        conversation_message.0,
        StatusCode::CREATED,
        "{}",
        conversation_message.1
    );
    assert_eq!(
        conversation_message.2["message"]["referencedWorkItems"][0]["id"],
        second.2["id"]
    );
    let conversation_timeline = get_json(
        &application,
        &cookie,
        &format!("/api/v1/conversations/{}/timeline", seed.channel_id),
    )
    .await;
    assert_eq!(conversation_timeline.0, StatusCode::OK);
    assert_eq!(
        conversation_timeline.2["entries"]
            .as_array()
            .expect("Conversation timeline")
            .last()
            .expect("Conversation message")["message"]["referencedWorkItems"][0]["id"],
        second.2["id"]
    );
    let timeline = get_json(
        &application,
        &cookie,
        &format!(
            "{base}/{}/timeline?limit=3",
            created.2["id"].as_str().expect("created ID")
        ),
    )
    .await;
    assert_eq!(timeline.0, StatusCode::OK, "{}", timeline.1);
    assert_eq!(timeline.2["entries"][0]["kind"], "comment");
    assert_eq!(timeline.2["entries"][0]["sequence"], 17);
    assert!(timeline.2["nextCursor"].is_string());

    let activity_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_activity WHERE project_id = $1",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("count M4 activity");
    assert_eq!(activity_count, 18);
    let outbox_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events WHERE project_id = $1 AND resource_kind = 'work_item' AND conversation_id IS NULL",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("count M4 outbox events");
    assert_eq!(outbox_count, 19);
    let counter = sqlx::query_scalar::<_, i64>(
        "SELECT next_work_item_number FROM project_counters WHERE project_id = $1",
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("read M4 Work Item counter");
    assert_eq!(counter, 4);
    let referenced_type_id = Uuid::parse_str(
        created.2["itemType"]["id"]
            .as_str()
            .expect("created Work Item type ID"),
    )
    .expect("created Work Item type UUID");
    let referenced_status_id = Uuid::parse_str(
        created.2["status"]["id"]
            .as_str()
            .expect("created Work Item status ID"),
    )
    .expect("created Work Item status UUID");
    let disable_type =
        sqlx::query("UPDATE work_item_types SET enabled = false WHERE project_id = $1 AND id = $2")
            .bind(seed.project_id)
            .bind(referenced_type_id)
            .execute(&pool)
            .await;
    assert!(disable_type.is_err());
    let disable_status = sqlx::query(
        "UPDATE work_item_statuses SET enabled = false WHERE project_id = $1 AND id = $2",
    )
    .bind(seed.project_id)
    .bind(referenced_status_id)
    .execute(&pool)
    .await;
    assert!(disable_status.is_err());
    let key_change = sqlx::query("UPDATE projects SET project_key = 'NEW' WHERE id = $1")
        .bind(seed.project_id)
        .execute(&pool)
        .await;
    assert!(key_change.is_err());

    supervisors.shutdown_all().await;
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated M4 Work Item schema");
}

async fn login(application: &axum::Router) -> String {
    login_as(application, LOGIN_NAME).await
}

async fn login_as(application: &axum::Router, login_name: &str) -> String {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "loginName": login_name,
                        "password": PASSWORD
                    }))
                    .expect("serialize M4 login request"),
                ))
                .expect("build M4 login request"),
        )
        .await
        .expect("M4 login response");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("M4 cookie header")
                .split(';')
                .next()
                .expect("M4 cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn csrf_from_cookie(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|pair| pair.strip_prefix("agent_workspace_csrf="))
        .expect("M4 CSRF cookie")
        .to_owned()
}

async fn get_json(
    application: &axum::Router,
    cookie: &str,
    uri: &str,
) -> (StatusCode, String, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .expect("build M4 GET request"),
        )
        .await
        .expect("M4 GET response");
    response_json(response).await
}

async fn post_json(
    application: &axum::Router,
    cookie: &str,
    csrf: Option<&str>,
    idempotency_key: Option<&str>,
    uri: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(csrf) = csrf {
        request = request
            .header("origin", "http://127.0.0.1:3100")
            .header("x-csrf-token", csrf);
    }
    if let Some(idempotency_key) = idempotency_key {
        request = request.header("idempotency-key", idempotency_key);
    }
    let response = application
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize M4 request"),
                ))
                .expect("build M4 POST request"),
        )
        .await
        .expect("M4 POST response");
    response_json(response).await
}

async fn patch_json(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    idempotency_key: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("origin", "http://127.0.0.1:3100")
                .header("x-csrf-token", csrf)
                .header("idempotency-key", idempotency_key)
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize M4 PATCH request"),
                ))
                .expect("build M4 PATCH request"),
        )
        .await
        .expect("M4 PATCH response");
    response_json(response).await
}

async fn delete_json(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    idempotency_key: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("origin", "http://127.0.0.1:3100")
                .header("x-csrf-token", csrf)
                .header("idempotency-key", idempotency_key)
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize M4 DELETE request"),
                ))
                .expect("build M4 DELETE request"),
        )
        .await
        .expect("M4 DELETE response");
    response_json(response).await
}

async fn put_json(
    application: &axum::Router,
    cookie: &str,
    csrf: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("origin", "http://127.0.0.1:3100")
                .header("x-csrf-token", csrf)
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize M4 PUT request"),
                ))
                .expect("build M4 PUT request"),
        )
        .await
        .expect("M4 PUT response");
    response_json(response).await
}

async fn response_json(response: axum::response::Response) -> (StatusCode, String, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read M4 response");
    let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 M4 response");
    let value = serde_json::from_str(&text).expect("JSON M4 response");
    (status, text, value)
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
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
        .connect(database_url)
        .await
        .expect("connect schema-scoped M4 Work Item pool")
}

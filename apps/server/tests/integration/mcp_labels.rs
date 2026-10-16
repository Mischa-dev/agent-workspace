//! Labels (docs/plans/mcp-tools.md, Phase 2). The costly bugs are silent: a
//! label from another project resolving, a viewer or read-only app changing
//! labels, an archived label still showing on tasks (or lost when it comes
//! back), a label change that bumps a task's version twice or not at all,
//! and a moved task dropping its labels.

use agent_workspace_server::{
    policy::ActorKind,
    work_item_commands::{
        AuditAttribution, AuditVersionMetadata, CommandActor, UpdateWorkItemCommand,
        UpdateWorkItemOutcome, lock_membership, update_work_item_by_id,
    },
    work_item_labels::{self, LabelDelta},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
};

fn id(value: &str) -> Uuid {
    value.parse().expect("UUID")
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {value}"))
        .iter()
        .map(|entry| match entry {
            Value::String(name) => name.clone(),
            entry => entry["name"].as_str().expect("name").to_owned(),
        })
        .collect()
}

fn message(result: &Value) -> &str {
    result["message"]
        .as_str()
        .unwrap_or_else(|| panic!("no message: {result}"))
}

async fn owner(setup: &Setup) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' LIMIT 1",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner")
}

async fn version(setup: &Setup, task: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("version")
}

/// Puts labels on a task and takes them off through the shared update
/// command, the one path task_update and the batch tools label tasks by.
async fn relabel(
    setup: &Setup,
    project: Uuid,
    task: Uuid,
    add: &[&str],
    remove: &[&str],
) -> UpdateWorkItemOutcome {
    let actor_id = owner(setup).await;
    let expected_version = version(setup, task).await;
    let mut transaction = setup.pool.begin().await.expect("transaction");
    lock_membership(&mut transaction, actor_id, project)
        .await
        .expect("lock")
        .expect("membership");
    let owned = |list: &[&str]| {
        list.iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    let add = work_item_labels::resolve(&mut transaction, project, &owned(add))
        .await
        .expect("resolve added");
    let remove = work_item_labels::resolve(&mut transaction, project, &owned(remove))
        .await
        .expect("resolve removed");
    assert!(
        add.is_complete() && remove.is_complete(),
        "{add:?} {remove:?}"
    );
    let (add, remove) = (add.ids(), remove.ids());
    let actor = CommandActor {
        actor_id,
        actor_kind: ActorKind::Human,
        project_id: project,
        audit: AuditAttribution::Mcp {
            authentication_class: "oauth_access_token",
            request_id: Uuid::now_v7().to_string(),
            client_name: "Codex".to_owned(),
            mcp_credential_id: None,
            oauth_grant_id: None,
            external_agent_connection_id: None,
        },
    };
    let outcome = update_work_item_by_id(
        &mut transaction,
        &actor,
        task,
        UpdateWorkItemCommand {
            expected_version,
            title: None,
            description: None,
            type_id: None,
            status_id: None,
            priority: None,
            assignee_actor_id: None,
            parent_work_item_id: None,
            board_placement: None,
            labels: Some(LabelDelta {
                add: &add,
                remove: &remove,
            }),
            fields: Default::default(),
            summary: "Labels changed",
            audit_action: "work_item.mcp_updated",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await
    .expect("update");
    transaction.commit().await.expect("commit");
    outcome
}

/// The plain-words error for label names a project can't use right now.
async fn label_problem(setup: &Setup, project: Uuid, references: &[&str]) -> String {
    let mut connection = setup.pool.acquire().await.expect("connection");
    let references = references
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let resolution = work_item_labels::resolve(&mut connection, project, &references)
        .await
        .expect("resolve");
    assert!(!resolution.is_complete(), "{resolution:?}");
    work_item_labels::unknown_message(&mut connection, project, &resolution)
        .await
        .expect("message")
}

#[tokio::test]
async fn labels_belong_to_one_project_and_only_members_change_them() {
    let Some(setup) = setup("mcp_labels_access").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    create_project(&setup, "SECRET", "Secret").await;
    let space = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (vic, vic_token) = create_agent(app, &setup.browser, &space, "Vic", "vic").await;
    for (agent, role) in [(mara, "member"), (vic, "viewer")] {
        let (status, added) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{work}/members"),
                None,
                json!({"actorId": agent, "role": role}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{added}");
    }
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let vic_session = initialize(app, &vic_token).await.expect("Vic MCP");

    let bug = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "name": "  Bug "}),
    )
    .await;
    assert_eq!(bug["outcome"], "ok", "{bug}");
    assert_eq!(bug["value"]["created"], true);
    assert_eq!(bug["value"]["changedFields"], json!(["label"]));
    assert_eq!(bug["value"]["label"]["name"], "Bug");
    // Picked from the name, the same in every release.
    assert_eq!(bug["value"]["label"]["color"], "#ffc53d");
    assert_eq!(bug["value"]["label"]["version"], 1);
    let again = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "name": "bug", "color": "red"}),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["created"], false);
    assert_eq!(again["value"]["label"]["id"], bug["value"]["label"]["id"]);
    assert_eq!(again["value"]["label"]["color"], "#ffc53d", "{again}");
    assert!(
        again["value"]["note"]
            .as_str()
            .expect("note")
            .contains("already exists"),
        "{again}"
    );

    let secret = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "SECRET", "name": "Launch codename"}),
    )
    .await;
    assert_eq!(secret["outcome"], "ok", "{secret}");
    let secret_label = secret["value"]["label"]["id"]
        .as_str()
        .expect("label")
        .to_owned();

    // Agents in the project create labels too.
    let mobile = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "name": "Mobile", "color": "blue", "description": "Phone apps"}),
    )
    .await;
    assert_eq!(mobile["outcome"], "ok", "{mobile}");
    assert_eq!(mobile["value"]["label"]["color"], "#0090ff");
    let design = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": work, "name": "Design", "color": "#ABC"}),
    )
    .await;
    assert_eq!(design["value"]["label"]["color"], "#aabbcc", "{design}");
    let bad_color = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "name": "Odd", "color": "chartreuse"}),
    )
    .await;
    assert_eq!(bad_color["outcome"], "invalid_request", "{bad_color}");
    assert_eq!(bad_color["field"], "color");
    // A description stays on one line. The server and the database's CHECK
    // refuse the same characters, line separators included, whatever the
    // database's locale makes of [[:cntrl:]].
    let two_lines = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "description": "Phone\u{2028}apps"}),
    )
    .await;
    assert_eq!(two_lines["outcome"], "invalid_request", "{two_lines}");
    assert_eq!(two_lines["field"], "description");
    let refused = sqlx::query("UPDATE project_labels SET description = $2 WHERE id = $1")
        .bind(id(mobile["value"]["label"]["id"].as_str().expect("id")))
        .bind("Phone\u{2028}apps")
        .execute(&setup.pool)
        .await
        .expect_err("the CHECK refuses a line separator");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
    let id_shaped = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "name": Uuid::now_v7().to_string()}),
    )
    .await;
    assert_eq!(id_shaped["field"], "name", "{id_shaped}");
    let nothing = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "label": "Bug"}),
    )
    .await;
    assert_eq!(nothing["outcome"], "invalid_request", "{nothing}");
    assert_eq!(nothing["field"], "label");

    // Viewers and read-only apps see labels but can't change them.
    let refused = tool(
        app,
        &vic_token,
        &vic_session,
        "label_write",
        json!({"projectId": "WORK", "name": "Viewer label"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let seen = tool(
        app,
        &vic_token,
        &vic_session,
        "label_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    assert_eq!(names(&seen["value"]["labels"]), ["Bug", "Design", "Mobile"]);
    assert_eq!(seen["value"]["labels"][2]["description"], "Phone apps");
    assert!(
        seen["value"]["labels"][0].get("description").is_none(),
        "an empty description is left out: {seen}"
    );
    let read_only = tool(
        app,
        &reader,
        &reader_session,
        "label_write",
        json!({"projectId": "WORK", "name": "Read only"}),
    )
    .await;
    assert_eq!(read_only["outcome"], "permission_denied", "{read_only}");

    // Another project's labels stay out of reach, by project and by label ID.
    let secret_project =
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM project_labels WHERE id = $1")
            .bind(id(&secret_label))
            .fetch_one(&setup.pool)
            .await
            .expect("secret project");
    let forbidden = tool(
        app,
        &mara_token,
        &mara_session,
        "label_list",
        json!({"projectId": secret_project}),
    )
    .await;
    assert_eq!(forbidden["outcome"], "forbidden", "{forbidden}");
    let leaked = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "label": secret_label, "name": "Leaked"}),
    )
    .await;
    assert_eq!(leaked["outcome"], "not_found", "{leaked}");
    assert!(!leaked.to_string().contains("Launch"), "{leaked}");
    let untouched = tool(
        app,
        &codex,
        &session,
        "label_list",
        json!({"projectId": "SECRET"}),
    )
    .await;
    assert_eq!(names(&untouched["value"]["labels"]), ["Launch codename"]);
    assert_eq!(untouched["value"]["labels"][0]["version"], 1);

    // Names stay unique among a project's active labels, ignoring case, and
    // a stale version is refused.
    let clash = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "name": "BUG"}),
    )
    .await;
    assert_eq!(clash["outcome"], "conflict", "{clash}");
    assert!(message(&clash).contains("already a label called \"Bug\""));
    let stale = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "name": "Phone", "expectedVersion": 5}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert!(message(&stale).contains("now version 1"), "{stale}");
    // Only the case changes: allowed, and the version moves on.
    let recased = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "label": "mobile", "name": "MOBILE", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(recased["outcome"], "ok", "{recased}");
    assert_eq!(recased["value"]["changedFields"], json!(["name"]));
    assert_eq!(recased["value"]["label"]["version"], 2);
    // Writing what's already there changes nothing and checks no version.
    let same = tool(
        app,
        &mara_token,
        &mara_session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "color": "#0090FF", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(same["outcome"], "ok", "{same}");
    assert_eq!(same["value"]["changedFields"], json!([]));
    assert_eq!(same["value"]["label"]["version"], 2);

    // The Messages space has no tasks, so no labels.
    let direct = tool(
        app,
        &codex,
        &session,
        "label_list",
        json!({"projectId": space}),
    )
    .await;
    assert_ne!(direct["outcome"], "ok", "{direct}");

    // Each change is audited once, as the app or agent that made it.
    let audited = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT action, count(*) FROM audit_events
        WHERE target_kind = 'work_item_label' AND channel = 'mcp'
        GROUP BY action ORDER BY action
        "#,
    )
    .fetch_all(&setup.pool)
    .await
    .expect("audit");
    assert_eq!(
        audited,
        [
            ("work_item_label.mcp_created".to_owned(), 4),
            ("work_item_label.mcp_updated".to_owned(), 1),
        ]
    );
}

#[tokio::test]
async fn labels_on_tasks_show_everywhere_and_archiving_hides_them_until_restored() {
    let Some(setup) = setup("mcp_labels_tasks").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = id(&work);
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mut label_ids = std::collections::HashMap::new();
    for name in ["Bug", "Mobile"] {
        let created = tool(
            app,
            &codex,
            &session,
            "label_write",
            json!({"projectId": "WORK", "name": name}),
        )
        .await;
        assert_eq!(created["outcome"], "ok", "{created}");
        label_ids.insert(
            name,
            created["value"]["label"]["id"]
                .as_str()
                .expect("label")
                .to_owned(),
        );
    }
    let first = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "Crash on launch"}),
    )
    .await;
    let task = id(first["workItemId"].as_str().expect("task"));
    let second = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"parentId": "WORK-1", "title": "Reproduce it"}),
    )
    .await;
    let subtask = id(second["workItemId"].as_str().expect("subtask"));

    // Two labels in one change: one version bump, one activity that names
    // the labels by ID.
    let outcome = relabel(&setup, project, task, &["bug", "MOBILE"], &[]).await;
    let UpdateWorkItemOutcome::Applied {
        version: new_version,
        changed_fields,
        ..
    } = outcome
    else {
        panic!("labels weren't applied: {outcome:?}");
    };
    assert_eq!(
        (new_version, changed_fields),
        (2, vec!["labels".to_owned()])
    );
    let payload = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT payload FROM work_item_activity
        WHERE work_item_id = $1 AND kind = 'work_item_updated'
        ORDER BY created_at DESC LIMIT 1
        "#,
    )
    .bind(task)
    .fetch_one(&setup.pool)
    .await
    .expect("activity");
    assert_eq!(payload["changedFields"], json!(["labels"]), "{payload}");
    assert_eq!(
        payload["labelsAdded"],
        json!([label_ids["Bug"], label_ids["Mobile"]]),
        "{payload}"
    );
    // Adding labels a task has already is no change at all.
    let outcome = relabel(&setup, project, task, &["Bug"], &[]).await;
    assert!(
        matches!(outcome, UpdateWorkItemOutcome::Noop(_)),
        "{outcome:?}"
    );
    assert_eq!(version(&setup, task).await, 2);
    assert!(matches!(
        relabel(&setup, project, subtask, &["Bug"], &[]).await,
        UpdateWorkItemOutcome::Applied { .. }
    ));

    // Every read shows them: task_open, its subtasks, task_list lines,
    // project_open, and the web app's detail, list and board.
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_eq!(names(&opened["value"]["task"]["labels"]), ["Bug", "Mobile"]);
    assert_eq!(opened["value"]["task"]["version"], 2);
    assert_eq!(names(&opened["value"]["subtasks"][0]["labels"]), ["Bug"]);
    let listed = tool(
        app,
        &codex,
        &session,
        "task_list",
        json!({"projectId": "WORK", "sort": "number"}),
    )
    .await;
    assert_eq!(
        names(&listed["value"]["tasks"][0]["labels"]),
        ["Bug", "Mobile"]
    );
    let overview = tool(
        app,
        &codex,
        &session,
        "project_open",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_eq!(names(&overview["value"]["labels"]), ["Bug", "Mobile"]);
    let web = |path: String| {
        let request = setup.browser.request("GET", &path, None, Value::Null);
        async move { send(app, request).await }
    };
    let (status, detail) = web(format!("/api/v1/projects/{work}/work-items/{task}")).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(names(&detail["labels"]), ["Bug", "Mobile"]);
    assert_eq!(detail["labels"][0]["id"], label_ids["Bug"].as_str());
    assert!(
        detail["labels"][0]["color"]
            .as_str()
            .is_some_and(|color| color.len() == 7 && color.starts_with('#')),
        "{detail}"
    );
    let (_, list) = web(format!("/api/v1/projects/{work}/work-items")).await;
    let card = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["key"] == "WORK-1")
        .expect("card");
    assert_eq!(names(&card["labels"]), ["Bug", "Mobile"]);
    let (_, board) = web(format!("/api/v1/projects/{work}/work-items/board")).await;
    let on_board = board["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .flat_map(|column| column["items"].as_array().expect("items").iter())
        .find(|item| item["key"] == "WORK-1")
        .expect("board card");
    assert_eq!(names(&on_board["labels"]), ["Bug", "Mobile"]);

    // A name the project doesn't have is refused with its labels listed.
    let unknown = label_problem(&setup, project, &["bugz"]).await;
    assert_eq!(
        unknown,
        "No label called \"bugz\" in WORK. Its labels: Bug, Mobile. Create one with label_write first."
    );

    // Archiving hides a label from every task without changing them, and
    // restoring brings it back.
    let archived = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_eq!(archived["value"]["label"]["archived"], true);
    assert!(
        archived["value"]["note"]
            .as_str()
            .expect("note")
            .contains("Hidden from 1 task"),
        "{archived}"
    );
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_eq!(names(&opened["value"]["task"]["labels"]), ["Bug"]);
    assert_eq!(opened["value"]["task"]["version"], 2);
    let (_, detail) = web(format!("/api/v1/projects/{work}/work-items/{task}")).await;
    assert_eq!(names(&detail["labels"]), ["Bug"]);
    let overview = tool(
        app,
        &codex,
        &session,
        "project_open",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_eq!(names(&overview["value"]["labels"]), ["Bug"]);
    let active = tool(
        app,
        &codex,
        &session,
        "label_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_eq!(names(&active["value"]["labels"]), ["Bug"]);
    let all = tool(
        app,
        &codex,
        &session,
        "label_list",
        json!({"projectId": "WORK", "includeArchived": true}),
    )
    .await;
    assert_eq!(names(&all["value"]["labels"]), ["Bug", "Mobile"]);
    assert_eq!(all["value"]["labels"][1]["archived"], true);
    assert_eq!(all["value"]["labels"][1]["taskCount"], 1);
    assert!(
        label_problem(&setup, project, &["mobile"]).await.contains(
            "\"Mobile\" is archived in WORK. Restore it with label_write (archived: false)"
        )
    );
    let reused = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "name": "mobile"}),
    )
    .await;
    assert_eq!(reused["outcome"], "conflict", "{reused}");
    assert!(
        message(&reused).contains("Restore it with label_write"),
        "{reused}"
    );
    // Nothing puts an archived label on a task, whatever the code path.
    let refused = sqlx::query(
        "INSERT INTO work_item_labels (project_id, work_item_id, label_id, added_by_actor_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(project)
    .bind(subtask)
    .bind(id(&label_ids["Mobile"]))
    .bind(owner(&setup).await)
    .execute(&setup.pool)
    .await
    .expect_err("an archived label can't be added");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
    let restored = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "archived": false}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(restored["value"]["note"], "Back on 1 task.");
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_eq!(names(&opened["value"]["task"]["labels"]), ["Bug", "Mobile"]);
    assert_eq!(opened["value"]["task"]["version"], 2);

    // A restore whose name was taken meanwhile needs a new name.
    let archived = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let renamed = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "label": "Bug", "name": "mobile"}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    let clash = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "label": label_ids["Mobile"], "archived": false}),
    )
    .await;
    assert_eq!(clash["outcome"], "conflict", "{clash}");
    assert!(
        message(&clash).contains("Another active label is called \"mobile\""),
        "{clash}"
    );
    // By name, a restore finds the archived label, never the active one
    // that took its name (which would answer ok and change nothing).
    let clash = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "archived": false}),
    )
    .await;
    assert_eq!(clash["outcome"], "conflict", "{clash}");
    assert!(
        message(&clash).contains("Another active label is called \"mobile\""),
        "{clash}"
    );
    let restored = tool(
        app,
        &codex,
        &session,
        "label_write",
        json!({"projectId": "WORK", "label": "Mobile", "archived": false, "name": "Mobile app"}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(
        restored["value"]["changedFields"],
        json!(["name", "archived"])
    );
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_eq!(
        names(&opened["value"]["task"]["labels"]),
        ["mobile", "Mobile app"]
    );

    // A task carries at most 20 labels; adding past that changes nothing,
    // and taking labels off always works.
    let many = (1..=20)
        .map(|number| format!("L{number}"))
        .collect::<Vec<_>>();
    for name in &many {
        let created = tool(
            app,
            &codex,
            &session,
            "label_write",
            json!({"projectId": "WORK", "name": name}),
        )
        .await;
        assert_eq!(created["outcome"], "ok", "{created}");
    }
    let many = many.iter().map(String::as_str).collect::<Vec<_>>();
    let before = version(&setup, subtask).await;
    let outcome = relabel(&setup, project, subtask, &many, &[]).await;
    assert!(
        matches!(
            outcome,
            UpdateWorkItemOutcome::TooManyLabels {
                limit: 20,
                would_have: 21
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(version(&setup, subtask).await, before);
    let outcome = relabel(&setup, project, subtask, &many, &["mobile"]).await;
    assert!(
        matches!(outcome, UpdateWorkItemOutcome::Applied { .. }),
        "{outcome:?}"
    );
    assert_eq!(version(&setup, subtask).await, before + 1);
    let labelled = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "WORK-2"}),
    )
    .await;
    assert_eq!(opened_labels(&labelled).len(), 20, "{labelled}");
    let label_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_labels WHERE work_item_id = $1",
    )
    .bind(subtask)
    .fetch_one(&setup.pool)
    .await
    .expect("label rows");
    assert_eq!(label_count, 20);
}

fn opened_labels(opened: &Value) -> Vec<String> {
    names(&opened["value"]["task"]["labels"])
}

#[tokio::test]
async fn moving_a_task_carries_its_labels() {
    let Some(setup) = setup("mcp_labels_move").await else {
        return;
    };
    let app = &setup.app;
    let work = id(&create_project(&setup, "WORK", "Work").await);
    let other = id(&create_project(&setup, "OTHER", "Other").await);
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let write = |arguments: Value| {
        let (codex, session) = (codex.clone(), session.clone());
        async move {
            let result = tool(app, &codex, &session, "label_write", arguments).await;
            assert_eq!(result["outcome"], "ok", "{result}");
            result
        }
    };
    write(json!({"projectId": "WORK", "name": "Bug", "color": "red"})).await;
    let mobile = write(json!({"projectId": "WORK", "name": "Mobile", "color": "teal"})).await;
    write(json!({"projectId": "WORK", "name": "Design"})).await;
    write(json!({"projectId": "OTHER", "name": "bug", "color": "blue"})).await;
    write(json!({"projectId": "OTHER", "name": "Design"})).await;
    write(json!({"projectId": "OTHER", "label": "Design", "archived": true})).await;
    for title in ["Offline mode", "Mockups"] {
        let created = tool(
            app,
            &codex,
            &session,
            "task_create",
            json!({"projectId": "WORK", "title": title}),
        )
        .await;
        assert_eq!(created["outcome"], "applied", "{created}");
    }
    let task_id = |number: i64| {
        let pool = setup.pool.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM work_items WHERE project_id = $1 AND number = $2",
            )
            .bind(work)
            .bind(number)
            .fetch_one(&pool)
            .await
            .expect("task")
        }
    };
    let offline = task_id(1).await;
    let mockups = task_id(2).await;
    relabel(&setup, work, offline, &["Bug", "Mobile"], &[]).await;
    relabel(&setup, work, mockups, &["Design", "Bug"], &[]).await;

    // An existing label is matched by name and keeps the target's color; a
    // missing one is created there with the source's color.
    let moved = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "WORK-1", "toProjectId": "OTHER"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["moved"][0]["to"], "OTHER-1");
    assert!(moved["value"].get("notes").is_none(), "{moved}");
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "OTHER-1"}),
    )
    .await;
    assert_eq!(opened_labels(&opened), ["bug", "Mobile"]);
    let labels = tool(
        app,
        &codex,
        &session,
        "label_list",
        json!({"projectId": "OTHER"}),
    )
    .await;
    let lines = labels["value"]["labels"].as_array().expect("labels");
    assert_eq!(names(&labels["value"]["labels"]), ["bug", "Mobile"]);
    assert_eq!(lines[0]["color"], "#0090ff", "{labels}");
    assert_eq!(lines[1]["color"], mobile["value"]["label"]["color"]);
    assert_eq!(lines[1]["taskCount"], 1);
    let created_audit = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM audit_events
        WHERE project_id = $1 AND action = 'work_item_label.mcp_created'
          AND metadata->>'movedWith' = 'WORK-1'
        "#,
    )
    .bind(other)
    .fetch_one(&setup.pool)
    .await
    .expect("audit");
    assert_eq!(created_audit, 1);

    // A label archived in the target isn't brought back by a move; the
    // result says so.
    let moved = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "WORK-2", "toProjectId": "OTHER"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let notes = moved["value"]["notes"].to_string();
    assert!(
        notes.contains("\\\"Design\\\" is archived in OTHER, so it wasn't put on OTHER-2"),
        "{moved}"
    );
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "OTHER-2"}),
    )
    .await;
    assert_eq!(opened_labels(&opened), ["bug"]);
    // The originals keep their labels with the rest of their history.
    let kept =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_labels WHERE project_id = $1")
            .bind(work)
            .fetch_one(&setup.pool)
            .await
            .expect("original labels");
    assert_eq!(kept, 4);
}

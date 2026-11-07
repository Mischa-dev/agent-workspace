//! AWD-102: moving a task (with its subtasks) to another project with a
//! private preview and one confirmation, through MCP and the web app. The
//! silent, costly failures: a value the target can't see being named in its
//! history, audit or stored results; a stale preview or lost access still
//! moving something; a retry moving twice, or a changed request being taken
//! as the same one; and the original losing its values.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::add_person,
    workspace_isolation::{call_tool, structured},
};

/// Values that must never reach the target project: a person who isn't
/// there, a page and a text field it has no place for.
const SECRET_TEXT: &str = "omitted-sentinel-text";
const SECRET_PERSON: &str = "patsentinel";
const SECRET_PAGE: &str = "Sentinel page";

/// Every row of the target project that a move writes (history, comments,
/// audit, outbox, notifications) and every receipt, as text.
async fn target_rows(setup: &Setup, project: &str) -> String {
    let mut text = String::new();
    for query in [
        "SELECT row_to_json(activity)::text FROM work_item_activity activity WHERE project_id = $1::uuid",
        "SELECT row_to_json(comment)::text FROM work_item_comments comment WHERE project_id = $1::uuid",
        "SELECT row_to_json(audit)::text FROM audit_events audit WHERE project_id = $1::uuid",
        "SELECT row_to_json(event)::text FROM outbox_events event WHERE event.payload::text LIKE '%' || $1 || '%'",
        "SELECT row_to_json(note)::text FROM notifications note WHERE note::text LIKE '%' || $1 || '%'",
        "SELECT row_to_json(receipt)::text FROM work_item_move_receipts receipt",
        "SELECT row_to_json(item)::text FROM work_items item WHERE project_id = $1::uuid",
    ] {
        for row in sqlx::query_scalar::<_, String>(query)
            .bind(project)
            .fetch_all(&setup.pool)
            .await
            .unwrap_or_else(|error| panic!("{query}: {error}"))
        {
            text.push_str(&row);
            text.push('\n');
        }
    }
    text
}

/// Counts of everything a move adds, so a refused or replayed one can be
/// shown to add nothing.
async fn footprint(setup: &Setup) -> Vec<i64> {
    let mut counts = Vec::new();
    for table in [
        "work_items",
        "work_item_comments",
        "work_item_activity",
        "work_item_key_aliases",
        "work_item_move_receipts",
        "audit_events",
        "outbox_events",
        "notifications",
        "work_item_claims",
    ] {
        counts.push(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&setup.pool)
                .await
                .expect(table),
        );
    }
    counts.push(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM work_items WHERE archived_at IS NOT NULL",
        )
        .fetch_one(&setup.pool)
        .await
        .expect("archived"),
    );
    counts
}

async fn field_values(setup: &Setup, task: &str) -> Value {
    sqlx::query_scalar::<_, Value>("SELECT field_values FROM work_items WHERE id = $1::uuid")
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("values")
}

async fn field_id(setup: &Setup, project: &str, name: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT id::text FROM field_definitions WHERE project_id = $1::uuid AND name = $2",
    )
    .bind(project)
    .bind(name)
    .fetch_one(&setup.pool)
    .await
    .unwrap_or_else(|_| panic!("field {name}"))
}

async fn option_id(setup: &Setup, project: &str, field: &str, option: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT option ->> 'id' FROM field_definitions, jsonb_array_elements(config -> 'options') option \
         WHERE project_id = $1::uuid AND name = $2 AND option ->> 'name' = $3",
    )
    .bind(project)
    .bind(field)
    .bind(option)
    .fetch_one(&setup.pool)
    .await
    .unwrap_or_else(|_| panic!("option {field}/{option}"))
}

async fn task_id(setup: &Setup, project: &str, number: i64) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT id::text FROM work_items WHERE project_id = $1::uuid AND number = $2",
    )
    .bind(project)
    .bind(number)
    .fetch_one(&setup.pool)
    .await
    .expect("task")
}

async fn version_of(setup: &Setup, key: &str) -> i64 {
    let (project, number) = key.rsplit_once('-').expect("key");
    sqlx::query_scalar::<_, i64>(
        "SELECT item.version FROM work_items item JOIN projects project ON project.id = item.project_id \
         WHERE project.project_key = $1 AND item.number = $2",
    )
    .bind(project)
    .bind(number.parse::<i64>().expect("number"))
    .fetch_one(&setup.pool)
    .await
    .expect("version")
}

fn issue<'a>(preview: &'a Value, task_key: &str, field: &str) -> &'a Value {
    preview["issues"]
        .as_array()
        .and_then(|issues| {
            issues
                .iter()
                .find(|issue| issue["taskKey"] == task_key && issue["field"] == field)
        })
        .unwrap_or_else(|| panic!("no issue {task_key}/{field}: {preview}"))
}

/// Two projects with fields that partly match, and a task tree in SRC whose
/// values cover every case: carried as is, an option the target lacks, a
/// person who isn't there, a page, a link inside the moving tree and one
/// outside it, and a field the target doesn't have.
struct World {
    src: String,
    dst: String,
    dee: Uuid,
    root: String,
    sub: String,
}

async fn world(setup: &Setup, codex: &str, session: &str) -> World {
    let app = &setup.app;
    let src = create_project(setup, "SRC", "Source").await;
    let dst = create_project(setup, "DST", "Destination").await;
    let (pat, _) = add_person(setup, &[&src], SECRET_PERSON).await;
    let (dee, _) = add_person(setup, &[&src, &dst], "dee").await;
    let _ = pat;
    for (project, name, extra) in [
        (
            "SRC",
            "Risk",
            json!({"type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]}),
        ),
        ("SRC", "Notes", json!({"type": "text"})),
        ("SRC", "Owner", json!({"type": "person"})),
        ("SRC", "Spec", json!({"type": "link", "target": "page"})),
        ("SRC", "Follows", json!({"type": "link", "target": "task"})),
        ("SRC", "Secret", json!({"type": "text"})),
        (
            "DST",
            "Risk",
            json!({"type": "single_choice", "options": [{"name": "Low"}, {"name": "Medium"}]}),
        ),
        ("DST", "Notes", json!({"type": "text"})),
        ("DST", "Owner", json!({"type": "person"})),
        ("DST", "Spec", json!({"type": "link", "target": "page"})),
        ("DST", "Follows", json!({"type": "link", "target": "task"})),
        ("DST", "Summary", json!({"type": "text"})),
    ] {
        let mut arguments = json!({"projectId": project, "name": name});
        for (key, value) in extra.as_object().expect("extra") {
            arguments[key] = value.clone();
        }
        let written = tool(app, codex, session, "field_write", arguments).await;
        assert_eq!(written["outcome"], "ok", "{written}");
    }
    let page = tool(
        app,
        codex,
        session,
        "page_write",
        json!({"projectId": "SRC", "title": SECRET_PAGE, "body": "Private plan"}),
    )
    .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    let page_id = page["value"]["id"].as_str().expect("page").to_owned();
    // SRC-1 the root, SRC-2 its subtask, SRC-3 a task that stays.
    let made = tool(
        app,
        codex,
        session,
        "task_create_many",
        json!({"projectId": "SRC", "clientRequestId": "move-world", "tasks": [
            {"ref": "root", "title": "Root"},
            {"ref": "sub", "title": "Sub", "parentRef": "root"},
            {"title": "Stays"},
        ]}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let root = task_id(setup, &src, 1).await;
    let sub = task_id(setup, &src, 2).await;
    for (task, fields) in [
        (
            "SRC-1",
            json!({"Risk": "High", "Notes": "plain note", "Owner": format!("@{SECRET_PERSON}"),
                   "Spec": page_id, "Follows": "SRC-2", "Secret": SECRET_TEXT}),
        ),
        ("SRC-2", json!({"Owner": "@dee", "Follows": "SRC-3"})),
    ] {
        let updated = tool(
            app,
            codex,
            session,
            "task_update",
            json!({"taskId": task, "fields": fields, "expectedVersion": version_of(setup, task).await}),
        )
        .await;
        assert_eq!(updated["outcome"], "applied", "{updated}");
    }
    World {
        src,
        dst,
        dee,
        root,
        sub,
    }
}

#[tokio::test]
async fn a_move_previews_privately_then_moves_once_with_the_choices_made() {
    let Some(setup) = setup("awd102_move_flow").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    let before = footprint(&setup).await;
    let original = field_values(&setup, &w.root).await;

    // 1. The preview is private and changes nothing.
    let previewed = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "preview": true}),
    )
    .await;
    assert_eq!(previewed["outcome"], "ok", "{previewed}");
    let preview = previewed["value"]["preview"].clone();
    assert_eq!(footprint(&setup).await, before, "a preview writes nothing");
    let keys = preview["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|task| task["key"].clone())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec![json!("SRC-1"), json!("SRC-2")], "{preview}");
    let carried = preview["carried"].as_array().expect("carried");
    assert!(
        carried
            .iter()
            .any(|value| value["taskKey"] == "SRC-1" && value["field"] == "Notes"),
        "{preview}"
    );
    assert!(
        carried.iter().any(|value| value["taskKey"] == "SRC-1"
            && value["field"] == "Follows"
            && value["followsMove"] == true),
        "a link inside the moving tree follows it: {preview}"
    );
    assert!(
        carried
            .iter()
            .any(|value| value["taskKey"] == "SRC-2" && value["field"] == "Owner"),
        "{preview}"
    );
    for (task, field, reason) in [
        ("SRC-1", "Risk", "option_missing"),
        ("SRC-1", "Owner", "person_not_in_target"),
        ("SRC-1", "Spec", "page_link"),
        ("SRC-1", "Secret", "no_matching_field"),
        ("SRC-2", "Follows", "link_outside_move"),
    ] {
        assert_eq!(issue(&preview, task, field)["reason"], reason, "{preview}");
    }
    // The mover reads what stays, through the task page's own projection.
    assert!(
        issue(&preview, "SRC-1", "Owner")["value"]
            .to_string()
            .contains(SECRET_PERSON)
    );
    assert_eq!(issue(&preview, "SRC-1", "Spec")["value"], SECRET_PAGE);
    let secret = issue(&preview, "SRC-1", "Secret");
    assert!(
        secret["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .any(|candidate| candidate["name"] == "Summary" && candidate["takesValue"] == true),
        "a text value fits another text field: {secret}"
    );
    // The web app reads the same plan.
    let (status, web) = send(
        app,
        setup.browser.request(
            "GET",
            &format!(
                "/api/v1/projects/{}/work-items/{}/project-move?targetProjectId={}",
                w.src, w.root, w.dst
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{web}");
    assert_eq!(web["snapshot"], preview["snapshot"], "one plan for both");

    // 2. Every issue needs a choice; a choice is checked in the target.
    let snapshot = preview["snapshot"].clone();
    let id_of = |task: &str, field: &str| issue(&preview, task, field)["id"].clone();
    let refused = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "snapshot": snapshot,
               "resolutions": [{"issue": id_of("SRC-1", "Risk"), "action": "leave"}],
               "clientRequestId": "move-incomplete"}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let refused = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "snapshot": snapshot,
               "resolutions": [{"issue": id_of("SRC-1", "Owner"), "action": "set",
                                "targetField": "Owner", "value": format!("@{SECRET_PERSON}")}],
               "leaveUnresolved": true, "clientRequestId": "move-bad-person"}),
    )
    .await;
    assert_eq!(
        refused["outcome"], "invalid_request",
        "someone not in DST: {refused}"
    );
    assert_eq!(footprint(&setup).await, before, "refusals write nothing");

    // 3. The move: Risk set to another option, the owner replaced, the
    // page and the secret left, the outside link left.
    let confirm = json!({
        "taskId": "SRC-1", "toProjectId": "DST", "snapshot": snapshot,
        "resolutions": [
            {"issue": id_of("SRC-1", "Risk"), "action": "set", "targetField": "Risk", "value": "Medium"},
            {"issue": id_of("SRC-1", "Owner"), "action": "set", "targetField": "Owner", "value": w.dee.to_string()},
            {"issue": id_of("SRC-1", "Spec"), "action": "leave"},
            {"issue": id_of("SRC-1", "Secret"), "action": "leave"},
            {"issue": id_of("SRC-2", "Follows"), "action": "leave"},
        ],
        "clientRequestId": "move-src-1",
    });
    let moved = tool(app, &codex, &session, "task_move", confirm.clone()).await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["created"], true);
    assert_eq!(moved["value"]["leftBehind"], 3, "{moved}");
    let new_root = moved["value"]["moved"][0]["id"]
        .as_str()
        .expect("root")
        .to_owned();
    let new_sub = moved["value"]["moved"][1]["id"]
        .as_str()
        .expect("sub")
        .to_owned();
    let values = field_values(&setup, &new_root).await;
    assert_eq!(
        values[field_id(&setup, &w.dst, "Risk").await],
        option_id(&setup, &w.dst, "Risk", "Medium").await
    );
    assert_eq!(
        values[field_id(&setup, &w.dst, "Notes").await],
        "plain note"
    );
    assert_eq!(
        values[field_id(&setup, &w.dst, "Owner").await],
        w.dee.to_string()
    );
    assert_eq!(
        values[field_id(&setup, &w.dst, "Follows").await],
        new_sub,
        "the link points at the subtask's new copy"
    );
    assert_eq!(values.as_object().expect("values").len(), 4, "{values}");
    let sub_values = field_values(&setup, &new_sub).await;
    assert_eq!(
        sub_values[field_id(&setup, &w.dst, "Owner").await],
        w.dee.to_string()
    );
    assert_eq!(
        sub_values.as_object().expect("values").len(),
        1,
        "{sub_values}"
    );
    // The original keeps everything, archived.
    assert_eq!(field_values(&setup, &w.root).await, original);
    // Nothing that stayed is named in the target.
    let rows = target_rows(&setup, &w.dst).await;
    for secret in [SECRET_TEXT, SECRET_PERSON, SECRET_PAGE] {
        assert!(
            !rows.contains(secret),
            "{secret} reached the target: {rows}"
        );
    }
    assert!(
        rows.contains("2 custom field values stayed on SRC-1")
            && rows.contains("1 custom field value stayed on SRC-2"),
        "{rows}"
    );
    let after = footprint(&setup).await;

    // 4. The same request again (a lost response): its first result, and
    // nothing more is written.
    let again = tool(app, &codex, &session, "task_move", confirm.clone()).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["replayed"], true, "{again}");
    assert_eq!(again["value"]["moved"], moved["value"]["moved"]);
    assert!(
        again["value"].get("notes").is_none(),
        "notes aren't kept: {again}"
    );
    assert_eq!(footprint(&setup).await, after, "a replay writes nothing");
    // Changed choices with the same ID: a conflict.
    let mut changed = confirm.clone();
    changed["resolutions"][3] =
        json!({"issue": id_of("SRC-1", "Secret"), "action": "set", "targetField": "Summary"});
    let conflict = tool(app, &codex, &session, "task_move", changed).await;
    assert_eq!(conflict["outcome"], "conflict", "{conflict}");
    // A new request ID: the task was moved, so nothing changes.
    let mut other = confirm.clone();
    other["clientRequestId"] = json!("move-src-1-again");
    let other = tool(app, &codex, &session, "task_move", other).await;
    assert_eq!(other["outcome"], "ok", "{other}");
    assert_eq!(other["value"]["created"], false, "{other}");
    assert_eq!(footprint(&setup).await, after);

    // 5. However late: the receipt doesn't expire with the day-long
    // request records.
    sqlx::query("UPDATE work_item_move_receipts SET created_at = now() - interval '400 days'")
        .execute(&setup.pool)
        .await
        .expect("age the receipt");
    sqlx::query("DELETE FROM idempotency_keys")
        .execute(&setup.pool)
        .await
        .expect("expire request records");
    let late = tool(app, &codex, &session, "task_move", confirm.clone()).await;
    assert_eq!(late["value"]["replayed"], true, "{late}");
    assert_eq!(late["value"]["moved"], moved["value"]["moved"]);
    // A move made before receipts existed: the same request finds where it
    // went, and nothing is made twice.
    sqlx::query("DELETE FROM work_item_move_receipts")
        .execute(&setup.pool)
        .await
        .expect("forget the receipt");
    let older = tool(app, &codex, &session, "task_move", confirm).await;
    assert_eq!(older["outcome"], "ok", "{older}");
    assert_eq!(older["value"]["created"], false, "{older}");
    let after_forgetting = footprint(&setup).await;
    assert_eq!(after_forgetting[0], after[0], "no second copy");
}

#[tokio::test]
async fn a_stale_preview_or_lost_access_moves_nothing() {
    let Some(setup) = setup("awd102_move_refusals").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    let preview = |who: &'static str| {
        let codex = codex.clone();
        let session = session.clone();
        async move {
            let previewed = tool(
                app,
                &codex,
                &session,
                "task_move",
                json!({"taskId": "SRC-1", "toProjectId": "DST", "preview": true}),
            )
            .await;
            assert_eq!(previewed["outcome"], "ok", "{who}: {previewed}");
            previewed["value"]["preview"].clone()
        }
    };
    let confirm = |preview: &Value, request: &str| {
        json!({"taskId": "SRC-1", "toProjectId": "DST", "snapshot": preview["snapshot"],
               "leaveUnresolved": true, "clientRequestId": request})
    };
    let before = footprint(&setup).await;

    // The target's fields change after the preview.
    let stale = preview("first").await;
    let changed = tool(
        app,
        &codex,
        &session,
        "field_write",
        json!({"projectId": "DST", "field": "Risk", "options": [
            {"id": option_id(&setup, &w.dst, "Risk", "Low").await, "name": "Low"},
            {"id": option_id(&setup, &w.dst, "Risk", "Medium").await, "name": "Medium"},
            {"name": "High"},
        ]}),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    let before = {
        let _ = before;
        footprint(&setup).await
    };
    let refused = tool(
        app,
        &codex,
        &session,
        "task_move",
        confirm(&stale, "move-stale"),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(footprint(&setup).await, before);

    // The target's milestone of the same name is replaced by another after
    // the preview: the same name, a different record.
    for (project, tasks) in [("SRC", json!(["SRC-1"])), ("DST", json!([]))] {
        let written = tool(
            app,
            &codex,
            &session,
            "milestone_write",
            json!({"projectId": project, "name": "M1", "addTasks": tasks}),
        )
        .await;
        assert_eq!(written["outcome"], "ok", "{written}");
    }
    let planned = preview("milestone").await;
    let old = tool(
        app,
        &codex,
        &session,
        "milestone_write",
        json!({"projectId": "DST", "name": "M1"}),
    )
    .await;
    let old_id = old["value"]["id"].as_str().expect("milestone").to_owned();
    let archived = tool(
        app,
        &codex,
        &session,
        "milestone_write",
        json!({"projectId": "DST", "milestoneId": old_id, "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let replaced = tool(
        app,
        &codex,
        &session,
        "milestone_write",
        json!({"projectId": "DST", "name": "M1"}),
    )
    .await;
    assert_eq!(replaced["outcome"], "ok", "{replaced}");
    assert_ne!(replaced["value"]["id"], old_id, "a new record");
    let before = footprint(&setup).await;
    let refused = tool(
        app,
        &codex,
        &session,
        "task_move",
        confirm(&planned, "move-milestone"),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(footprint(&setup).await, before);

    // A subtask leaves the tree after the preview (its mapping is late).
    let fresh = preview("second").await;
    let archived = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "SRC-2", "archived": true, "expectedVersion": version_of(&setup, "SRC-2").await}),
    )
    .await;
    assert_eq!(archived["outcome"], "applied", "{archived}");
    let before = footprint(&setup).await;
    let refused = tool(
        app,
        &codex,
        &session,
        "task_move",
        confirm(&fresh, "move-tree"),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(footprint(&setup).await, before);
    let restored = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "SRC-2", "archived": false, "expectedVersion": version_of(&setup, "SRC-2").await}),
    )
    .await;
    assert_eq!(restored["outcome"], "applied", "{restored}");

    // Someone who loses the right to create in the target after previewing.
    let (mo, mo_browser) = add_person(&setup, &[&w.src, &w.dst], "moira").await;
    let mo_preview = |browser: &super::agent_interface::Browser| {
        browser.request(
            "GET",
            &format!(
                "/api/v1/projects/{}/work-items/{}/project-move?targetProjectId={}",
                w.src, w.root, w.dst
            ),
            None,
            Value::Null,
        )
    };
    let (status, plan) = send(app, mo_preview(&mo_browser)).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    sqlx::query("UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1::uuid AND actor_id = $2")
        .bind(&w.dst)
        .bind(mo)
        .execute(&setup.pool)
        .await
        .expect("demote");
    let before = footprint(&setup).await;
    let (status, body) = send(
        app,
        mo_browser.request(
            "POST",
            &format!("/api/v1/projects/{}/work-items/{}/project-move", w.src, w.root),
            Some("mo-move"),
            json!({"targetProjectId": w.dst, "snapshot": plan["snapshot"], "leaveUnresolved": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(footprint(&setup).await, before);
    // ... or loses the source altogether.
    sqlx::query("UPDATE project_memberships SET enabled = false WHERE project_id = $1::uuid AND actor_id = $2")
        .bind(&w.src)
        .bind(mo)
        .execute(&setup.pool)
        .await
        .expect("remove");
    let before = footprint(&setup).await;
    let (status, body) = send(app, mo_preview(&mo_browser)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(footprint(&setup).await, before);

    // An agent in SRC only reaches no DST; one that only reads DST can't
    // move into it.
    let (agent, agent_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mover", "mover").await;
    add_member(&setup, &w.src, agent, "member").await;
    let agent_session = initialize(app, &agent_token).await.expect("agent MCP");
    // Each refusal is measured from just before it: making the agent and its
    // memberships legitimately writes audit rows and events.
    let before = footprint(&setup).await;
    let refused = tool(
        app,
        &agent_token,
        &agent_session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": w.dst, "preview": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    assert_eq!(footprint(&setup).await, before);
    add_member(&setup, &w.dst, agent, "viewer").await;
    let before = footprint(&setup).await;
    let refused = tool(
        app,
        &agent_token,
        &agent_session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "preview": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(footprint(&setup).await, before);

    // Two identical requests at once make one move: the same person and
    // request ID from two MCP sessions, as two tabs or retries would send
    // them. (Two concurrent calls on one session never reached the move in
    // this harness: run local1794091998_3834933 timed out after both had
    // authenticated, with no database wait.)
    let fresh = preview("third").await;
    let request = confirm(&fresh, "move-twice");
    let other_session = initialize(app, &codex).await.expect("second Codex session");
    let (first, second) = tokio::join!(
        tool(app, &codex, &session, "task_move", request.clone()),
        tool(app, &codex, &other_session, "task_move", request.clone()),
    );
    assert_eq!(first["outcome"], "ok", "{first}");
    assert_eq!(second["outcome"], "ok", "{second}");
    assert_eq!(first["value"]["moved"], second["value"]["moved"]);
    assert_eq!(
        [&first, &second]
            .iter()
            .filter(|result| result["value"]["replayed"] == true)
            .count(),
        1,
        "{first} {second}"
    );
    let copies =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_items WHERE project_id = $1::uuid")
            .bind(&w.dst)
            .fetch_one(&setup.pool)
            .await
            .expect("copies");
    assert_eq!(copies, 2, "the root and its subtask, once");
    let _ = w.sub;
}

#[tokio::test]
async fn a_move_without_a_preview_carries_what_fits_and_keeps_the_rest() {
    let Some(setup) = setup("awd102_move_legacy").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    let original = field_values(&setup, &w.root).await;

    // The two arguments task_move always took: it moves at once.
    let moved = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["created"], true);
    assert_eq!(moved["value"]["leftBehind"], 5, "{moved}");
    let notes = moved["value"]["notes"].to_string();
    assert!(
        notes.contains("\\\"Secret\\\" on SRC-1 stayed on the original"),
        "{notes}"
    );
    let new_root = moved["value"]["moved"][0]["id"]
        .as_str()
        .expect("root")
        .to_owned();
    let values = field_values(&setup, &new_root).await;
    assert_eq!(
        values.as_object().expect("values").len(),
        2,
        "Notes and Follows: {values}"
    );
    assert_eq!(field_values(&setup, &w.root).await, original);
    let rows = target_rows(&setup, &w.dst).await;
    for secret in [SECRET_TEXT, SECRET_PERSON, SECRET_PAGE] {
        assert!(!rows.contains(secret), "{secret} reached the target");
    }
    // Moving again returns where it went, as it always did.
    let again = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST"}),
    )
    .await;
    assert_eq!(again["value"]["created"], false, "{again}");
    assert_eq!(
        again["value"]["moved"][0]["to"],
        moved["value"]["moved"][0]["to"]
    );

    // The web app moves too, and always previews first.
    let (status, body) = send(
        app,
        setup.browser.request(
            "POST",
            &format!(
                "/api/v1/projects/{}/work-items/{}/project-move",
                w.src,
                task_id(&setup, &w.src, 3).await
            ),
            Some("web-move-no-preview"),
            json!({"targetProjectId": w.dst, "snapshot": "0".repeat(64)}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let stays = task_id(&setup, &w.src, 3).await;
    let (status, plan) = send(
        app,
        setup.browser.request(
            "GET",
            &format!(
                "/api/v1/projects/{}/work-items/{stays}/project-move?targetProjectId={}",
                w.src, w.dst
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let request = json!({"targetProjectId": w.dst, "snapshot": plan["snapshot"]});
    let path = format!("/api/v1/projects/{}/work-items/{stays}/project-move", w.src);
    let (status, done) = send(
        app,
        setup
            .browser
            .request("POST", &path, Some("web-move"), request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (status, replay) = send(
        app,
        setup
            .browser
            .request("POST", &path, Some("web-move"), request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["replayed"], true, "{replay}");
    assert_eq!(replay["moved"], done["moved"]);
    let (status, conflict) = send(
        app,
        setup.browser.request(
            "POST",
            &path,
            Some("web-move"),
            json!({"targetProjectId": w.dst, "snapshot": plan["snapshot"], "leaveUnresolved": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
}

/// Counts of what a move makes: tasks, aliases, receipts, archived tasks.
async fn moves_made(setup: &Setup) -> Vec<i64> {
    let mut counts = Vec::new();
    for query in [
        "SELECT count(*) FROM work_items",
        "SELECT count(*) FROM work_item_key_aliases",
        "SELECT count(*) FROM work_item_move_receipts",
        "SELECT count(*) FROM work_items WHERE archived_at IS NOT NULL",
    ] {
        counts.push(
            sqlx::query_scalar::<_, i64>(query)
                .fetch_one(&setup.pool)
                .await
                .expect(query),
        );
    }
    counts
}

/// The task's old key names the move it made, through the tool's own key
/// resolution: the same request by key replays, a changed one conflicts,
/// and neither moves the copy, even after the copy moved on.
#[tokio::test]
async fn a_move_named_by_its_old_key_replays_or_conflicts() {
    let Some(setup) = setup("awd102_move_keys").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let _w = world(&setup, &codex, &session).await;
    let third = create_project(&setup, "EEE", "Third").await;
    let call = |arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, "task_move", arguments).await }
    };

    let first = json!({"taskId": "SRC-1", "toProjectId": "DST", "clientRequestId": "key-move-1"});
    let moved = call(first.clone()).await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["created"], true, "{moved}");
    let after = moves_made(&setup).await;
    // The same request by key: its first result, nothing new.
    let again = call(first.clone()).await;
    assert_eq!(again["value"]["replayed"], true, "{again}");
    assert_eq!(again["value"]["moved"], moved["value"]["moved"]);
    assert_eq!(moves_made(&setup).await, after);
    // The same request ID with another target: a conflict, and the copy in
    // DST doesn't move to EEE.
    let mut elsewhere = first.clone();
    elsewhere["toProjectId"] = json!("EEE");
    let conflict = call(elsewhere.clone()).await;
    assert_eq!(conflict["outcome"], "conflict", "{conflict}");
    assert_eq!(moves_made(&setup).await, after);
    let in_third =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_items WHERE project_id = $1::uuid")
            .bind(&third)
            .fetch_one(&setup.pool)
            .await
            .expect("third");
    assert_eq!(in_third, 0, "nothing reached EEE");

    // The copy moves on, by its own key and request ID.
    let copy_key = moved["value"]["moved"][0]["to"]
        .as_str()
        .expect("copy")
        .to_owned();
    let onward = json!({"taskId": copy_key, "toProjectId": "EEE", "clientRequestId": "key-move-2"});
    let moved_on = call(onward.clone()).await;
    assert_eq!(moved_on["outcome"], "ok", "{moved_on}");
    assert_eq!(moved_on["value"]["created"], true, "{moved_on}");
    let after = moves_made(&setup).await;
    // Each request still answers as it first did.
    let first_again = call(first).await;
    assert_eq!(first_again["value"]["replayed"], true, "{first_again}");
    assert_eq!(
        first_again["value"]["moved"][0]["to"],
        moved["value"]["moved"][0]["to"]
    );
    let onward_again = call(onward).await;
    assert_eq!(onward_again["value"]["replayed"], true, "{onward_again}");
    assert_eq!(onward_again["value"]["moved"], moved_on["value"]["moved"]);
    let conflict = call(elsewhere).await;
    assert_eq!(conflict["outcome"], "conflict", "{conflict}");
    // Without a request ID, the old key says where it went.
    let plain = call(json!({"taskId": "SRC-1", "toProjectId": "EEE"})).await;
    assert_eq!(plain["value"]["created"], false, "{plain}");
    assert_eq!(moves_made(&setup).await, after, "nothing moved twice");
}

/// An agent's reach rests on its connection's home project. Archiving that
/// project while the agent's move waits for a lock stops the move: it reads
/// reach only once it holds its locks.
#[tokio::test]
async fn archiving_an_agents_home_while_its_move_waits_stops_the_move() {
    let Some(setup) = setup("awd102_move_anchor").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    // Made last, so its ID sorts after both projects the move locks.
    let home = create_project(&setup, "HOME", "Home").await;
    let (agent, token) = create_agent(app, &setup.browser, &home, "Mover", "mover").await;
    add_member(&setup, &w.src, agent, "member").await;
    add_member(&setup, &w.dst, agent, "member").await;
    let agent_session = initialize(app, &token).await.expect("agent MCP");
    let before = moves_made(&setup).await;

    // Another write holds the source project's row.
    let mut held = setup.pool.begin().await.expect("begin");
    sqlx::query("SELECT 1 FROM projects WHERE id = $1::uuid FOR UPDATE")
        .bind(&w.src)
        .execute(&mut *held)
        .await
        .expect("hold the source");
    let holder = sqlx::query_scalar::<_, i64>("SELECT txid_current() % 4294967296")
        .fetch_one(&mut *held)
        .await
        .expect("holder");
    // The raw call: the result's schema check would ask again with the agent's
    // credential, which archiving its home has made useless by then.
    let moving = call_tool(
        app,
        &token,
        &agent_session,
        "task_move",
        json!({"taskId": w.root, "toProjectId": w.dst}),
    );
    let archiving = async {
        a_move_is_waiting(&setup, holder).await;
        let (status, body) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{home}/archive"),
                Some("archive-home"),
                json!({"expectedVersion": 1}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        held.commit().await.expect("release");
    };
    let ((_, moved), ()) = tokio::join!(moving, archiving);
    let moved = structured(&moved);
    assert_eq!(moved["outcome"], "not_found", "{moved}");
    assert_eq!(moves_made(&setup).await, before, "nothing moved");
}

/// The move dialog's projects come from the mover's actual permissions in
/// each, custom roles included, not their role's name: a role that adds
/// tasks is offered whatever it's called, one that can't isn't, and the
/// list grants nothing (the preview and the move check again).
#[tokio::test]
async fn only_projects_the_mover_can_add_tasks_to_are_offered() {
    let Some(setup) = setup("awd102_move_destinations").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    let editors = create_project(&setup, "EDIT", "Editors only").await;
    let looking = create_project(&setup, "LOOK", "Look only").await;
    let closed = create_project(&setup, "SHUT", "Shut").await;
    let (mo, mo_browser) = add_person(
        &setup,
        &[&w.src, &w.dst, &editors, &looking, &closed],
        "moira",
    )
    .await;
    let roles = format!("/api/v1/organizations/{}/roles", setup.organization_id);
    let mut role_ids = Vec::new();
    for (name, permissions) in [
        // An unfamiliar name, with what a move needs.
        ("Quartermaster", json!(["tasks.view", "tasks.create"])),
        // A member-like role without adding tasks.
        ("Editor", json!(["tasks.view", "tasks.edit"])),
    ] {
        let (status, role) = send(
            app,
            setup.browser.request(
                "POST",
                &roles,
                None,
                json!({"name": name, "baseRole": "member", "permissions": permissions}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{role}");
        role_ids.push(
            role["id"]
                .as_str()
                .expect("role")
                .parse::<Uuid>()
                .expect("id"),
        );
    }
    for (project, role, role_id) in [
        (&w.dst, "member", Some(role_ids[0])),
        (&editors, "member", Some(role_ids[1])),
        (&looking, "viewer", None),
    ] {
        sqlx::query(
            "UPDATE project_memberships SET role = $3, role_id = $4 WHERE project_id = $1::uuid AND actor_id = $2",
        )
        .bind(project)
        .bind(mo)
        .bind(role)
        .bind(role_id)
        .execute(&setup.pool)
        .await
        .expect("role");
    }
    sqlx::query(
        "UPDATE projects SET archived_at = now(), archived_by_actor_id = $2 WHERE id = $1::uuid",
    )
    .bind(&closed)
    .bind(mo)
    .execute(&setup.pool)
    .await
    .expect("archive");
    let before = moves_made(&setup).await;
    let destinations = |browser: &super::agent_interface::Browser, task: &str| {
        browser.request(
            "GET",
            &format!(
                "/api/v1/projects/{}/work-items/{task}/project-move/destinations",
                w.src
            ),
            None,
            Value::Null,
        )
    };
    let (status, listed) = send(app, destinations(&mo_browser, &w.root)).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let keys = listed["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .map(|project| project["key"].clone())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec![json!("DST")], "{listed}");
    // The custom role that adds tasks can really preview and move there.
    let (status, plan) = send(
        app,
        mo_browser.request(
            "GET",
            &format!(
                "/api/v1/projects/{}/work-items/{}/project-move?targetProjectId={}",
                w.src, w.root, w.dst
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    // One that doesn't is refused by the preview and the move too.
    let (status, refused) = send(
        app,
        mo_browser.request(
            "GET",
            &format!(
                "/api/v1/projects/{}/work-items/{}/project-move?targetProjectId={editors}",
                w.src, w.root
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, refused) = send(
        app,
        mo_browser.request(
            "POST",
            &format!("/api/v1/projects/{}/work-items/{}/project-move", w.src, w.root),
            Some("mo-editors"),
            json!({"targetProjectId": editors, "snapshot": plan["snapshot"], "leaveUnresolved": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(
        moves_made(&setup).await,
        before,
        "listing and refusals change nothing"
    );
    // Roles are as they were: the list granted nothing.
    let role_now = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT role_id FROM project_memberships WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(&editors)
    .bind(mo)
    .fetch_one(&setup.pool)
    .await
    .expect("role");
    assert_eq!(role_now, Some(role_ids[1]));
    // Someone who can't move tasks out of the source gets no list.
    sqlx::query("UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1::uuid AND actor_id = $2")
        .bind(&w.src)
        .bind(mo)
        .execute(&setup.pool)
        .await
        .expect("viewer");
    let (status, refused) = send(app, destinations(&mo_browser, &w.root)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
}

/// A task whose type has no match in the target lands on the target's
/// default type. Switching that default after the preview is a change: the
/// preview is out of date, and nothing moves.
#[tokio::test]
async fn a_changed_default_type_makes_the_preview_stale() {
    let Some(setup) = setup("awd102_move_default").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    // No type of DST shares a name with SRC's, so the default is used.
    sqlx::query("UPDATE work_item_types SET name = 'DST ' || name WHERE project_id = $1::uuid")
        .bind(&w.dst)
        .execute(&setup.pool)
        .await
        .expect("rename");
    let previewed = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "preview": true}),
    )
    .await;
    assert_eq!(previewed["outcome"], "ok", "{previewed}");
    let snapshot = previewed["value"]["preview"]["snapshot"].clone();
    let other = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_types WHERE project_id = $1::uuid AND enabled AND NOT is_default ORDER BY position LIMIT 1",
    )
    .bind(&w.dst)
    .fetch_one(&setup.pool)
    .await
    .expect("another type");
    // One default at a time (work_item_types_one_default): clear, then set.
    sqlx::query("UPDATE work_item_types SET is_default = false WHERE project_id = $1::uuid")
        .bind(&w.dst)
        .execute(&setup.pool)
        .await
        .expect("clear the default");
    sqlx::query(
        "UPDATE work_item_types SET is_default = true WHERE project_id = $1::uuid AND id = $2",
    )
    .bind(&w.dst)
    .bind(other)
    .execute(&setup.pool)
    .await
    .expect("set the default");
    let before = moves_made(&setup).await;
    let refused = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "snapshot": snapshot,
               "leaveUnresolved": true, "clientRequestId": "move-default"}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(moves_made(&setup).await, before);
    // A fresh preview moves onto the new default.
    let fresh = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "preview": true}),
    )
    .await;
    let moved = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "SRC-1", "toProjectId": "DST", "snapshot": fresh["value"]["preview"]["snapshot"],
               "leaveUnresolved": true, "clientRequestId": "move-default-2"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let landed =
        sqlx::query_scalar::<_, Uuid>("SELECT type_id FROM work_items WHERE id = $1::uuid")
            .bind(moved["value"]["moved"][0]["id"].as_str().expect("id"))
            .fetch_one(&setup.pool)
            .await
            .expect("type");
    assert_eq!(landed, other);
}

/// Waits until some backend is waiting on `holder`, this test's own open
/// transaction (by its transaction ID), so no other test's waits count.
async fn a_move_is_waiting(setup: &Setup, holder: i64) {
    for _ in 0..200 {
        let waiting = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_locks WHERE locktype = 'transactionid' AND NOT granted AND transactionid::text::bigint = $1",
        )
        .bind(holder)
        .fetch_one(&setup.pool)
        .await
        .expect("locks");
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the move never waited for the held project");
}

/// Removing an agent takes its home project, then its connection and
/// membership, then its actor. A move by that agent takes project rows
/// first and its actor last, so a removal that starts while the move waits
/// never waits on the move: it finishes, and the move then finds the agent
/// gone and moves nothing.
#[tokio::test]
async fn removing_an_agent_never_waits_on_its_waiting_move() {
    let Some(setup) = setup("awd102_move_remove_agent").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    let home = create_project(&setup, "HOME", "Home").await;
    let (agent, token) = create_agent(app, &setup.browser, &home, "Mover", "mover").await;
    add_member(&setup, &w.src, agent, "member").await;
    add_member(&setup, &w.dst, agent, "member").await;
    let agent_session = initialize(app, &token).await.expect("agent MCP");
    let (connection, version) = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, version FROM external_agent_connections WHERE actor_id = $1",
    )
    .bind(agent)
    .fetch_one(&setup.pool)
    .await
    .expect("connection");
    let before = moves_made(&setup).await;

    // Another write holds the source project's row; the move waits for it.
    let mut held = setup.pool.begin().await.expect("begin");
    sqlx::query("SELECT 1 FROM projects WHERE id = $1::uuid FOR UPDATE")
        .bind(&w.src)
        .execute(&mut *held)
        .await
        .expect("hold the source");
    let holder = sqlx::query_scalar::<_, i64>("SELECT txid_current() % 4294967296")
        .fetch_one(&mut *held)
        .await
        .expect("holder");
    let moving = tokio::spawn({
        let app = app.clone();
        let root = w.root.clone();
        let dst = w.dst.clone();
        // The raw call: the result's schema check would ask again with the
        // agent's credential, which removing the agent has revoked by then.
        async move {
            call_tool(
                &app,
                &token,
                &agent_session,
                "task_move",
                json!({"taskId": root, "toProjectId": dst}),
            )
            .await
        }
    });
    a_move_is_waiting(&setup, holder).await;
    // The removal finishes while the move still waits.
    let (status, removed) = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{home}/external-agents/{connection}/remove"),
                Some("remove-mover"),
                json!({"expectedVersion": version}),
            ),
        ),
    )
    .await
    .expect("the removal doesn't wait on the move");
    assert_eq!(status, StatusCode::OK, "{removed}");
    held.commit().await.expect("release");
    let moved = structured(&moving.await.expect("move").1);
    assert_eq!(moved["outcome"], "not_found", "{moved}");
    assert_eq!(moves_made(&setup).await, before, "nothing moved");
}

/// A stored receipt is the move's original result, but it is only ever
/// returned with access now: a caller who has lost the target or the
/// source gets a refusal that names nothing of it, and the receipt is still
/// there for when access comes back.
#[tokio::test]
async fn a_receipt_is_returned_only_with_access_now() {
    let Some(setup) = setup("awd102_move_receipt_access").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let w = world(&setup, &codex, &session).await;
    let (agent, token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mover", "mover").await;
    add_member(&setup, &w.src, agent, "member").await;
    add_member(&setup, &w.dst, agent, "member").await;
    let agent_session = initialize(app, &token).await.expect("agent MCP");
    // By ID, so the move's own checks answer, not key resolution.
    let request = json!({"taskId": w.root, "toProjectId": w.dst, "clientRequestId": "agent-move"});
    let moved = tool(app, &token, &agent_session, "task_move", request.clone()).await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["created"], true, "{moved}");
    let new_root = moved["value"]["moved"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let after = moves_made(&setup).await;
    let again = tool(app, &token, &agent_session, "task_move", request.clone()).await;
    assert_eq!(again["value"]["replayed"], true, "{again}");

    let set_enabled = |project: String, enabled: bool| {
        let pool = setup.pool.clone();
        async move {
            sqlx::query(
                "UPDATE project_memberships SET enabled = $3 WHERE project_id = $1::uuid AND actor_id = $2",
            )
            .bind(project)
            .bind(agent)
            .bind(enabled)
            .execute(&pool)
            .await
            .expect("membership");
        }
    };
    for lost in [w.dst.clone(), w.src.clone()] {
        set_enabled(lost.clone(), false).await;
        let refused = tool(app, &token, &agent_session, "task_move", request.clone()).await;
        assert_ne!(refused["outcome"], "ok", "{refused}");
        assert!(
            refused["outcome"] == "not_found" || refused["outcome"] == "permission_denied",
            "{refused}"
        );
        assert!(
            !refused.to_string().contains(&new_root),
            "the receipt leaked: {refused}"
        );
        assert_eq!(
            moves_made(&setup).await,
            after,
            "a refused replay changes nothing"
        );
        set_enabled(lost, true).await;
    }
    // Access back: the same original result.
    let restored = tool(app, &token, &agent_session, "task_move", request).await;
    assert_eq!(restored["value"]["replayed"], true, "{restored}");
    assert_eq!(restored["value"]["moved"], moved["value"]["moved"]);
}

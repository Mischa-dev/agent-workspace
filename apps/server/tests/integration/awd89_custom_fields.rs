//! AWD-89: custom fields on tasks. The silent, costly failures: someone who
//! isn't an owner or admin changing a project's fields; a value of the wrong
//! type, a person outside the project or a link to another project's task
//! being stored; a change that leaves no before and after in history; a
//! filter, sort or group that reads the wrong values; and an archived field
//! losing its values.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::add_person,
};

fn field_values(fields: &Value) -> Vec<(String, Value)> {
    fields
        .as_array()
        .unwrap_or_else(|| panic!("no fields: {fields}"))
        .iter()
        .map(|field| {
            (
                field["name"].as_str().expect("name").to_owned(),
                field["value"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn fields_are_typed_checked_recorded_and_filtered_the_same_for_agents_and_the_web() {
    let Some(setup) = setup("awd89_fields").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let side = create_project(&setup, "SIDE", "Side").await;
    // The owner's connected app acts as the owner.
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let owner = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    // Mara is an agent and a member of WORK, not an admin.
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let agent = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    // Owners and admins manage fields; members can't, over MCP or the web.
    for (name, extra) in [
        (
            "Risk",
            json!({"type": "single_choice", "options": [{"name": "Low"}, {"name": "High", "color": "red"}]}),
        ),
        ("Points", json!({"type": "number"})),
        ("Owner", json!({"type": "person"})),
        ("Shipped", json!({"type": "checkbox"})),
        ("Follows", json!({"type": "link", "target": "task"})),
        ("Launch", json!({"type": "date"})),
    ] {
        let mut arguments = json!({"projectId": "WORK", "name": name});
        for (key, value) in extra.as_object().expect("extra") {
            arguments[key] = value.clone();
        }
        let written = owner("field_write", arguments).await;
        assert_eq!(written["outcome"], "ok", "{written}");
        assert_eq!(written["value"]["created"], true, "{written}");
    }
    let refused = agent(
        "field_write",
        json!({"projectId": "WORK", "name": "Mine", "type": "text"}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let listed = agent("field_list", json!({"projectId": "WORK"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["canManage"], false);
    let names = listed["value"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .map(|field| field["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["Risk", "Points", "Owner", "Shipped", "Follows", "Launch"]
    );
    let risk = listed["value"]["fields"][0]["id"]
        .as_str()
        .expect("risk")
        .to_owned();
    let (_, member_browser) = add_person(&setup, &[&work], "pat").await;
    let (status, body) = send(
        app,
        member_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/fields"),
            Some("member-field"),
            json!({"name": "Mine", "type": "text"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/fields"),
            Some("owner-field"),
            json!({"name": "Customer", "type": "text"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let customer = body["id"].as_str().expect("customer").to_owned();

    // Values go in by field name, typed, as anyone in the project writes them.
    let other = owner(
        "task_create",
        json!({"projectId": "SIDE", "title": "Elsewhere", "clientRequestId": "side-1"}),
    )
    .await;
    assert_eq!(other["outcome"], "applied", "{other}");
    let first = agent(
        "task_create",
        json!({"projectId": "WORK", "title": "First", "clientRequestId": "first"}),
    )
    .await;
    assert_eq!(first["outcome"], "applied", "{first}");
    let created = agent(
        "task_create",
        json!({
            "projectId": "WORK",
            "title": "Ship fields",
            "fields": {"risk": "high", "Points": "3", "Owner": "me", "Shipped": true,
                       "Follows": "WORK-1", "Launch": "2026-12-01"},
            "clientRequestId": "fields-1",
        }),
    )
    .await;
    assert_eq!(created["outcome"], "applied", "{created}");
    let opened = agent("task_open", json!({"taskId": "WORK-2"})).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(
        field_values(&opened["value"]["fields"]),
        [
            ("Risk".to_owned(), json!("High")),
            ("Points".to_owned(), json!(3)),
            ("Owner".to_owned(), json!("@mara")),
            ("Shipped".to_owned(), json!(true)),
            ("Follows".to_owned(), json!("WORK-1")),
            ("Launch".to_owned(), json!("2026-12-01")),
            ("Customer".to_owned(), Value::Null),
        ]
    );
    let version = opened["value"]["task"]["version"]
        .as_i64()
        .expect("version");

    // Wrong types, unknown fields, people outside the project and links to
    // another project's tasks are refused, and nothing changes.
    let side_task = other["workItemId"].as_str().expect("side task").to_owned();
    for (fields, at) in [
        (json!({"Points": "lots"}), "fields.Points"),
        (json!({"Risk": "Medium"}), "fields.Risk"),
        (json!({"Launch": "next week"}), "fields.Launch"),
        (json!({"Shipped": "maybe"}), "fields.Shipped"),
        (json!({"Nope": 1}), "fields"),
        (json!({"Owner": "@nobody"}), "fields.Owner"),
        (json!({"Owner": uuid::Uuid::now_v7()}), "fields.Owner"),
        (json!({"Follows": side_task}), "fields.Follows"),
        (json!({"Follows": "SIDE-1"}), "fields.Follows"),
    ] {
        let refused = agent(
            "task_update",
            json!({"taskId": "WORK-2", "expectedVersion": version, "fields": fields}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{fields}: {refused}");
        assert_eq!(refused["field"], at, "{refused}");
    }
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!(
                "/api/v1/projects/{work}/work-items/{}",
                created["workItemId"].as_str().expect("id")
            ),
            Some("web-wrong"),
            json!({"expectedVersion": version, "fieldValues": {risk.clone(): "Medium"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // A change records each field's before and after, in one version bump.
    let updated = agent(
        "task_update",
        json!({"taskId": "WORK-2", "expectedVersion": version,
               "fields": {"Risk": "Low", "Points": null}}),
    )
    .await;
    assert_eq!(updated["outcome"], "applied", "{updated}");
    assert_eq!(updated["version"], version + 1, "{updated}");
    assert_eq!(updated["changedFields"], json!(["fields"]), "{updated}");
    let history = agent(
        "task_open",
        json!({"taskId": "WORK-2", "include": ["history"]}),
    )
    .await;
    let text = history["value"]["history"].to_string();
    assert!(text.contains("Risk: High → Low"), "{text}");
    assert!(text.contains("Points: 3 → none"), "{text}");

    // The web reads and writes the same values, keyed by field ID.
    let task_id = created["workItemId"].as_str().expect("id").to_owned();
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/work-items/{task_id}"),
            Some("web-customer"),
            json!({"expectedVersion": version + 1, "fieldValues": {customer.clone(): "Acme"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["fieldValues"][&customer], "Acme", "{body}");
    assert!(
        body["fieldReferences"]
            .as_array()
            .expect("references")
            .iter()
            .any(|reference| reference["key"] == "@mara"),
        "{body}"
    );
    let (_, timeline) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items/{task_id}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await;
    let change = timeline.to_string();
    assert!(change.contains(&format!("field:{customer}")), "{change}");
    assert!(change.contains("\"Acme\""), "{change}");

    // task_list filters on fields, by name, as stored values.
    for (filters, expected) in [
        (
            json!([{"field": "Risk", "op": "is", "values": ["low"]}]),
            vec!["WORK-2"],
        ),
        (
            json!([{"field": "Risk", "op": "is", "values": ["High"]}]),
            vec![],
        ),
        (json!([{"field": "Risk", "op": "empty"}]), vec!["WORK-1"]),
        (
            json!([{"field": "Owner", "op": "is", "values": ["me"]}]),
            vec!["WORK-2"],
        ),
        (
            json!([{"field": "Shipped", "op": "is", "values": ["false"]}]),
            vec!["WORK-1"],
        ),
        (
            json!([{"field": "Launch", "op": "gt", "values": ["2026-11-30"]}]),
            vec!["WORK-2"],
        ),
        (
            json!([{"field": "Customer", "op": "contains", "values": ["acm"]}]),
            vec!["WORK-2"],
        ),
    ] {
        let listed = agent(
            "task_list",
            json!({"projectId": "WORK", "fieldFilters": filters}),
        )
        .await;
        assert_eq!(listed["outcome"], "ok", "{filters}: {listed}");
        let keys = listed["value"]["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .map(|task| task["key"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(keys, expected, "{filters}");
    }
    let listed = agent("task_list", json!({"projectId": "WORK", "query": "WORK-2"})).await;
    assert_eq!(
        listed["value"]["tasks"][0]["fields"]["Risk"], "Low",
        "{listed}"
    );
    let refused = agent(
        "task_list",
        json!({"projectId": "WORK", "fieldFilters": [{"field": "Risk", "op": "gt", "values": ["1"]}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    // Fields belong to one project, which the caller names. Mara's only
    // project with Tasks is WORK (her agent space has none since modules,
    // AWD-75), but reaching one project isn't choosing it.
    let refused = agent(
        "task_list",
        json!({"fieldFilters": [{"field": "Risk", "op": "empty"}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    assert_eq!(refused["field"], "fieldFilters", "{refused}");
    // Nor across WORK and SIDE.
    let refused = owner(
        "task_list",
        json!({"fieldFilters": [{"field": "Risk", "op": "empty"}]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    assert_eq!(refused["field"], "fieldFilters", "{refused}");

    // The web list filters, sorts and groups by field ID; another project's
    // field is refused.
    let low = sqlx::query_scalar::<_, String>(
        "SELECT option ->> 'id' FROM field_definitions, jsonb_array_elements(config -> 'options') option \
         WHERE id = $1::uuid AND option ->> 'name' = 'Low'",
    )
    .bind(&risk)
    .fetch_one(&setup.pool)
    .await
    .expect("low option");
    let filter = serde_urlencoded::to_string([(
        "fieldFilters",
        json!([{"field": risk, "op": "is", "values": [low]}]).to_string(),
    )])
    .expect("query");
    let (status, page) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items?{filter}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 1, "{page}");

    // A field filter passed with a saved view replaces the view's filter on
    // that field, rather than adding to it.
    let high = sqlx::query_scalar::<_, String>(
        "SELECT option ->> 'id' FROM field_definitions, jsonb_array_elements(config -> 'options') option \
         WHERE id = $1::uuid AND option ->> 'name' = 'High'",
    )
    .bind(&risk)
    .fetch_one(&setup.pool)
    .await
    .expect("high option");
    let (status, view) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/views"),
            Some("high-risk-view"),
            json!({"name": "High risk", "shared": true, "query": {
                "fieldFilters": json!([{"field": risk, "op": "is", "values": [high]}]).to_string(),
            }}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    for (arguments, expected) in [
        (json!({"projectId": "WORK", "view": "High risk"}), vec![]),
        (
            json!({"projectId": "WORK", "view": "High risk",
                   "fieldFilters": [{"field": "Risk", "op": "is", "values": ["Low"]}]}),
            vec!["WORK-2"],
        ),
    ] {
        let listed = agent("task_list", arguments.clone()).await;
        assert_eq!(listed["outcome"], "ok", "{arguments}: {listed}");
        let keys = listed["value"]["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .map(|task| task["key"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(keys, expected, "{arguments}: {listed}");
    }
    let (status, groups) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items/groups?groupBy=field:{risk}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{groups}");
    let labels = groups["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .map(|group| {
            (
                group["label"].as_str().unwrap_or_default().to_owned(),
                group["count"].clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            ("Low".to_owned(), json!(1)),
            ("No value".to_owned(), json!(1))
        ]
    );
    let (status, sorted) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items?sort=field:{risk}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sorted}");
    assert_eq!(sorted["items"][0]["key"], "WORK-2", "{sorted}");
    let side_field = owner(
        "field_write",
        json!({"projectId": "SIDE", "name": "Size", "type": "number"}),
    )
    .await;
    let side_field = side_field["value"]["field"]["id"]
        .as_str()
        .expect("side field");
    for query in [
        format!("sort=field:{side_field}"),
        format!("groupBy=field:{side_field}"),
        serde_urlencoded::to_string([(
            "fieldFilters",
            json!([{"field": side_field, "op": "notEmpty"}]).to_string(),
        )])
        .expect("query"),
    ] {
        let (status, body) = send(
            app,
            setup.browser.request(
                "GET",
                &format!("/api/v1/projects/{work}/work-items?{query}"),
                None,
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }

    // Archiving hides a field and keeps its values; restoring brings them back.
    let archived = owner(
        "field_write",
        json!({"projectId": "WORK", "field": "Risk", "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let opened = agent("task_open", json!({"taskId": "WORK-2"})).await;
    assert!(
        !field_values(&opened["value"]["fields"])
            .iter()
            .any(|(name, _)| name == "Risk"),
        "{opened}"
    );
    let version = opened["value"]["task"]["version"]
        .as_i64()
        .expect("version");
    let refused = agent(
        "task_update",
        json!({"taskId": "WORK-2", "expectedVersion": version, "fields": {"Risk": "High"}}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let restored = owner(
        "field_write",
        json!({"projectId": "WORK", "field": "Risk", "archived": false}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    let opened = agent("task_open", json!({"taskId": "WORK-2"})).await;
    assert!(
        field_values(&opened["value"]["fields"]).contains(&("Risk".to_owned(), json!("Low"))),
        "{opened}"
    );
    let _ = side;
}

async fn task_version(pool: &sqlx::PgPool, task_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1::uuid")
        .bind(task_id)
        .fetch_one(pool)
        .await
        .expect("version")
}

/// AWD-74's roles decide who defines fields and who sets values: defining
/// is "Change the workflow" (owners and admins), setting values is "Edit
/// tasks", and an app or agent never goes past its person's agent limit.
#[tokio::test]
async fn defining_fields_follows_the_workflow_permission_and_values_follow_task_edits() {
    let Some(setup) = setup("awd89_roles").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("roles-task"),
            json!({"title": "Roles"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().expect("task").to_owned();
    let task_path = format!("/api/v1/projects/{work}/work-items/{task_id}");
    let fields_path = format!("/api/v1/projects/{work}/fields");

    // Alex is an admin of WORK: Alex defines fields.
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    sqlx::query("UPDATE project_memberships SET role = 'admin' WHERE actor_id = $1")
        .bind(alex)
        .execute(&setup.pool)
        .await
        .expect("make Alex a project admin");
    let (status, risk) = send(
        app,
        alex_browser.request(
            "POST",
            &fields_path,
            Some("alex-risk"),
            json!({"name": "Risk", "type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{risk}");
    let risk_id = risk["id"].as_str().expect("risk").to_owned();

    // Bea holds a narrowed role that sees tasks but doesn't edit them: she
    // reads the fields, and can neither define them nor set a value.
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (status, reader) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{}/roles", setup.organization_id),
            None,
            json!({"name": "Reader", "baseRole": "member", "permissions": ["tasks.view"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{reader}");
    let membership_version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("membership version");
    let (status, assigned) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/role",
                setup.organization_id
            ),
            None,
            json!({"roleId": reader["id"], "applyToProjects": true, "expectedVersion": membership_version}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {assigned}");
    let (status, listed) = send(
        app,
        bea_browser.request("GET", &fields_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["canManage"], false);
    assert_eq!(listed["fields"][0]["name"], "Risk");
    let (status, refused) = send(
        app,
        bea_browser.request(
            "POST",
            &fields_path,
            Some("bea-field"),
            json!({"name": "Mine", "type": "text"}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {refused}");
    let (status, refused) = send(
        app,
        bea_browser.request(
            "PATCH",
            &task_path,
            Some("bea-value"),
            json!({"expectedVersion": task_version(&setup.pool, &task_id).await, "fieldValues": {risk_id.clone(): "High"}}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {refused}");

    // Alex's app may only see and edit tasks: it sets values, but field
    // definitions are neither offered nor accepted through it.
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{alex}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["tasks.view", "tasks.edit"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let alex_app = connect_app(app, &alex_browser, "Alex's app", "full").await;
    let session = initialize(app, &alex_app).await.expect("Alex's app");
    let listed = tool(
        app,
        &alex_app,
        &session,
        "field_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["canManage"], false, "{listed}");
    let refused = tool(
        app,
        &alex_app,
        &session,
        "field_write",
        json!({"projectId": "WORK", "name": "Through the app", "type": "text"}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let set = tool(
        app,
        &alex_app,
        &session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": task_version(&setup.pool, &task_id).await, "fields": {"Risk": "High"}}),
    )
    .await;
    assert_eq!(set["outcome"], "applied", "{set}");

    // Only Alex's value landed, and nothing Bea or the app tried defined a field.
    let (values, count) = sqlx::query_as::<_, (Value, i64)>(
        "SELECT item.field_values, (SELECT count(*) FROM field_definitions WHERE project_id = item.project_id) \
         FROM work_items item WHERE item.id = $1::uuid",
    )
    .bind(&task_id)
    .fetch_one(&setup.pool)
    .await
    .expect("values");
    assert_eq!(count, 1);
    let high = risk["options"][1]["id"].as_str().expect("High");
    assert_eq!(values[&risk_id], high);

    // Cam keeps the workflow without editing tasks: defining fields is the
    // workflow's permission, so the page offers it to her and the server
    // takes it, and she still can't set a value.
    let (cam, cam_browser) = add_person(&setup, &[&work], "cam").await;
    let (status, keeper) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{}/roles", setup.organization_id),
            None,
            json!({"name": "Workflow keeper", "baseRole": "admin", "permissions": ["tasks.view", "tasks.workflow"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{keeper}");
    let membership_version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(cam)
    .fetch_one(&setup.pool)
    .await
    .expect("membership version");
    let (status, assigned) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{cam}/role",
                setup.organization_id
            ),
            None,
            json!({"roleId": keeper["id"], "applyToProjects": true, "expectedVersion": membership_version}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {assigned}");
    let (status, listed) = send(
        app,
        cam_browser.request("GET", &fields_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["canManage"], true, "{listed}");
    let (status, made) = send(
        app,
        cam_browser.request(
            "POST",
            &fields_path,
            Some("cam-field"),
            json!({"name": "Area", "type": "text"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let (status, refused) = send(
        app,
        cam_browser.request(
            "PATCH",
            &task_path,
            Some("cam-value"),
            json!({"expectedVersion": task_version(&setup.pool, &task_id).await, "fieldValues": {risk_id.clone(): "Low"}}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {refused}");
}

/// Tiny numbers are short in JSON but PostgreSQL writes them out in full
/// (1e-300 is about 300 bytes): twenty of them changing together still save,
/// and the stored history stays under work_item_activity's 8 KB check.
#[tokio::test]
async fn tiny_numbers_changing_together_keep_history_under_its_limit() {
    let Some(setup) = setup("awd89_tiny_numbers").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let fields_path = format!("/api/v1/projects/{work}/fields");
    let mut ids = Vec::new();
    for number in 0..20 {
        let (status, field) = send(
            app,
            setup.browser.request(
                "POST",
                &fields_path,
                Some(&format!("tiny-field-{number}")),
                json!({"name": format!("N{number}"), "type": "number"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{field}");
        ids.push(field["id"].as_str().expect("field").to_owned());
    }
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("tiny-task"),
            json!({"title": "Tiny numbers"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().expect("task").to_owned();
    let task_path = format!("/api/v1/projects/{work}/work-items/{task_id}");
    for (step, number) in [1e-300, 2e-300].into_iter().enumerate() {
        let values = ids
            .iter()
            .map(|id| (id.clone(), json!(number)))
            .collect::<serde_json::Map<_, _>>();
        let (status, body) = send(
            app,
            setup.browser.request(
                "PATCH",
                &task_path,
                Some(&format!("tiny-values-{step}")),
                json!({"expectedVersion": task_version(&setup.pool, &task_id).await, "fieldValues": values}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "step {step}: {body}");
    }
    let (updates, largest) = sqlx::query_as::<_, (i64, i32)>(
        "SELECT count(*), max(octet_length(payload::text)) FROM work_item_activity \
         WHERE work_item_id = $1::uuid AND kind = 'work_item_updated'",
    )
    .bind(&task_id)
    .fetch_one(&setup.pool)
    .await
    .expect("history");
    assert_eq!(updates, 2);
    assert!(largest <= 8192, "{largest} bytes");
}

/// "is" and "is not" compare a number field by value, however the number is
/// written (1e-7, 0.00000010, 12.0, 0012) and however PostgreSQL writes it
/// back (1e-7 as 0.0000001); a text field's "0012" stays the text it is.
#[tokio::test]
async fn number_filters_compare_by_value_and_text_filters_as_written() {
    let Some(setup) = setup("awd89_numbers").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let fields_path = format!("/api/v1/projects/{work}/fields");
    let mut ids = Vec::new();
    for (name, kind) in [("Size", "number"), ("Code", "text")] {
        let (status, field) = send(
            app,
            setup.browser.request(
                "POST",
                &fields_path,
                Some(&format!("numbers-field-{name}")),
                json!({"name": name, "type": kind}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{field}");
        ids.push(field["id"].as_str().expect("field").to_owned());
    }
    let (size, code) = (ids[0].clone(), ids[1].clone());
    // WORK-1: Size 1e-7. WORK-2: Size 12. WORK-3: Code "0012". WORK-4: Code "12".
    for (index, (field, value)) in [
        (&size, json!(1e-7)),
        (&size, json!(12)),
        (&code, json!("0012")),
        (&code, json!("12")),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, task) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{work}/work-items"),
                Some(&format!("numbers-task-{index}")),
                json!({"title": format!("Task {index}"), "fieldValues": {field.clone(): value}}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
    }
    for (field, op, values, expected) in [
        (&size, "is", json!(["1e-7"]), vec!["WORK-1"]),
        (&size, "is", json!(["0.00000010"]), vec!["WORK-1"]),
        (&size, "is", json!(["0012"]), vec!["WORK-2"]),
        (
            &size,
            "is",
            json!(["12.0", "1e-7"]),
            vec!["WORK-1", "WORK-2"],
        ),
        (
            &size,
            "isNot",
            json!(["1E-7"]),
            vec!["WORK-2", "WORK-3", "WORK-4"],
        ),
        (&code, "is", json!(["0012"]), vec!["WORK-3"]),
        (&code, "is", json!(["12"]), vec!["WORK-4"]),
        (
            &code,
            "isNot",
            json!(["12"]),
            vec!["WORK-1", "WORK-2", "WORK-3"],
        ),
    ] {
        let filter = serde_urlencoded::to_string([(
            "fieldFilters",
            json!([{"field": field, "op": op, "values": values}]).to_string(),
        )])
        .expect("query");
        let (status, page) = send(
            app,
            setup.browser.request(
                "GET",
                &format!("/api/v1/projects/{work}/work-items?{filter}"),
                None,
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{op} {values}: {page}");
        let mut keys = page["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["key"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, expected, "{op} {values}: {page}");
    }
}

/// The largest choice field the app accepts saves: 100 options, each name
/// 60 characters of four-byte text, well past the 16 KB the config column
/// once allowed.
#[tokio::test]
async fn the_largest_choice_field_saves() {
    let Some(setup) = setup("awd89_large_config").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let options = (0..100)
        .map(|index| json!({"name": format!("{index:02}{}", "𝄞".repeat(58))}))
        .collect::<Vec<_>>();
    let (status, field) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/fields"),
            Some("large-config"),
            json!({"name": "Areas", "type": "multiple_choice", "options": options}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{field}");
    assert_eq!(field["options"].as_array().map(Vec::len), Some(100));
    let stored = sqlx::query_scalar::<_, i32>(
        "SELECT octet_length(config::text) FROM field_definitions WHERE id = $1::uuid",
    )
    .bind(field["id"].as_str().expect("field"))
    .fetch_one(&setup.pool)
    .await
    .expect("config");
    assert!(stored > 16_384, "{stored} bytes");
}

/// A task's values are measured as PostgreSQL stores them: long text with
/// tiny numbers (short in JSON, about 300 bytes each when stored) is
/// refused as too large, with the task untouched, rather than failing the
/// column's 64 KB check.
#[tokio::test]
async fn values_too_large_as_stored_are_refused_not_failed() {
    let Some(setup) = setup("awd89_values_size").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let fields_path = format!("/api/v1/projects/{work}/fields");
    let mut values = serde_json::Map::new();
    for index in 0..50 {
        let (kind, value) = if index < 3 {
            ("text", json!("𝄞".repeat(4_600)))
        } else {
            ("number", json!(1e-300))
        };
        let (status, field) = send(
            app,
            setup.browser.request(
                "POST",
                &fields_path,
                Some(&format!("size-field-{index}")),
                json!({"name": format!("F{index}"), "type": kind}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{field}");
        values.insert(field["id"].as_str().expect("field").to_owned(), value);
    }
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("size-task"),
            json!({"title": "Big values"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().expect("task").to_owned();
    let before = task_version(&setup.pool, &task_id).await;
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/work-items/{task_id}"),
            Some("size-values"),
            json!({"expectedVersion": before, "fieldValues": values}),
        ),
    )
    .await;
    // The size refusal itself, not some other validation failure.
    let too_large = |status: StatusCode, body: &Value| {
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], "validation_failed", "{body}");
        assert_eq!(
            body["error"]["message"],
            agent_workspace_server::work_item_fields::TOO_LARGE,
            "{body}"
        );
    };
    too_large(status, &body);
    let (version, stored) = sqlx::query_as::<_, (i64, Value)>(
        "SELECT version, field_values FROM work_items WHERE id = $1::uuid",
    )
    .bind(&task_id)
    .fetch_one(&setup.pool)
    .await
    .expect("values");
    assert_eq!((version, stored), (before, json!({})));

    // A new task with the same values is refused the same way, and isn't made.
    let (status, body) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("size-create"),
            json!({"title": "Too big from the start", "fieldValues": values}),
        ),
    )
    .await;
    too_large(status, &body);
    let made = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_items WHERE project_id = $1::uuid AND title = 'Too big from the start'",
    )
    .bind(&work)
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(made, 0);
}

/// A long title, a long description and eight custom fields changing in one
/// update still save, and their history fits work_item_activity's 8 KB:
/// the custom changes give way first, so the update isn't rolled back.
#[tokio::test]
async fn a_large_change_keeps_its_history_within_the_payload_limit() {
    let Some(setup) = setup("awd89_whole_payload").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let mut ids = Vec::new();
    for index in 0..8 {
        let (status, field) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{work}/fields"),
                Some(&format!("payload-field-{index}")),
                json!({"name": format!("F{index}"), "type": "text"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{field}");
        ids.push(field["id"].as_str().expect("field").to_owned());
    }
    let values = |text: &str| {
        ids.iter()
            .map(|id| (id.clone(), json!(text.repeat(80))))
            .collect::<serde_json::Map<_, _>>()
    };
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("payload-task"),
            json!({"title": "😀".repeat(240), "description": "a".repeat(2_248), "fieldValues": values("😀")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().expect("task").to_owned();
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/work-items/{task_id}"),
            Some("payload-update"),
            json!({"expectedVersion": task_version(&setup.pool, &task_id).await,
                   "title": "😁".repeat(240), "description": "b".repeat(2_248),
                   "fieldValues": values("😁")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (updates, largest, title) = sqlx::query_as::<_, (i64, i32, Option<String>)>(
        "SELECT count(*), max(octet_length(payload::text)), \
                max(payload -> 'changes' -> 'title' ->> 'to') FROM work_item_activity \
         WHERE work_item_id = $1::uuid AND kind = 'work_item_updated'",
    )
    .bind(&task_id)
    .fetch_one(&setup.pool)
    .await
    .expect("history");
    assert_eq!(updates, 1);
    assert!(largest <= 8192, "{largest} bytes");
    assert_eq!(title.as_deref(), Some("😁".repeat(240).as_str()));
}

/// AWD-101: the bulk tools and the web's bulk route set custom fields on
/// every task or none, typed and checked as a single edit's are, with one
/// history entry per task; someone who can only read tasks can't.
#[tokio::test]
async fn bulk_tools_set_fields_on_every_task_or_none() {
    let Some(setup) = setup("awd101_bulk_fields").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let owner = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    for (name, extra) in [
        (
            "Risk",
            json!({"type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]}),
        ),
        ("Points", json!({"type": "number"})),
    ] {
        let mut arguments = json!({"projectId": "WORK", "name": name});
        for (key, value) in extra.as_object().expect("extra") {
            arguments[key] = value.clone();
        }
        let written = owner("field_write", arguments).await;
        assert_eq!(written["outcome"], "ok", "{written}");
    }
    let field = |name: &'static str| {
        let pool = setup.pool.clone();
        let work = work.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT id::text FROM field_definitions WHERE project_id = $1::uuid AND name = $2",
            )
            .bind(&work)
            .bind(name)
            .fetch_one(&pool)
            .await
            .expect("field")
        }
    };
    let (risk, points) = (field("Risk").await, field("Points").await);
    let option = |name: &'static str| {
        let pool = setup.pool.clone();
        let risk = risk.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT option ->> 'id' FROM field_definitions, jsonb_array_elements(config -> 'options') option \
                 WHERE id = $1::uuid AND option ->> 'name' = $2",
            )
            .bind(&risk)
            .bind(name)
            .fetch_one(&pool)
            .await
            .expect("option")
        }
    };
    let (low, high) = (option("Low").await, option("High").await);
    let values = || {
        let pool = setup.pool.clone();
        let work = work.clone();
        async move {
            sqlx::query_as::<_, (i64, i64, Value)>(
                "SELECT number, version, field_values FROM work_items \
                 WHERE project_id = $1::uuid ORDER BY number",
            )
            .bind(&work)
            .fetch_all(&pool)
            .await
            .expect("tasks")
        }
    };

    // task_create_many: each task's own fields, typed; one bad value and
    // none of the batch is made.
    let refused = owner(
        "task_create_many",
        json!({"projectId": "WORK", "clientRequestId": "bulk-make-bad", "tasks": [
            {"title": "Four", "fields": {"Risk": "Low"}},
            {"title": "Five", "fields": {"Risk": "Nope"}},
        ]}),
    )
    .await;
    assert_eq!(refused["outcome"], "rejected", "{refused}");
    assert!(values().await.is_empty());
    let made = owner(
        "task_create_many",
        json!({"projectId": "WORK", "clientRequestId": "bulk-make", "tasks": [
            {"title": "One", "fields": {"Risk": "Low"}},
            {"title": "Two", "fields": {"Points": "2"}},
            {"title": "Three"},
        ]}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let tasks = values().await;
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].2[&risk], json!(low));
    assert_eq!(tasks[1].2[&points], json!(2));
    assert_eq!(tasks[2].2, json!({}));

    // task_update_many: the same fields on every task, all or nothing.
    let versions = tasks.iter().map(|task| task.1).collect::<Vec<_>>();
    let refused = owner(
        "task_update_many",
        json!({"projectId": "WORK", "taskIds": ["WORK-1", "WORK-2", "WORK-3"],
               "fields": {"Risk": "High", "Points": "lots"}, "clientRequestId": "bulk-set-bad"}),
    )
    .await;
    assert_eq!(refused["outcome"], "rejected", "{refused}");
    assert_eq!(
        values().await.iter().map(|task| task.1).collect::<Vec<_>>(),
        versions,
        "nothing changed"
    );
    let updated = owner(
        "task_update_many",
        json!({"projectId": "WORK", "taskIds": ["WORK-1", "WORK-2", "WORK-3"],
               "fields": {"Risk": "High", "Points": 5}, "clientRequestId": "bulk-set"}),
    )
    .await;
    assert_eq!(updated["outcome"], "ok", "{updated}");
    assert_eq!(updated["value"]["changedCount"], 3, "{updated}");
    for task in values().await {
        assert_eq!(task.2[&risk], json!(high), "WORK-{}", task.0);
        assert_eq!(task.2[&points], json!(5), "WORK-{}", task.0);
    }
    // One history entry per task, with the field's before and after.
    let entries = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_activity \
         WHERE project_id = $1::uuid AND kind = 'work_item_updated' AND payload -> 'changes' ? $2",
    )
    .bind(&work)
    .bind(format!("field:{risk}"))
    .fetch_one(&setup.pool)
    .await
    .expect("history");
    assert_eq!(entries, 3);

    // The web's bulk route: clear a field on two tasks; a bad value changes none.
    let tasks = values().await;
    let items = |tasks: &[(i64, i64, Value)]| -> Value {
        let ids = ["WORK-1", "WORK-2"];
        Value::Array(
            tasks
                .iter()
                .filter(|task| ids.contains(&format!("WORK-{}", task.0).as_str()))
                .map(|task| json!({"number": task.0, "expectedVersion": task.1}))
                .collect(),
        )
    };
    let ids = sqlx::query_as::<_, (i64, String)>(
        "SELECT number, id::text FROM work_items WHERE project_id = $1::uuid ORDER BY number",
    )
    .bind(&work)
    .fetch_all(&setup.pool)
    .await
    .expect("ids");
    let bulk_items = items(&tasks)
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            let id = &ids
                .iter()
                .find(|(number, _)| json!(number) == item["number"])
                .expect("id")
                .1;
            json!({"id": id, "expectedVersion": item["expectedVersion"]})
        })
        .collect::<Vec<_>>();
    let bulk = |key: &'static str, changes: Value| {
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items/bulk"),
            Some(key),
            json!({"items": bulk_items, "changes": changes}),
        )
    };
    let (status, body) = send(
        app,
        bulk("bulk-bad", json!({"fieldValues": {risk.clone(): "nope"}})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(values().await, tasks, "nothing changed");
    let (status, body) = send(
        app,
        bulk("bulk-clear", json!({"fieldValues": {risk.clone(): null}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = values().await;
    assert!(after[0].2.get(&risk).is_none() && after[1].2.get(&risk).is_none());
    assert_eq!(after[2].2[&risk], json!(high), "WORK-3 wasn't selected");

    // Someone who only reads tasks can't set fields in bulk.
    let (pat, pat_browser) = add_person(&setup, &[&work], "pat").await;
    sqlx::query("UPDATE project_memberships SET role = 'viewer' WHERE actor_id = $1")
        .bind(pat)
        .execute(&setup.pool)
        .await
        .expect("make Pat a viewer");
    let after_items = after
        .iter()
        .take(1)
        .map(|task| {
            json!({"id": ids.iter().find(|(number, _)| *number == task.0).expect("id").1, "expectedVersion": task.1})
        })
        .collect::<Vec<_>>();
    let (status, body) = send(
        app,
        pat_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items/bulk"),
            Some("bulk-viewer"),
            json!({"items": after_items, "changes": {"fieldValues": {risk.clone(): low}}}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {body}");
    assert_eq!(values().await, after, "nothing changed");
}

/// AWD-89: an owner renames and reorders fields, on the web and through
/// MCP; values stay with the field, and someone who can't change the
/// workflow can't do either.
#[tokio::test]
async fn fields_are_renamed_and_reordered() {
    let Some(setup) = setup("awd89_rename_reorder").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let fields_path = format!("/api/v1/projects/{work}/fields");
    let mut ids = Vec::new();
    for name in ["Alpha", "Beta", "Gamma"] {
        let (status, field) = send(
            app,
            setup.browser.request(
                "POST",
                &fields_path,
                Some(&format!("reorder-{name}")),
                json!({"name": name, "type": "text"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{field}");
        ids.push(field["id"].as_str().expect("field").to_owned());
    }
    let fields_of = |status: StatusCode, body: Value| {
        assert_eq!(status, StatusCode::OK, "{body}");
        body["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .map(|field| {
                (
                    field["id"].as_str().unwrap_or_default().to_owned(),
                    field["name"].as_str().unwrap_or_default().to_owned(),
                    field["version"].as_i64().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>()
    };
    let browser = &setup.browser;
    let listed = || {
        let request = browser.request("GET", &fields_path, None, Value::Null);
        async move {
            let (status, body) = send(app, request).await;
            fields_of(status, body)
        }
    };
    let names = |fields: &[(String, String, i64)]| {
        fields
            .iter()
            .map(|field| field.1.clone())
            .collect::<Vec<_>>()
    };
    let version_of = |fields: &[(String, String, i64)], id: &str| {
        fields.iter().find(|field| field.0 == id).expect("field").2
    };

    // A value set on Alpha stays with it through a rename.
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("reorder-task"),
            json!({"title": "Keeps its value", "fieldValues": {ids[0].clone(): "kept"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().expect("task").to_owned();

    // The web: move Gamma to the top, then rename Alpha.
    let fields = listed().await;
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("{fields_path}/{}", ids[2]),
            Some("reorder-gamma"),
            json!({"expectedVersion": version_of(&fields, &ids[2]), "position": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(names(&listed().await), ["Gamma", "Alpha", "Beta"]);
    let fields = listed().await;
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("{fields_path}/{}", ids[0]),
            Some("rename-alpha"),
            json!({"expectedVersion": version_of(&fields, &ids[0]), "name": "First"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(names(&listed().await), ["Gamma", "First", "Beta"]);

    // MCP: rename Beta and move it to the top.
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let renamed = tool(
        app,
        &codex,
        &session,
        "field_write",
        json!({"projectId": "WORK", "field": "Beta", "name": "Second", "position": 1}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    assert_eq!(names(&listed().await), ["Second", "Gamma", "First"]);
    let listed_by_agent = tool(
        app,
        &codex,
        &session,
        "field_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    let agent_names = listed_by_agent["value"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .map(|field| field["name"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(agent_names, ["Second", "Gamma", "First"]);

    // The value stayed with its field, under the new name.
    let stored =
        sqlx::query_scalar::<_, Value>("SELECT field_values FROM work_items WHERE id = $1::uuid")
            .bind(&task_id)
            .fetch_one(&setup.pool)
            .await
            .expect("values");
    assert_eq!(stored[&ids[0]], json!("kept"));

    // A member can do neither.
    let (_, member_browser) = add_person(&setup, &[&work], "pat").await;
    let fields = listed().await;
    for (key, change) in [
        ("member-rename", json!({"name": "Mine"})),
        ("member-reorder", json!({"position": 1})),
    ] {
        let mut body = change;
        body["expectedVersion"] = json!(version_of(&fields, &ids[1]));
        let (status, refused) = send(
            app,
            member_browser.request(
                "PATCH",
                &format!("{fields_path}/{}", ids[1]),
                Some(key),
                body,
            ),
        )
        .await;
        assert!(status.is_client_error(), "{status}: {refused}");
    }
    assert_eq!(names(&listed().await), ["Second", "Gamma", "First"]);
}

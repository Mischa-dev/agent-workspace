//! AWD-74: roles and access. The silent, costly failures: a built-in role
//! that quietly means something else, someone handing out more than they
//! hold (or an admin role without being an owner), a workspace left without
//! an owner, and an app or agent acting for a person doing more than that
//! person's agent limit allows, through MCP or the public API.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::{add_person, create_channel},
};

fn roles_path(setup: &Setup) -> String {
    format!("/api/v1/organizations/{}/roles", setup.organization_id)
}

async fn roles(setup: &Setup, browser: &Browser) -> Value {
    let (status, body) = send(
        &setup.app,
        browser.request("GET", &roles_path(setup), None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn role<'a>(listed: &'a Value, key: &str) -> &'a Value {
    listed["roles"]
        .as_array()
        .expect("roles")
        .iter()
        .find(|role| role["key"] == key)
        .unwrap_or_else(|| panic!("{key} in {listed}"))
}

async fn create_role(setup: &Setup, browser: &Browser, body: Value) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request("POST", &roles_path(setup), None, body),
    )
    .await
}

async fn assign(
    setup: &Setup,
    browser: &Browser,
    actor: Uuid,
    role_id: &Value,
    apply: bool,
) -> (StatusCode, Value) {
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("membership version");
    send(
        &setup.app,
        browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{actor}/role",
                setup.organization_id
            ),
            None,
            json!({"roleId": role_id, "applyToProjects": apply, "expectedVersion": version}),
        ),
    )
    .await
}

async fn make_admin(setup: &Setup, actor: Uuid) {
    sqlx::query(
        "UPDATE organization_memberships SET role = 'admin', role_explicit = true WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(actor)
    .execute(&setup.pool)
    .await
    .expect("make admin");
}

fn keys(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("a list: {value}"))
        .iter()
        .map(|key| key.as_str().expect("key").to_owned())
        .collect()
}

#[tokio::test]
async fn built_in_roles_stay_as_they_were_and_nobody_grants_more_than_they_hold() {
    let Some(setup) = setup("awd74_roles").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let (bea, _) = add_person(&setup, &[&work], "bea").await;
    make_admin(&setup, alex).await;

    // The four built-in roles are there, mean what they always did, and
    // Member is where new people start.
    let listed = roles(&setup, &setup.browser).await;
    let catalogue = listed["permissions"].as_array().expect("catalogue");
    for key in ["owner", "admin", "member", "viewer"] {
        let built_in = role(&listed, key);
        assert_eq!(built_in["builtIn"], true);
        let expected = catalogue
            .iter()
            .filter(|permission| {
                permission["builtIn"]
                    .as_array()
                    .is_some_and(|roles| roles.contains(&json!(key)))
            })
            .map(|permission| permission["key"].as_str().expect("key").to_owned())
            .collect::<Vec<_>>();
        assert_eq!(keys(&built_in["permissions"]), expected, "{key}");
    }
    assert_eq!(role(&listed, "member")["isDefault"], true);
    assert_eq!(role(&listed, "owner")["workspaceHolders"], 1);
    assert!(
        keys(&role(&listed, "viewer")["permissions"])
            .iter()
            .all(|key| key.ends_with(".view")),
        "viewers only read"
    );

    // A custom role keeps a subset of its base, and always lets holders
    // open what they're in.
    let (status, contractor) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Contractor", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit", "channels.post"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{contractor}");
    assert_eq!(
        keys(&contractor["permissions"]),
        ["projects.view", "tasks.view", "tasks.edit", "channels.post"]
    );
    let (status, beyond) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Too much", "baseRole": "member", "permissions": ["code.manage"]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{beyond}");

    // Built-in roles keep their permissions; their words can change.
    let member = role(&listed, "member").clone();
    let member_path = format!(
        "{}/{}",
        roles_path(&setup),
        member["id"].as_str().expect("id")
    );
    let (status, refused) = send(
        app,
        setup.browser.request(
            "PATCH",
            &member_path,
            None,
            json!({"expectedVersion": member["version"], "permissions": ["tasks.view"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, renamed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &member_path,
            None,
            json!({"expectedVersion": member["version"], "description": "Does the work."}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["permissions"], member["permissions"]);
    let owner_role = role(&listed, "owner").clone();
    let (status, refused) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!(
                "{}/{}",
                roles_path(&setup),
                owner_role["id"].as_str().expect("id")
            ),
            None,
            json!({"expectedVersion": owner_role["version"], "agentLimit": ["tasks.view"]}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "owners' apps aren't limited: {refused}"
    );
    let (status, _) = send(
        app,
        setup.browser.request(
            "DELETE",
            &format!(
                "{}/{}",
                roles_path(&setup),
                owner_role["id"].as_str().expect("id")
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "built-in roles stay");

    // Admins make roles from Member or Viewer, never from Admin.
    let (status, refused) = create_role(
        &setup,
        &alex_browser,
        json!({"name": "Deputy", "baseRole": "admin", "permissions": ["members.manage"]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, reviewer) = create_role(
        &setup,
        &alex_browser,
        json!({"name": "Reviewer", "baseRole": "viewer", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{reviewer}");
    // An admin holding a narrowed role can't hand out what it took away.
    let (status, narrowed) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Ops", "baseRole": "admin", "permissions": ["roles.manage", "members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{narrowed}");
    let (status, assigned) = assign(&setup, &setup.browser, alex, &narrowed["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let (status, refused) = create_role(
        &setup,
        &alex_browser,
        json!({"name": "Builders", "baseRole": "member", "permissions": ["code.publish"]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, refused) = assign(&setup, &alex_browser, bea, &contractor["id"], false).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Contractor holds tasks.edit, which Ops doesn't: {refused}"
    );
    let (status, assigned) = assign(
        &setup,
        &setup.browser,
        alex,
        &role(&listed, "admin")["id"],
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");

    // Admins assign member-based roles to members, and nothing else.
    let (status, assigned) = assign(&setup, &alex_browser, bea, &contractor["id"], true).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(assigned["projectsChanged"], 1, "{assigned}");
    let held = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT role_id FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("project role");
    assert_eq!(
        held.map(|id| id.to_string()),
        contractor["id"].as_str().map(str::to_owned),
        "applied to their project"
    );
    let (status, refused) = assign(
        &setup,
        &alex_browser,
        bea,
        &role(&listed, "admin")["id"],
        false,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "only owners make admins: {refused}"
    );
    let owner_actor =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&setup.pool)
            .await
            .expect("owner");
    let (status, refused) =
        assign(&setup, &alex_browser, owner_actor, &contractor["id"], false).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "owners keep ownership: {refused}"
    );
    let (status, refused) = assign(
        &setup,
        &setup.browser,
        owner_actor,
        &contractor["id"],
        false,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "nobody changes their own role: {refused}"
    );

    // The last owner can't leave, so a workspace always keeps one.
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{}/leave", setup.organization_id),
            None,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let owners = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' AND enabled",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owners");
    assert_eq!(owners, 1);

    // A role somebody holds can't go; once nobody does, it can.
    let contractor_path = format!(
        "{}/{}",
        roles_path(&setup),
        contractor["id"].as_str().expect("id")
    );
    let (status, _) = send(
        app,
        setup
            .browser
            .request("DELETE", &contractor_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = assign(
        &setup,
        &setup.browser,
        bea,
        &role(&listed, "member")["id"],
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        app,
        setup
            .browser
            .request("DELETE", &contractor_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Every change is in the audit log, with what it was before.
    let changed = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE action = 'role.changed' AND organization_id = $1",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("role change audited");
    assert_eq!(changed["before"]["description"], member["description"]);
    assert_eq!(changed["after"]["description"], "Does the work.");
    let assignments = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE action = 'workspace_membership.role_changed' AND target_id = $1 ORDER BY occurred_at",
    )
    .bind(bea)
    .fetch_all(&setup.pool)
    .await
    .expect("assignments audited");
    assert_eq!(assignments.len(), 2, "{assignments:?}");
    assert_eq!(assignments[0]["previousRole"], "member");
    assert_eq!(assignments[0]["roleName"], "Contractor");
    for action in ["role.created", "role.removed"] {
        let found = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE action = $1 AND organization_id = $2",
        )
        .bind(action)
        .bind(setup.organization_id)
        .fetch_one(&setup.pool)
        .await
        .expect("audit");
        assert!(found >= 1, "{action}");
    }
}

/// A personal API token's call to the public API.
async fn public_call(
    app: &axum::Router,
    token: &str,
    tool: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/public/v1/{tool}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("JSON")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn listed_tools(app: &axum::Router, token: &str, session: &str) -> Vec<String> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::HOST, "127.0.0.1")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", "2025-06-18")
                .body(Body::from(
                    serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"}))
                        .expect("JSON"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text);
    let value: Value =
        serde_json::from_str(json).unwrap_or_else(|error| panic!("tools/list: {error}: {text}"));
    value["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name").to_owned())
        .collect()
}

#[tokio::test]
async fn apps_and_agents_do_only_what_their_role_and_limit_allow_everywhere() {
    let Some(setup) = setup("awd74_limits").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let listed = roles(&setup, &setup.browser).await;

    // Bea's apps may only read and comment, though Bea herself may do more.
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["tasks.view", "channels.post"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");

    // In the web app, Bea creates tasks as before.
    let (status, made) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("bea-web-task"),
            json!({"title": "Made in the browser"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");

    // Through her app the tool isn't offered, and a call is refused.
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let tools = listed_tools(app, &bea_app, &session).await;
    assert!(!tools.contains(&"task_create".to_owned()), "{tools:?}");
    assert!(tools.contains(&"task_list".to_owned()), "{tools:?}");
    assert!(tools.contains(&"my_permissions".to_owned()), "{tools:?}");
    let refused = tool(
        app,
        &bea_app,
        &session,
        "task_create",
        json!({"projectId": work, "title": "Made by the app"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let mine = tool(
        app,
        &bea_app,
        &session,
        "my_permissions",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(mine["outcome"], "ok", "{mine}");
    assert_eq!(mine["value"]["actingAs"], "person");
    let may = keys(&mine["value"]["project"]["thisConnectionMay"]);
    assert_eq!(
        may,
        ["projects.view", "tasks.view", "channels.post"],
        "{mine}"
    );
    assert!(keys(&mine["value"]["project"]["permissions"]).contains(&"tasks.create".to_owned()));

    // The limit holds inside tools that check access themselves, too.
    let made_title = sqlx::query_scalar::<_, String>(
        "SELECT id::text FROM work_items WHERE project_id = $1 AND title = 'Made in the browser'",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .fetch_one(&setup.pool)
    .await
    .expect("task");
    let refused = tool(
        app,
        &bea_app,
        &session,
        "task_update",
        json!({"taskId": made_title, "title": "Renamed by the app", "expectedVersion": 1}),
    )
    .await;
    assert_ne!(refused["outcome"], "applied", "{refused}");
    assert_ne!(refused["outcome"], "ok", "{refused}");

    // The public API is an app acting for her too.
    let (status, issued) = send(
        app,
        bea_browser.request(
            "POST",
            "/api/v1/account/api-tokens",
            Some("bea-token"),
            json!({"projectId": work, "name": "script", "scopes": ["read", "write"], "expiresInDays": 30}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let token = issued["token"].as_str().expect("token");
    let (status, refused) = public_call(
        app,
        token,
        "task_create",
        json!({"projectId": work, "title": "Made by a script"}),
    )
    .await;
    assert!(
        !status.is_success() || refused["outcome"] == "permission_denied",
        "{status}: {refused}"
    );
    let scripted = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_items WHERE title IN ('Made by the app', 'Made by a script', 'Renamed by the app')",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(scripted, 0, "nothing the limit forbids happened");

    // A role's own limit applies the same way, to everyone holding it.
    let (status, _) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": null}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let member = role(&listed, "member");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("{}/{}", roles_path(&setup), member["id"].as_str().expect("id")),
            None,
            json!({"expectedVersion": member["version"], "agentLimit": ["tasks.view", "tasks.create"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let made = tool(
        app,
        &bea_app,
        &session,
        "task_create",
        json!({"projectId": work, "title": "Allowed again"}),
    )
    .await;
    assert_ne!(made["outcome"], "permission_denied", "{made}");
    let refused = tool(
        app,
        &bea_app,
        &session,
        "message_send",
        json!({"projectId": work, "conversationId": "general", "text": "Hello"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // An agent with its own identity follows the role it holds.
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let (status, readers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Readers", "baseRole": "viewer", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{readers}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(mara)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{mara}"),
            Some("mara-readers"),
            json!({"expectedVersion": version, "roleId": readers["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["role"], "viewer");
    assert_eq!(changed["roleId"], readers["id"]);
    let mara_session = initialize(app, &mara_token).await.expect("Mara");
    let refused = tool(
        app,
        &mara_token,
        &mara_session,
        "task_create",
        json!({"projectId": work, "title": "Made by Mara"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let mine = tool(
        app,
        &mara_token,
        &mara_session,
        "my_permissions",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(mine["value"]["actingAs"], "agent");
    assert_eq!(
        keys(&mine["value"]["project"]["permissions"]),
        ["projects.view", "tasks.view"]
    );

    // Only a person changes roles: never an agent as itself, never a
    // member's app.
    let refused = tool(
        app,
        &mara_token,
        &mara_session,
        "role_write",
        json!({"projectId": work, "name": "Mine", "baseRole": "member", "permissions": ["tasks.edit"]}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let refused = tool(
        app,
        &bea_app,
        &session,
        "role_assign",
        json!({"projectId": work, "person": "@bea", "role": "admin"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    let written = tool(
        app,
        &owner_app,
        &owner_session,
        "role_write",
        json!({"projectId": work, "name": "Helpers", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let given = tool(
        app,
        &owner_app,
        &owner_session,
        "role_assign",
        json!({"projectId": work, "person": "@bea", "role": "Helpers", "applyToProjects": true}),
    )
    .await;
    assert_eq!(given["outcome"], "ok", "{given}");
    assert_eq!(given["value"]["projectsChanged"], 1, "{given}");
    let via_app = sqlx::query_scalar::<_, String>(
        "SELECT channel FROM audit_events WHERE action = 'role.created' AND target_id = $1",
    )
    .bind(
        written["value"]["role"]["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .fetch_one(&setup.pool)
    .await
    .expect("audited");
    assert_eq!(via_app, "mcp");
}

#[tokio::test]
async fn people_arrive_where_their_role_says() {
    let Some(setup) = setup("awd74_defaults").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let leads = create_channel(&setup, &work, "leads").await;
    let secret = create_channel(&setup, &work, "secret").await;
    let leads_id = leads.parse::<Uuid>().expect("channel");
    let secret_id = secret.parse::<Uuid>().expect("channel");
    let owner =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&setup.pool)
            .await
            .expect("owner");
    // Both private; the owner is in leads only.
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = ANY($1)")
        .bind(vec![leads_id, secret_id])
        .execute(&setup.pool)
        .await
        .expect("private");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1, $2, $3, $3)",
    )
    .bind(project)
    .bind(leads_id)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("owner in leads");

    let (status, lead) = create_role(
        &setup,
        &setup.browser,
        json!({
            "name": "Lead",
            "baseRole": "member",
            "permissions": ["tasks.view", "tasks.edit", "channels.post"],
            "defaults": {"channels": ["#Leads", "secret"], "pinnedViews": ["blocked"], "layoutPreset": "kanban", "homeHidden": ["home.capacity"]}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{lead}");
    assert_eq!(lead["defaults"]["channels"], json!(["Leads", "secret"]));
    let (status, refused) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Odd", "baseRole": "member", "permissions": [], "defaults": {"pinnedViews": ["everything"]}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    // Cara signs in through another project first.
    let elsewhere = create_project(&setup, "ELSE", "Elsewhere").await;
    let (cara, _) = add_person(&setup, &[&elsewhere], "cara").await;
    let (status, added) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/members"),
            None,
            json!({"actorId": cara, "role": "member", "roleId": lead["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["roleId"], lead["id"]);
    let joined = sqlx::query_scalar::<_, Uuid>(
        "SELECT conversation_id FROM conversation_members WHERE actor_id = $1",
    )
    .bind(cara)
    .fetch_all(&setup.pool)
    .await
    .expect("channels joined");
    assert_eq!(
        joined,
        [leads_id],
        "joins the private channel the person adding them is in, and not the other"
    );
    let pinned =
        sqlx::query_scalar::<_, Option<Vec<String>>>("SELECT role_default_pinned_views($1, $2)")
            .bind(project)
            .bind(cara)
            .fetch_one(&setup.pool)
            .await
            .expect("pinned views");
    assert_eq!(pinned, Some(vec!["blocked".to_owned()]));
}

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
    w20_files::{read, with_data_root},
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
    // Nor through a project role or an invitation (Ops has no members.invite).
    let project = work.parse::<Uuid>().expect("project");
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("bea views");
    sqlx::query("UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2")
        .bind(project)
        .bind(alex)
        .bind(narrowed["id"].as_str().expect("id").parse::<Uuid>().expect("UUID"))
        .execute(&setup.pool)
        .await
        .expect("alex is Ops in the project too");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, refused) = send(
        &setup.app,
        alex_browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some("ops-promotes-bea"),
            json!({"expectedVersion": version, "role": "member"}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "plain Member holds more than Ops: {refused}"
    );
    let (status, refused) = send(
        &setup.app,
        alex_browser.request(
            "POST",
            &format!(
                "/api/v1/organizations/{}/invitations",
                setup.organization_id
            ),
            None,
            json!({"role": "member", "expiresInHours": 24}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'member' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("bea is a member again");
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
    // Without "see tasks", search and overviews leave tasks out.
    let search = json!({"query": "Allowed", "kinds": ["tasks"], "projectId": work});
    let found = tool(app, &mara_token, &mara_session, "search", search.clone()).await;
    assert_eq!(
        found["value"]["results"].as_array().map(Vec::len),
        Some(1),
        "{found}"
    );
    sqlx::query("UPDATE workspace_roles SET permissions = '{}' WHERE id = $1")
        .bind(
            readers["id"]
                .as_str()
                .expect("id")
                .parse::<Uuid>()
                .expect("UUID"),
        )
        .execute(&setup.pool)
        .await
        .expect("Readers see nothing");
    let found = tool(app, &mara_token, &mara_session, "search", search).await;
    assert_eq!(found["value"]["results"], json!([]), "{found}");
    let opened = tool(
        app,
        &mara_token,
        &mara_session,
        "project_open",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(opened["value"]["tasks"]["next"], json!([]), "{opened}");

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

#[tokio::test]
async fn creating_from_a_template_needs_create_tasks_in_that_project() {
    let Some(setup) = setup("awd74_templates").await else {
        return;
    };
    let app = &setup.app;
    let alpha = create_project(&setup, "ALPHA", "Alpha").await;
    let beta = create_project(&setup, "BETA", "Beta").await;
    let (tom, tom_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Tom", "tom").await;
    add_member(&setup, &alpha, tom, "member").await;
    add_member(&setup, &beta, tom, "member").await;

    // In Beta, Tom may edit tasks but not create them.
    let (status, editors) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Editors", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{editors}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(beta.parse::<Uuid>().expect("project"))
    .bind(tom)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{beta}/members/{tom}"),
            Some("tom-editors"),
            json!({"expectedVersion": version, "roleId": editors["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");

    for project in [&alpha, &beta] {
        let key = format!("template-{project}");
        let (status, template) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{project}/task-templates"),
                Some(&key),
                json!({"name": "Weekly", "title": "From the template"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{template}");
    }

    let session = initialize(app, &tom_token).await.expect("Tom");
    // Creating tasks in Alpha lets the tool through the gate, and it works
    // there; in Beta the tool itself refuses.
    let made = tool(
        app,
        &tom_token,
        &session,
        "task_from_template",
        json!({"projectId": alpha, "template": "Weekly"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let refused = tool(
        app,
        &tom_token,
        &session,
        "task_from_template",
        json!({"projectId": beta, "template": "Weekly"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let in_beta = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_items WHERE project_id = $1 AND title = 'From the template'",
    )
    .bind(beta.parse::<Uuid>().expect("project"))
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(in_beta, 0, "nothing was created in Beta");
}

#[tokio::test]
async fn a_narrowed_project_admin_role_does_not_make_a_workspace_admin() {
    let Some(setup) = setup("awd74_promotion").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, _) = add_person(&setup, &[&work], "bea").await;
    let (status, lead) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Project lead", "baseRole": "admin", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{lead}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some("bea-lead"),
            json!({"expectedVersion": version, "roleId": lead["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["role"], "admin");
    // A plain project admin still becomes a workspace admin, as before; one
    // narrowed below "manage members" stays a workspace member.
    let workspace_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("workspace role");
    assert_eq!(workspace_role, "member");
}

/// A task's files follow who may see the task: removing "see tasks" from a
/// role, or from an app's agent limit, removes its files too, in the browser
/// and over MCP. "Edit tasks" alone is enough to attach a file to a task.
#[tokio::test]
async fn task_files_follow_who_can_see_tasks() {
    let Some((setup, _root)) = with_data_root("awd74_files").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let work_id = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("files-task"),
            json!({"title": "With a file"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    let task = made["id"].as_str().expect("task").to_owned();

    // An agent that may edit tasks but not post messages attaches a file.
    let (tom, tom_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Tom", "tom").await;
    add_member(&setup, &work, tom, "member").await;
    let (status, editors) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Task editors", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{editors}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work_id)
    .bind(tom)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{tom}"),
            Some("tom-task-editors"),
            json!({"expectedVersion": version, "roleId": editors["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let session = initialize(app, &tom_token).await.expect("Tom");
    let uploaded = tool(
        app,
        &tom_token,
        &session,
        "file_upload",
        json!({"taskId": task, "name": "notes.txt", "text": "task notes"}),
    )
    .await;
    assert_eq!(uploaded["outcome"], "ok", "{uploaded}");
    let file = uploaded["value"]["fileId"]
        .as_str()
        .expect("file")
        .to_owned();
    let opened = tool(
        app,
        &tom_token,
        &session,
        "file_read",
        json!({"fileId": file}),
    )
    .await;
    assert_eq!(opened["value"]["text"], "task notes", "{opened}");

    // Without "see tasks" in Tom's role, the file is gone for him.
    sqlx::query("UPDATE workspace_roles SET permissions = ARRAY['tasks.edit'] WHERE id = $1")
        .bind(
            editors["id"]
                .as_str()
                .expect("id")
                .parse::<Uuid>()
                .expect("UUID"),
        )
        .execute(&setup.pool)
        .await
        .expect("Task editors no longer see tasks");
    let refused = tool(
        app,
        &tom_token,
        &session,
        "file_read",
        json!({"fileId": file}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(!refused.to_string().contains("task notes"), "{refused}");

    // Bea's app reads it until her agent limit leaves out "see tasks";
    // Bea herself still reads it in the browser.
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let bea_session = initialize(app, &bea_app).await.expect("Bea's app");
    let opened = tool(
        app,
        &bea_app,
        &bea_session,
        "file_read",
        json!({"fileId": file}),
    )
    .await;
    assert_eq!(opened["value"]["text"], "task notes", "{opened}");
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["channels.post"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let refused = tool(
        app,
        &bea_app,
        &bea_session,
        "file_read",
        json!({"fileId": file}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(!refused.to_string().contains("task notes"), "{refused}");
    assert_eq!(
        read(&setup, &bea_browser, &file).await.as_deref(),
        Ok(&b"task notes"[..])
    );

    // In the browser, a role without "see tasks" loses the file too.
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "No tasks", "baseRole": "viewer", "permissions": []}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work_id)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some("bea-no-tasks"),
            json!({"expectedVersion": version, "roleId": blind["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(
        read(&setup, &bea_browser, &file).await,
        Err(StatusCode::NOT_FOUND)
    );
}

/// Bringing back a disabled member hands their role out again, so a
/// narrowed admin can do it only within what they hold, as with a role
/// change.
#[tokio::test]
async fn a_narrowed_admin_cannot_bring_back_more_than_they_hold() {
    let Some(setup) = setup("awd74_reenable").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let (bea, _) = add_person(&setup, &[&work], "bea").await;
    make_admin(&setup, alex).await;

    // Alex manages members as Ops, which leaves out editing tasks; Bea is a
    // plain Member, who edits tasks.
    let (status, narrowed) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Ops", "baseRole": "admin", "permissions": ["members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{narrowed}");
    let narrowed_id = narrowed["id"]
        .as_str()
        .expect("id")
        .parse::<Uuid>()
        .expect("UUID");
    let (status, assigned) = assign(&setup, &setup.browser, alex, &narrowed["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    sqlx::query("UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2")
        .bind(project)
        .bind(alex)
        .bind(narrowed_id)
        .execute(&setup.pool)
        .await
        .expect("alex is Ops in the project too");
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("bea was removed");

    // Alex can't bring Bea back without a role change: Member holds more
    // than Ops.
    let (version, _) = project_membership(&setup, project, bea).await;
    let (status, refused) = send(
        app,
        alex_browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some("ops-brings-bea-back"),
            json!({"expectedVersion": version, "enabled": true}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "plain Member holds more than Ops: {refused}"
    );
    assert!(
        !project_membership(&setup, project, bea).await.1,
        "Bea stayed out"
    );

    // The owner, who holds everything, can.
    let (status, back) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some("owner-brings-bea-back"),
            json!({"expectedVersion": version, "enabled": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert!(
        project_membership(&setup, project, bea).await.1,
        "Bea is back"
    );
}

/// File search leaves out a task's files where the viewer (or an app's
/// limit) can't see tasks, and keeps files posted in messages.
#[tokio::test]
async fn file_search_leaves_out_task_files_where_tasks_are_hidden() {
    let Some((setup, _root)) = with_data_root("awd74_file_search").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("file-search-task"),
            json!({"title": "Quarterly plan"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    let task = made["id"].as_str().expect("task").to_owned();
    let channel = create_channel(&setup, &work, "general").await;

    // Tom attaches one file to the task and posts another in a channel.
    let (tom, tom_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Tom", "tom").await;
    add_member(&setup, &work, tom, "member").await;
    let session = initialize(app, &tom_token).await.expect("Tom");
    let on_task = tool(
        app,
        &tom_token,
        &session,
        "file_upload",
        json!({"taskId": task, "name": "budget-task.txt", "text": "task budget"}),
    )
    .await;
    assert_eq!(on_task["outcome"], "ok", "{on_task}");
    let in_channel = tool(
        app,
        &tom_token,
        &session,
        "file_upload",
        json!({"conversationId": channel, "name": "budget-channel.txt", "text": "channel budget"}),
    )
    .await;
    assert_eq!(in_channel["outcome"], "ok", "{in_channel}");
    let posted = tool(
        app,
        &tom_token,
        &session,
        "message_send",
        json!({"conversationId": channel, "attachmentIds": [in_channel["value"]["fileId"]]}),
    )
    .await;
    assert_eq!(posted["outcome"], "ok", "{posted}");
    let names = |found: &Value| {
        let mut names = found["value"]["results"]
            .as_array()
            .unwrap_or_else(|| panic!("results: {found}"))
            .iter()
            .map(|hit| hit["title"].as_str().expect("title").to_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    let search = json!({"query": "budget", "kinds": ["files"], "projectId": work});

    // Bea's app finds both until her agent limit leaves out "see tasks".
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let bea_session = initialize(app, &bea_app).await.expect("Bea's app");
    let found = tool(app, &bea_app, &bea_session, "search", search.clone()).await;
    assert_eq!(
        names(&found),
        ["budget-channel.txt", "budget-task.txt"],
        "{found}"
    );
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["channels.post"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let found = tool(app, &bea_app, &bea_session, "search", search.clone()).await;
    assert_eq!(names(&found), ["budget-channel.txt"], "{found}");
    assert!(!found.to_string().contains("Quarterly plan"), "{found}");

    // An agent whose role leaves out "see tasks" finds only the channel's.
    let (status, posters) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Posters", "baseRole": "member", "permissions": ["channels.post"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{posters}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(tom)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{tom}"),
            Some("tom-posters"),
            json!({"expectedVersion": version, "roleId": posters["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let found = tool(app, &tom_token, &session, "search", search).await;
    assert_eq!(names(&found), ["budget-channel.txt"], "{found}");
    assert!(!found.to_string().contains("Quarterly plan"), "{found}");
}

/// A project membership's version and whether it's enabled.
async fn project_membership(setup: &Setup, project: Uuid, actor: Uuid) -> (i64, bool) {
    sqlx::query_as::<_, (i64, bool)>(
        "SELECT version, enabled FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("project membership")
}

/// The token in an issued invitation's URL.
fn invitation_token(issued: &Value) -> String {
    issued["invitationUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("invitation URL: {issued}"))
        .split_once("#token=")
        .expect("token fragment")
        .1
        .to_owned()
}

/// Every way of handing access out again stays within what the giver holds
/// now: an invitation stops working once its issuer's role narrows below
/// what it gives, a narrowed admin can't resend an invitation for more than
/// they hold, and they can't switch back on an agent holding more.
#[tokio::test]
async fn invitations_and_agents_stay_within_what_the_giver_holds_now() {
    let Some(setup) = setup("awd74_regrant").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    make_admin(&setup, alex).await;
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .execute(&setup.pool)
    .await
    .expect("alex is a project admin");

    // As a plain admin, Alex invites a Member to the workspace and the project.
    let mut tokens = Vec::new();
    for (path, resolve) in [
        (
            format!(
                "/api/v1/organizations/{}/invitations",
                setup.organization_id
            ),
            "/api/v1/workspace-invitations/resolve",
        ),
        (
            format!("/api/v1/projects/{work}/invitations"),
            "/api/v1/project-invitations/resolve",
        ),
    ] {
        let (status, issued) = send(
            app,
            alex_browser.request(
                "POST",
                &path,
                None,
                json!({"role": "member", "expiresInHours": 24}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{path}: {issued}");
        let token = invitation_token(&issued);
        let (status, resolved) = send(
            app,
            setup
                .browser
                .request("POST", resolve, None, json!({"token": token})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{resolve}: {resolved}");
        tokens.push((resolve, token));
    }
    // The owner invites a Member to the project too, and adds an agent.
    let (status, owners) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/invitations"),
            None,
            json!({"role": "member", "expiresInHours": 24}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{owners}");
    let owners_invitation = owners["id"].as_str().expect("invitation").to_owned();
    let (mara, _) = create_agent(app, &setup.browser, &work, "Mara", "mara").await;
    let (connection_id, version, _) = agent_connection(&setup, mara).await;
    let (status, disabled) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/external-agents/{connection_id}"),
            None,
            json!({"expectedVersion": version, "enabled": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");

    // Then the owner narrows Alex to Ops, which leaves out editing tasks.
    let (status, ops) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Ops", "baseRole": "admin", "permissions": ["agents.manage", "members.invite", "members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{ops}");
    let (status, assigned) = assign(&setup, &setup.browser, alex, &ops["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    sqlx::query(
        "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .bind(
        ops["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .execute(&setup.pool)
    .await
    .expect("alex is Ops in the project too");

    // Alex's invitations no longer open: Member holds more than Ops.
    for (resolve, token) in &tokens {
        let (status, refused) = send(
            app,
            setup
                .browser
                .request("POST", resolve, None, json!({"token": token})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{resolve}: {refused}");
    }
    // Nor can Alex resend the owner's Member invitation as their own.
    let (status, refused) = send(
        app,
        alex_browser.request(
            "POST",
            &format!("/api/v1/project-invitations/{owners_invitation}/replace"),
            None,
            json!({"expectedVersion": 1, "expiresInHours": 48}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    // Nor switch Mara, a Member, back on.
    let (_, version, _) = agent_connection(&setup, mara).await;
    let (status, refused) = send(
        app,
        alex_browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/external-agents/{connection_id}"),
            None,
            json!({"expectedVersion": version, "enabled": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (_, _, status_now) = agent_connection(&setup, mara).await;
    assert_ne!(status_now, "active", "Mara stayed off");
    // The owner, who holds everything, can.
    let (status, enabled) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/external-agents/{connection_id}"),
            None,
            json!({"expectedVersion": version, "enabled": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
}

/// An agent's connection: its ID, version and status.
async fn agent_connection(setup: &Setup, agent: Uuid) -> (Uuid, i64, String) {
    sqlx::query_as::<_, (Uuid, i64, String)>(
        "SELECT id, version, status FROM external_agent_connections WHERE actor_id = $1",
    )
    .bind(agent)
    .fetch_one(&setup.pool)
    .await
    .expect("agent connection")
}

/// A task made by the owner in `project`, by its title; returns its ID.
async fn owner_task(setup: &Setup, project: &str, title: &str) -> Uuid {
    let (status, made) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/work-items"),
            Some(&format!("task-{}", title.replace(' ', "-"))),
            json!({"title": title}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    made["id"]
        .as_str()
        .expect("task")
        .parse::<Uuid>()
        .expect("UUID")
}

/// The array at `field` in a successful tool result. Some tools (later_list)
/// leave an empty list out, so a missing one is empty.
fn listed(result: &Value, field: &str) -> Vec<Value> {
    assert_eq!(result["outcome"], "ok", "{field}: {result}");
    result["value"][field]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Reads that name a task or a repository follow "see tasks" and "see
/// code" in each project, for the role and for an app's agent limit: the
/// task list across projects, saved items, notifications and the project
/// overview's repositories.
#[tokio::test]
async fn reads_naming_tasks_or_repositories_follow_the_role_and_limit() {
    let Some(setup) = setup("awd74_named_reads").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    let in_work = owner_task(&setup, &work, "Visible plan").await;
    let in_other = owner_task(&setup, &other, "Hidden plan").await;
    let repository = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, trusted, preparation_state, prepared_at
        ) VALUES ($1, $2, 424242, 'Mischa-dev', 'roles-repo',
                  'https://github.com/Mischa-dev/roles-repo.git',
                  'main', $3, true, 'ready', now())
        "#,
    )
    .bind(repository)
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(format!("/tmp/agent-workspace-test-{repository}.git"))
    .execute(&setup.pool)
    .await
    .expect("repository");
    for (project, task) in [(&work, in_work), (&other, in_other)] {
        sqlx::query(
            r#"
            INSERT INTO notifications (
                id, project_id, recipient_actor_id, kind, subject_kind, subject_id,
                work_item_id, group_key
            ) VALUES ($1, $2, $3, 'task_assigned', 'work_item', $4, $4, $5)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(bea)
        .bind(task)
        .bind(format!("assigned:{task}"))
        .execute(&setup.pool)
        .await
        .expect("notification");
    }

    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    for task in [in_work, in_other] {
        let saved = tool(app, &bea_app, &session, "later_save", json!({"task": task})).await;
        assert_eq!(saved["outcome"], "ok", "{saved}");
    }
    let titles = |result: &Value| {
        let mut titles = listed(result, "tasks")
            .iter()
            .map(|task| task["title"].as_str().expect("title").to_owned())
            .collect::<Vec<_>>();
        titles.sort();
        titles
    };
    let both = json!({"projectIds": [work, other]});
    let found = tool(app, &bea_app, &session, "task_list", both.clone()).await;
    assert_eq!(titles(&found), ["Hidden plan", "Visible plan"], "{found}");
    let later = tool(app, &bea_app, &session, "later_list", json!({})).await;
    assert_eq!(listed(&later, "items").len(), 2, "{later}");
    let inbox = tool(app, &bea_app, &session, "notification_list", json!({})).await;
    assert_eq!(listed(&inbox, "notifications").len(), 2, "{inbox}");
    let overview = tool(
        app,
        &bea_app,
        &session,
        "project_open",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(listed(&overview, "repositories").len(), 1, "{overview}");

    // In Other, Bea's role no longer sees tasks: its task leaves the task
    // list (though the gate opens on Work), saved items and notifications.
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "No tasks", "baseRole": "viewer", "permissions": []}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let other_id = other.parse::<Uuid>().expect("project");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(other_id)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{other}/members/{bea}"),
            Some("bea-other-no-tasks"),
            json!({"expectedVersion": version, "roleId": blind["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let found = tool(app, &bea_app, &session, "task_list", both).await;
    assert_eq!(titles(&found), ["Visible plan"], "{found}");
    let later = tool(app, &bea_app, &session, "later_list", json!({})).await;
    assert_eq!(listed(&later, "items").len(), 1, "{later}");
    assert!(!later.to_string().contains("Hidden plan"), "{later}");
    let inbox = tool(app, &bea_app, &session, "notification_list", json!({})).await;
    assert_eq!(listed(&inbox, "notifications").len(), 1, "{inbox}");
    assert!(!inbox.to_string().contains("Hidden plan"), "{inbox}");
    let (status, page) = send(
        app,
        bea_browser.request("GET", "/api/v1/later", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1), "{page}");

    // Her app's limit leaves out "see tasks" and "see code": Work's task,
    // its notification and the repository go too.
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["channels.post"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let later = tool(app, &bea_app, &session, "later_list", json!({})).await;
    assert_eq!(listed(&later, "items").len(), 0, "{later}");
    let inbox = tool(app, &bea_app, &session, "notification_list", json!({})).await;
    assert_eq!(listed(&inbox, "notifications").len(), 0, "{inbox}");
    let overview = tool(
        app,
        &bea_app,
        &session,
        "project_open",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(overview["outcome"], "ok", "{overview}");
    assert_eq!(listed(&overview, "repositories").len(), 0, "{overview}");
    // Bea herself still has Work's in the browser.
    let (_, page) = send(
        app,
        bea_browser.request("GET", "/api/v1/later", None, Value::Null),
    )
    .await;
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1), "{page}");
}

/// Moderating someone else's content through an app needs "moderate
/// channels" in the app's agent limit as well as in the person's role: an
/// app acting for a project admin can't delete another person's task
/// comment while its limit leaves moderation out.
#[tokio::test]
async fn an_app_moderates_only_within_its_agent_limit() {
    let Some(setup) = setup("awd74_moderation").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("bea is a project admin");
    let task = owner_task(&setup, &work, "Moderated").await;
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    let commented = tool(
        app,
        &owner_app,
        &owner_session,
        "task_comment",
        json!({"taskId": task, "text": "The owner's note", "clientRequestId": "owner-note"}),
    )
    .await;
    assert_eq!(commented["outcome"], "applied", "{commented}");
    let comment = commented["resultId"]
        .as_str()
        .unwrap_or_else(|| panic!("comment id: {commented}"))
        .parse::<Uuid>()
        .expect("UUID");
    let removed = move |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, bool>(
            "SELECT removed_at IS NOT NULL FROM work_item_comments WHERE id = $1",
        )
        .bind(comment)
        .fetch_one(&pool)
        .await
        .expect("comment")
    };

    // Bea's app may see and edit tasks, but not moderate.
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["tasks.view", "tasks.edit"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let delete = json!({"taskId": task, "commentId": comment});
    let refused = tool(
        app,
        &bea_app,
        &session,
        "task_comment_delete",
        delete.clone(),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(
        !removed(setup.pool.clone()).await,
        "the app moderated beyond its limit"
    );

    // With the limit lifted, Bea's admin role moderates through it.
    let (status, limit) = send(
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
    assert_eq!(status, StatusCode::OK, "{limit}");
    let deleted = tool(app, &bea_app, &session, "task_comment_delete", delete).await;
    assert_eq!(deleted["outcome"], "ok", "{deleted}");
    assert!(removed(setup.pool.clone()).await, "Bea's role moderates");
}

/// The task chips in `browser`'s view of a conversation's timeline.
async fn timeline_chips(setup: &Setup, browser: &Browser, conversation: &str) -> Vec<Value> {
    let (status, timeline) = send(
        &setup.app,
        browser.request(
            "GET",
            &format!("/api/v1/conversations/{conversation}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    timeline["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("entries: {timeline}"))
        .iter()
        .filter_map(|entry| entry["message"]["referencedWorkItems"].as_array())
        .flatten()
        .cloned()
        .collect()
}

/// A task chip in a message names its task (title and status) only for a
/// reader who sees tasks there; others get the key and link alone. MCP
/// reads never name it for an app whose limit leaves out "see tasks".
#[tokio::test]
async fn task_chips_name_the_task_only_for_readers_who_see_tasks() {
    let Some(setup) = setup("awd74_chips").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (cal, cal_browser) = add_person(&setup, &[&work], "cal").await;
    let channel = create_channel(&setup, &work, "general").await;
    let task = owner_task(&setup, &work, "Secret roadmap").await;
    let (status, posted) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{channel}/messages"),
            Some("chip-message"),
            json!({"content": {"version": 1, "nodes": [
                {"type": "text", "text": "See "},
                {"type": "work_item_ref", "workItemId": task}
            ]}}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {posted}");

    // Bea sees tasks: the chip names the task.
    let chips = timeline_chips(&setup, &bea_browser, &channel).await;
    assert_eq!(chips.len(), 1, "{chips:?}");
    assert_eq!(chips[0]["title"], "Secret roadmap", "{chips:?}");

    // Once her role leaves out "see tasks", the key and link stay.
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "No tasks", "baseRole": "viewer", "permissions": []}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some("bea-chips-no-tasks"),
            json!({"expectedVersion": version, "roleId": blind["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let chips = timeline_chips(&setup, &bea_browser, &channel).await;
    assert_eq!(chips.len(), 1, "{chips:?}");
    assert!(chips[0].get("title").is_none(), "{chips:?}");
    assert!(chips[0].get("statusCategory").is_none(), "{chips:?}");
    assert!(
        chips[0]["key"].is_string() && chips[0]["href"].is_string(),
        "{chips:?}"
    );

    // Cal's app, limited away from "see tasks", reads the channel without
    // the task's title; Cal in the browser still sees it.
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{cal}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["channels.post"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let cal_app = connect_app(app, &cal_browser, "Cal's app", "full").await;
    let session = initialize(app, &cal_app).await.expect("Cal's app");
    let read = tool(
        app,
        &cal_app,
        &session,
        "conversation_read",
        json!({"conversationId": channel}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert!(!read.to_string().contains("Secret roadmap"), "{read}");
    let chips = timeline_chips(&setup, &cal_browser, &channel).await;
    assert_eq!(chips[0]["title"], "Secret roadmap", "{chips:?}");
}

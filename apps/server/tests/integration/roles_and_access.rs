//! AWD-74: roles and access. The silent, costly failures: a built-in role
//! that quietly means something else, someone handing out more than they
//! hold (or an admin role without being an owner), a workspace left without
//! an owner, and an app or agent acting for a person doing more than that
//! person's agent limit allows, through MCP or the public API.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    mcp_tasks::{add_member, space},
    message_edits::{add_person, create_channel, direct_space, owner_id, say, text},
    support::{
        app::{Browser, connect_app, create_agent, initialize, send, tool},
        world::{Setup, create_project, setup},
    },
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
    // Viewers only read, apart from reporting their own work, which every
    // role does (A1.3).
    assert!(
        keys(&role(&listed, "viewer")["permissions"])
            .iter()
            .all(|key| key.ends_with(".view") || key == "runs.report" || key == "requests.raise"),
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
    // And a plain Admin in Work again: applying a role there is held to
    // Alex's role in the project too, which was still Ops.
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin', role_id = NULL WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .execute(&setup.pool)
    .await
    .expect("alex is a plain admin in Work again");

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
pub(super) async fn public_call(
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

pub(super) async fn listed_tools(app: &axum::Router, token: &str, session: &str) -> Vec<String> {
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
        "SELECT id::text FROM task_rows WHERE project_id = $1 AND title = 'Made in the browser'",
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
        "SELECT count(*) FROM task_rows WHERE title IN ('Made by the app', 'Made by a script', 'Renamed by the app')",
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
    // A member caps their own apps, and nobody else's.
    let own_cap = tool(
        app,
        &bea_app,
        &session,
        "role_assign",
        json!({"projectId": work, "person": "@bea", "agentLimit": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(own_cap["outcome"], "ok", "{own_cap}");
    let lifted = tool(
        app,
        &bea_app,
        &session,
        "role_assign",
        json!({"projectId": work, "person": "@bea", "agentLimit": null}),
    )
    .await;
    assert_eq!(lifted["outcome"], "permission_denied", "{lifted}");
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
        "SELECT count(*) FROM task_rows WHERE project_id = $1 AND title = 'From the template'",
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
            // Idempotency keys take letters, digits, - _ and . only; a space was
            // always -, so existing keys stay the same.
            Some(&format!(
                "task-{}",
                title
                    .chars()
                    .map(|character| {
                        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                        {
                            character
                        } else {
                            '-'
                        }
                    })
                    .collect::<String>()
            )),
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
    // Things (AWD-75) name tasks too: both open, for her app and in the browser.
    let open_thing = |task: Uuid| {
        let token = bea_app.clone();
        let session = session.clone();
        async move {
            tool(
                app,
                &token,
                &session,
                "thing_open",
                json!({"kind": "task", "id": task}),
            )
            .await
        }
    };
    let browse_thing = |task: Uuid| {
        let request = bea_browser.request(
            "GET",
            &format!("/api/v1/things/task/{task}"),
            None,
            Value::Null,
        );
        async move { send(app, request).await }
    };
    for task in [in_work, in_other] {
        let opened = open_thing(task).await;
        assert_eq!(opened["outcome"], "ok", "{opened}");
        let (status, thing) = browse_thing(task).await;
        assert_eq!(status, StatusCode::OK, "{thing}");
    }

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
    // Nor is it a thing for her any more, by MCP or in the browser.
    let opened = open_thing(in_other).await;
    assert_eq!(opened["outcome"], "not_found", "{opened}");
    assert!(!opened.to_string().contains("Hidden plan"), "{opened}");
    let (status, thing) = browse_thing(in_other).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{thing}");
    assert!(!thing.to_string().contains("Hidden plan"), "{thing}");
    let opened = open_thing(in_work).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
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
    // Work's task isn't a thing for her app now, but still is in her browser.
    let opened = open_thing(in_work).await;
    assert_eq!(opened["outcome"], "not_found", "{opened}");
    let (status, thing) = browse_thing(in_work).await;
    assert_eq!(status, StatusCode::OK, "{thing}");
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
/// P0.8: the SQL permission helpers hold to the membership's base role, so
/// a viewer or a worker (no custom role) never keeps what only members hold.
#[tokio::test]
async fn sql_permission_helpers_hold_to_the_base_role() {
    use agent_workspace_server::{policy::Permission, search::projects_allowing};
    let Some(setup) = setup("p08_base_role").await else {
        return;
    };
    let work = create_project(&setup, "BASE", "Base").await;
    let project = work.parse::<Uuid>().expect("project");
    let (member, _) = add_person(&setup, &[&work], "mia").await;
    let (viewer, _) = add_person(&setup, &[&work], "vic").await;
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer', role_id = NULL WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(viewer)
    .execute(&setup.pool)
    .await
    .expect("viewer");
    let worker = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'execution_worker')")
        .bind(worker)
        .execute(&setup.pool)
        .await
        .expect("worker actor");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
         VALUES ($1, $2, $3, 'worker', 'Base worker', 'base-worker')",
    )
    .bind(Uuid::now_v7())
    .bind(project)
    .bind(worker)
    .execute(&setup.pool)
    .await
    .expect("worker membership");
    let allowing = |actor: Uuid, permission: Permission| {
        let pool = setup.pool.clone();
        async move {
            projects_allowing(&pool, actor, &[project], permission)
                .await
                .expect("projects allowing")
        }
    };
    assert_eq!(allowing(member, Permission::TasksEdit).await, vec![project]);
    assert!(allowing(viewer, Permission::TasksEdit).await.is_empty());
    assert!(allowing(worker, Permission::TasksEdit).await.is_empty());
    assert_eq!(allowing(viewer, Permission::TasksView).await, vec![project]);
    assert!(allowing(worker, Permission::TasksView).await.is_empty());
}

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
    // The comment's ID is the write's activityId; resultId is its audit record.
    let comment = commented["activityId"]
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

/// A task's linked repositories and pull requests come with it only for a
/// reader whose role (and app limit) keeps "see code"; the task itself
/// stays, in the browser and over MCP.
#[tokio::test]
async fn a_tasks_linked_code_needs_see_code() {
    let Some(setup) = setup("awd74_task_code").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Linked to code").await;
    let repository = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, trusted, preparation_state, prepared_at
        ) VALUES ($1, $2, 434343, 'Mischa-dev', 'task-code',
                  'https://github.com/Mischa-dev/task-code.git',
                  'main', $3, true, 'ready', now())
        "#,
    )
    .bind(repository)
    .bind(project)
    .bind(format!("/tmp/agent-workspace-test-{repository}.git"))
    .execute(&setup.pool)
    .await
    .expect("repository");
    sqlx::query(
        r#"
        INSERT INTO work_item_repository_links
            (project_id, work_item_id, repository_id, role, created_by_actor_id)
        VALUES ($1, $2, $3, 'primary', $4)
        "#,
    )
    .bind(project)
    .bind(task)
    .bind(repository)
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("link the repository");
    let count = |value: &Value| value.as_array().map_or(0, Vec::len);
    let browser_task = |browser: &Browser| {
        browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items/{task}"),
            None,
            Value::Null,
        )
    };

    let (status, item) = send(app, browser_task(&bea_browser)).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert_eq!(count(&item["linkedRepositories"]), 1, "{item}");
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let opened = tool(
        app,
        &bea_app,
        &session,
        "task_open",
        json!({"taskId": task}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(
        count(&opened["value"]["task"]["linkedRepositories"]),
        1,
        "{opened}"
    );

    // Her app's limit leaves out "see code": the task, without its code.
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["tasks.view"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let opened = tool(
        app,
        &bea_app,
        &session,
        "task_open",
        json!({"taskId": task}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(
        opened["value"]["task"]["title"], "Linked to code",
        "{opened}"
    );
    assert_eq!(
        count(&opened["value"]["task"]["linkedRepositories"]),
        0,
        "{opened}"
    );
    assert!(!opened.to_string().contains("task-code.git"), "{opened}");

    // A role without "see code" gets the same in the browser.
    let (status, tasks_only) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Tasks only", "baseRole": "viewer", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tasks_only}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    let (status, changed) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some("bea-tasks-only"),
            json!({"expectedVersion": version, "roleId": tasks_only["id"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let (status, item) = send(app, browser_task(&bea_browser)).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert_eq!(item["title"], "Linked to code", "{item}");
    assert_eq!(count(&item["linkedRepositories"]), 0, "{item}");
}

/// Sets `actor`'s role in `project` as the owner, with `body` (a role or a
/// roleId) and the membership's current version.
async fn set_project_role(
    setup: &Setup,
    project: &str,
    actor: Uuid,
    key: &str,
    mut body: Value,
) -> (StatusCode, Value) {
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project.parse::<Uuid>().expect("project"))
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("version");
    body["expectedVersion"] = json!(version);
    send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{project}/members/{actor}"),
            Some(key),
            body,
        ),
    )
    .await
}

/// `actor`'s plain workspace role.
async fn workspace_role(setup: &Setup, actor: Uuid) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT role FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("workspace role")
}

/// A custom project role based on Admin never widens into the full
/// workspace Admin, even when it keeps "manage members": its holder can't
/// send workspace invitations it leaves out. The built-in project Admin
/// still makes a workspace admin, as before.
#[tokio::test]
async fn only_the_built_in_project_admin_makes_a_workspace_admin() {
    let Some(setup) = setup("awd74_custom_admin").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (cal, cal_browser) = add_person(&setup, &[&work], "cal").await;
    let (status, manager) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Project manager", "baseRole": "admin", "permissions": ["members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{manager}");
    let invitations = format!(
        "/api/v1/organizations/{}/invitations",
        setup.organization_id
    );
    let invite = json!({"role": "member", "expiresInHours": 24});

    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-manager",
        json!({"roleId": manager["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(workspace_role(&setup, bea).await, "member");
    let (status, refused) = send(
        app,
        bea_browser.request("POST", &invitations, None, invite.clone()),
    )
    .await;
    assert!(
        !status.is_success(),
        "a custom project admin invited to the workspace: {refused}"
    );

    let (status, changed) =
        set_project_role(&setup, &work, cal, "cal-admin", json!({"role": "admin"})).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(workspace_role(&setup, cal).await, "admin");
    let (status, invited) =
        send(app, cal_browser.request("POST", &invitations, None, invite)).await;
    assert_eq!(status, StatusCode::CREATED, "{invited}");
}

const OAUTH_REDIRECT: &str = "http://127.0.0.1:49152/callback";

/// Whether `browser`'s person can complete OAuth consent connecting a new
/// app as the agent behind `connection`: true when an authorization code is
/// issued.
async fn authorizes_as_agent(setup: &Setup, browser: &Browser, connection: Uuid) -> bool {
    let (status, registration) = send(
        &setup.app,
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "client_name": "Agent client",
                    "redirect_uris": [OAUTH_REDIRECT],
                    "token_endpoint_auth_method": "none",
                    "grant_types": ["authorization_code", "refresh_token"],
                    "response_types": ["code"]
                }))
                .expect("registration"),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{registration}");
    let client_id = registration["client_id"]
        .as_str()
        .expect("client")
        .to_owned();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest("v".repeat(43).as_bytes()));
    let agent = format!("agent:{connection}");
    let form = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", OAUTH_REDIRECT),
        ("scope", "read write"),
        ("state", "state"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("connect_as", agent.as_str()),
        ("access", "full"),
        ("csrf_token", browser.csrf.as_str()),
        ("decision", "approve"),
    ])
    .expect("consent");
    let response = setup
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/confirm")
                .header(header::COOKIE, &browser.cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .expect("consent request"),
        )
        .await
        .expect("consent response");
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let page = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .expect("consent page")
            .to_vec(),
    )
    .unwrap_or_default();
    location.contains("code=aw_code_") || page.contains("code=aw_code_")
}

/// Connecting an app as an agent hands it the agent's role, so it's held to
/// what the person connecting holds: a narrowed admin can't connect as a
/// plain Member agent; the owner can.
#[tokio::test]
async fn a_narrowed_admin_cannot_connect_an_app_as_a_stronger_agent() {
    let Some(setup) = setup("awd74_oauth_agent").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    make_admin(&setup, alex).await;
    let (status, ops) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Ops", "baseRole": "admin", "permissions": ["agents.manage", "members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{ops}");
    let (status, assigned) = assign(&setup, &setup.browser, alex, &ops["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    sqlx::query("UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2")
        .bind(project)
        .bind(alex)
        .bind(ops["id"].as_str().expect("id").parse::<Uuid>().expect("UUID"))
        .execute(&setup.pool)
        .await
        .expect("alex is Ops in the project too");
    let (mara, _) = create_agent(&setup.app, &setup.browser, &work, "Mara", "mara").await;
    let (connection, _, _) = agent_connection(&setup, mara).await;

    assert!(
        !authorizes_as_agent(&setup, &alex_browser, connection).await,
        "a narrowed admin connected an app as a Member agent"
    );
    assert!(
        authorizes_as_agent(&setup, &setup.browser, connection).await,
        "the owner connects an app as the agent"
    );
}

/// The same holds for the agent's project: a workspace Admin whose role in
/// Work keeps "manage agents" but leaves out editing tasks and seeing code
/// can't connect an app as a Member agent working there; as a plain Admin
/// in Work, they can.
#[tokio::test]
async fn connecting_an_app_as_an_agent_stays_within_the_project_role() {
    let Some(setup) = setup("awd74_oauth_agent_project").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    make_admin(&setup, alex).await;
    let (status, keepers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Agent keepers in Work", "baseRole": "admin", "permissions": ["agents.manage", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{keepers}");
    let set_alex = |role_id: Option<Uuid>| {
        let pool = setup.pool.clone();
        async move {
            sqlx::query(
                "UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2",
            )
            .bind(project)
            .bind(alex)
            .bind(role_id)
            .execute(&pool)
            .await
            .expect("alex's role in Work");
        }
    };
    let (mara, _) = create_agent(&setup.app, &setup.browser, &work, "Mara", "mara").await;
    let (connection, _, _) = agent_connection(&setup, mara).await;

    set_alex(Some(
        keepers["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    ))
    .await;
    assert!(
        !authorizes_as_agent(&setup, &alex_browser, connection).await,
        "an admin narrowed in the project connected an app as a Member agent there"
    );
    set_alex(None).await;
    assert!(
        authorizes_as_agent(&setup, &alex_browser, connection).await,
        "a plain admin in the project connects an app as the agent"
    );
}

/// A project invitation with a custom role based on Admin never makes a
/// workspace admin, whether the invitee is new or already in the workspace.
#[tokio::test]
async fn a_custom_admin_invitation_does_not_make_a_workspace_admin() {
    let Some(setup) = setup("awd74_invite_admin").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    // In the workspace through another project (an account needs one), not in Work.
    let elsewhere = create_project(&setup, "ELSE", "Elsewhere").await;
    let (bea, bea_browser) = add_person(&setup, &[&elsewhere], "bea").await;
    let (status, manager) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Project manager", "baseRole": "admin", "permissions": ["members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{manager}");
    let mut tokens = Vec::new();
    for _ in 0..2 {
        let (status, issued) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/projects/{work}/invitations"),
                None,
                json!({"role": "admin", "roleId": manager["id"], "expiresInHours": 24}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{issued}");
        tokens.push(invitation_token(&issued));
    }

    // Someone new joins with it.
    let (status, joined) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/v1/project-invitations/accept-new")
            .header(header::ORIGIN, "http://127.0.0.1:3100")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "token": tokens[0],
                    "loginName": "newcomer",
                    "displayName": "Newcomer",
                    "handle": "newcomer",
                    "password": "newcomer correct horse battery staple",
                }))
                .expect("body"),
            ))
            .expect("request"),
    )
    .await;
    assert!(status.is_success(), "{status}: {joined}");
    let newcomer_role = sqlx::query_scalar::<_, String>(
        r#"
        SELECT membership.role FROM organization_memberships membership
        JOIN users account ON account.actor_id = membership.actor_id
        WHERE membership.organization_id = $1 AND account.login_name = 'newcomer'
        "#,
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("newcomer's workspace role");
    assert_eq!(newcomer_role, "member");

    // Bea, already in the workspace, accepts the other.
    let (status, accepted) = send(
        app,
        bea_browser.request(
            "POST",
            "/api/v1/project-invitations/accept-existing",
            None,
            json!({"token": tokens[1], "handle": "bea"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {accepted}");
    assert_eq!(workspace_role(&setup, bea).await, "member");
}

/// A project invitation holds only while its issuer may still manage
/// invitations in that project: an issuer who stays a workspace admin but
/// whose project role loses "invite people" can't have their pending
/// invitation opened or accepted.
#[tokio::test]
async fn a_project_invitation_lapses_when_its_issuer_loses_invite_in_the_project() {
    let Some(setup) = setup("awd74_issuer_project").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    // In the workspace through another project (an account needs one), not in Work.
    let elsewhere = create_project(&setup, "ELSE", "Elsewhere").await;
    let (_, bea_browser) = add_person(&setup, &[&elsewhere], "bea").await;
    make_admin(&setup, alex).await;
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .execute(&setup.pool)
    .await
    .expect("alex is a project admin");
    let (status, issued) = send(
        app,
        alex_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/invitations"),
            None,
            json!({"role": "member", "expiresInHours": 24}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let token = invitation_token(&issued);
    let resolve = |token: &str| {
        setup.browser.request(
            "POST",
            "/api/v1/project-invitations/resolve",
            None,
            json!({"token": token}),
        )
    };
    let (status, resolved) = send(app, resolve(&token)).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");

    // Alex stays a workspace admin, but in the project loses "invite people".
    let (status, narrowed) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "No inviting", "baseRole": "admin", "permissions": ["members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{narrowed}");
    sqlx::query(
        "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .bind(
        narrowed["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .execute(&setup.pool)
    .await
    .expect("narrow alex in the project");
    assert_eq!(workspace_role(&setup, alex).await, "admin");

    let (status, refused) = send(app, resolve(&token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    let (status, refused) = send(
        app,
        bea_browser.request(
            "POST",
            "/api/v1/project-invitations/accept-existing",
            None,
            json!({"token": token, "handle": "bea"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
}

/// A task's links to tasks in another project (blockers, blocked tasks,
/// relations) name those tasks only where the reader sees tasks there.
#[tokio::test]
async fn linked_tasks_in_another_project_follow_see_tasks_there() {
    let Some(setup) = setup("awd74_cross_links").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    let blocked = owner_task(&setup, &work, "Needs the other").await;
    let blocker = owner_task(&setup, &other, "Hidden blocker").await;
    sqlx::query(
        r#"
        INSERT INTO work_item_dependencies (
            project_id, blocked_work_item_id, blocking_project_id, blocking_work_item_id,
            created_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(blocked)
    .bind(other.parse::<Uuid>().expect("project"))
    .bind(blocker)
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("dependency");
    let detail = |browser: &Browser| {
        browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items/{blocked}"),
            None,
            Value::Null,
        )
    };
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let open = json!({"taskId": blocked});

    // Bea sees tasks in both: the blocker is named.
    let (status, item) = send(app, detail(&bea_browser)).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert!(item.to_string().contains("Hidden blocker"), "{item}");
    let opened = tool(app, &bea_app, &session, "task_open", open.clone()).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(opened.to_string().contains("Hidden blocker"), "{opened}");

    // Her role in Other loses "see tasks": the blocker isn't named anywhere.
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "No tasks", "baseRole": "viewer", "permissions": []}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let (status, changed) = set_project_role(
        &setup,
        &other,
        bea,
        "bea-other-blind",
        json!({"roleId": blind["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let (status, item) = send(app, detail(&bea_browser)).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert!(!item.to_string().contains("Hidden blocker"), "{item}");
    let opened = tool(app, &bea_app, &session, "task_open", open).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(!opened.to_string().contains("Hidden blocker"), "{opened}");
}

/// The built-in Viewer can't become the workspace's default role: a
/// workspace has no plain Viewer, and invitations couldn't honour it. The
/// previous default stays.
#[tokio::test]
async fn the_built_in_viewer_cannot_be_the_workspace_default() {
    let Some(setup) = setup("awd74_viewer_default").await else {
        return;
    };
    let listed = roles(&setup, &setup.browser).await;
    assert_eq!(role(&listed, "member")["isDefault"], true);
    let viewer = role(&listed, "viewer");
    let (status, refused) = send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!(
                "{}/{}",
                roles_path(&setup),
                viewer["id"].as_str().expect("id")
            ),
            None,
            json!({"expectedVersion": viewer["version"], "isDefault": true}),
        ),
    )
    .await;
    assert!(!status.is_success(), "{status}: {refused}");
    let listed = roles(&setup, &setup.browser).await;
    assert_eq!(role(&listed, "member")["isDefault"], true, "{listed}");
    assert_eq!(role(&listed, "viewer")["isDefault"], false, "{listed}");
}

/// A write-scope personal token for anyone whose role may write anything
/// in the project, not only edit tasks; each tool still checks its own
/// action. A role that may only see things still gets read access only.
#[tokio::test]
async fn a_write_token_follows_any_write_permission() {
    let Some(setup) = setup("awd74_write_token").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (cal, cal_browser) = add_person(&setup, &[&work], "cal").await;
    let task = owner_task(&setup, &work, "Not Bea's to edit").await;
    let (status, writers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Page writers", "baseRole": "member", "permissions": ["tasks.view", "pages.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{writers}");
    let (status, readers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Readers", "baseRole": "viewer", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{readers}");
    for (actor, role, key) in [
        (bea, &writers, "bea-writers"),
        (cal, &readers, "cal-readers"),
    ] {
        let (status, changed) =
            set_project_role(&setup, &work, actor, key, json!({"roleId": role["id"]})).await;
        assert_eq!(status, StatusCode::OK, "{changed}");
    }
    let token_request = json!({"projectId": work, "name": "script", "scopes": ["read", "write"], "expiresInDays": 30});

    // Bea may edit pages: she gets a write token, which edits pages only.
    let (status, issued) = send(
        app,
        bea_browser.request(
            "POST",
            "/api/v1/account/api-tokens",
            Some("bea-write-token"),
            token_request.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let token = issued["token"].as_str().expect("token").to_owned();
    let (status, page) = public_call(
        app,
        &token,
        "page_write",
        json!({"projectId": work, "title": "Written by the token", "body": "Hello"}),
    )
    .await;
    assert!(status.is_success(), "{status}: {page}");
    let pages = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM project_pages WHERE title = 'Written by the token'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count pages");
    assert_eq!(pages, 1, "{page}");
    let (_, refused) = public_call(
        app,
        &token,
        "task_update",
        json!({"taskId": task, "title": "Renamed by the token", "expectedVersion": 1}),
    )
    .await;
    assert_ne!(refused["outcome"], "applied", "{refused}");
    let renamed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_rows WHERE title = 'Renamed by the token'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count tasks");
    assert_eq!(renamed, 0, "the token edited a task: {refused}");

    // Cal may only see things: no write token.
    let (status, refused) = send(
        app,
        cal_browser.request(
            "POST",
            "/api/v1/account/api-tokens",
            Some("cal-write-token"),
            token_request,
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "a read-only role got a write token: {refused}"
    );
}

/// Changing a task doesn't reveal it: someone who may archive tasks but not
/// see them gets the outcome, id and version from archive and restore
/// (including an unchanged restore and a version conflict), never the
/// title. Someone who sees tasks still gets the whole task.
#[tokio::test]
async fn archiving_without_see_tasks_reveals_no_task() {
    let Some(setup) = setup("awd74_archive_view").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret archive plan").await;
    let (status, archivers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Archivers", "baseRole": "member", "permissions": ["tasks.delete"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{archivers}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-archivers",
        json!({"roleId": archivers["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let act = |browser: &Browser, action: &str, key: &str, expected: i64| {
        browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items/{task}/{action}"),
            Some(key),
            json!({"expectedVersion": expected}),
        )
    };

    let (status, archived) = send(
        app,
        act(
            &bea_browser,
            "archive",
            "bea-archive",
            task_version(&setup, project, task).await,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    assert!(
        !archived.to_string().contains("Secret archive plan"),
        "{archived}"
    );
    let stale = task_version(&setup, project, task).await - 1;
    let (status, conflict) = send(app, act(&bea_browser, "restore", "bea-stale", stale)).await;
    assert!(!status.is_success(), "{status}: {conflict}");
    assert!(
        !conflict.to_string().contains("Secret archive plan"),
        "{conflict}"
    );
    let (status, restored) = send(
        app,
        act(
            &bea_browser,
            "restore",
            "bea-restore",
            task_version(&setup, project, task).await,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert!(
        !restored.to_string().contains("Secret archive plan"),
        "{restored}"
    );
    let (status, unchanged) = send(
        app,
        act(
            &bea_browser,
            "restore",
            "bea-again",
            task_version(&setup, project, task).await,
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {unchanged}");
    assert!(
        !unchanged.to_string().contains("Secret archive plan"),
        "{unchanged}"
    );

    // The owner sees tasks: the whole task comes back.
    let (status, archived) = send(
        app,
        act(
            &setup.browser,
            "archive",
            "owner-archive",
            task_version(&setup, project, task).await,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!(archived["title"], "Secret archive plan", "{archived}");
}

/// A rule run by hand loads a task only if its maker may see tasks there
/// now: "run now" goes through the same check as an event. A role that may
/// edit tasks but not see them gets a skipped run, never the task's title;
/// with "see tasks" the title reaches the notification.
#[tokio::test]
async fn a_rule_run_by_hand_shows_only_tasks_its_maker_sees() {
    let Some(setup) = setup("awd74_rule_run").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret rule plan").await;
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Blind editors", "baseRole": "member", "permissions": ["tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-blind",
        json!({"roleId": blind["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let (status, rule) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/automations"),
            None,
            json!({
                "name": "Tell me",
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "notify", "recipients": "owner", "message": "Task: {{task.title}}"}],
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {rule}");
    let rule_id = rule["id"].as_str().expect("rule").to_owned();
    let run_path = format!("/api/v1/projects/{work}/automations/{rule_id}/run");

    // Without "see tasks": skipped, and the title goes nowhere.
    let (status, run) = send(
        app,
        bea_browser.request("POST", &run_path, None, json!({"workItemId": task})),
    )
    .await;
    assert!(status.is_success(), "{status}: {run}");
    assert_eq!(run["state"], "skipped", "{run}");
    assert!(!run.to_string().contains("Secret rule plan"), "{run}");
    assert_eq!(notified_of(&setup, bea, "Secret rule plan").await, 0);

    // With it, the same run tells her the title.
    let (status, seeing) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Seeing editors", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{seeing}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-seeing",
        json!({"roleId": seeing["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let (status, run) = send(
        app,
        bea_browser.request("POST", &run_path, None, json!({"workItemId": task})),
    )
    .await;
    assert!(status.is_success(), "{status}: {run}");
    assert_eq!(run["state"], "succeeded", "{run}");
    assert_eq!(notified_of(&setup, bea, "Secret rule plan").await, 1);
}

/// A rule made in the browser and run by hand through the maker's app keeps
/// to that app's limit: without "see tasks" there the run is skipped and the
/// title goes nowhere. The same run from the browser tells her the title.
#[tokio::test]
async fn a_browser_rule_run_through_an_app_keeps_to_the_apps_limit() {
    let Some(setup) = setup("awd74_rule_app_run").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret app plan").await;
    let (status, rule) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/automations"),
            None,
            json!({
                "name": "Tell me",
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "notify", "recipients": "owner", "message": "Task: {{task.title}}"}],
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {rule}");
    let rule_id = rule["id"].as_str().expect("rule").to_owned();
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    // Her apps may change tasks but not see them.
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["tasks.edit"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");

    let ran = tool(
        app,
        &bea_app,
        &session,
        "automation_write",
        json!({"projectId": work, "operation": "run", "ruleId": rule_id, "taskId": task}),
    )
    .await;
    assert_eq!(ran["outcome"], "ok", "{ran}");
    assert_eq!(ran["value"]["state"], "skipped", "{ran}");
    assert!(!ran.to_string().contains("Secret app plan"), "{ran}");
    assert_eq!(notified_of(&setup, bea, "Secret app plan").await, 0);

    let (status, run) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/automations/{rule_id}/run"),
            None,
            json!({"workItemId": task}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {run}");
    assert_eq!(run["state"], "succeeded", "{run}");
    assert_eq!(notified_of(&setup, bea, "Secret app plan").await, 1);
}

/// "Apply to their projects" changes someone's role only in projects where
/// the caller may manage members: a workspace admin whose custom role in
/// one project lacks "manage members" leaves that project as it was, and
/// it's reported as skipped; where they keep it, the role is applied.
#[tokio::test]
async fn applying_a_role_to_projects_skips_those_the_caller_cant_manage() {
    let Some(setup) = setup("awd74_apply_projects").await else {
        return;
    };
    let closed = create_project(&setup, "SHUT", "Shut").await;
    let open = create_project(&setup, "OPEN", "Open").await;
    let (alex, alex_browser) = add_person(&setup, &[&closed, &open], "alex").await;
    let (bea, _) = add_person(&setup, &[&closed, &open], "bea").await;
    make_admin(&setup, alex).await;
    let (status, no_members) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "No members", "baseRole": "admin", "permissions": ["tasks.view", "members.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{no_members}");
    let (status, helpers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Helpers", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{helpers}");
    // Alex manages members in Open (built-in admin) but not in Shut.
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(open.parse::<Uuid>().expect("project"))
    .bind(alex)
    .execute(&setup.pool)
    .await
    .expect("alex is an admin in Open");
    sqlx::query(
        "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(closed.parse::<Uuid>().expect("project"))
    .bind(alex)
    .bind(
        no_members["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .execute(&setup.pool)
    .await
    .expect("alex doesn't manage members in Shut");

    let (status, assigned) = assign(&setup, &alex_browser, bea, &helpers["id"], true).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(assigned["projectsChanged"], 1, "{assigned}");
    assert_eq!(assigned["projectsSkipped"], 1, "{assigned}");
    assert_eq!(
        assigned["skippedProjects"][0]["projectId"],
        closed.as_str(),
        "{assigned}"
    );
    assert!(
        assigned["skippedProjects"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("manage members")),
        "{assigned}"
    );
    assert_eq!(project_role_id(&setup, &closed, bea).await, None);
    assert_eq!(
        project_role_id(&setup, &open, bea)
            .await
            .map(|id| id.to_string()),
        helpers["id"].as_str().map(str::to_owned)
    );
}

/// Messages follow the workspace role as it is now: someone who opened
/// Messages as the owner, then handed ownership on and was given a custom
/// role without "post messages", can't send a direct message; the person
/// they talk with, who may post, still can.
#[tokio::test]
async fn a_former_owner_without_post_messages_cant_send_direct_messages() {
    let Some(setup) = setup("awd74_former_owner_dm").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (cal, cal_browser) = add_person(&setup, &[&work], "cal").await;
    let owner = owner_id(&setup.pool).await;
    let (status, quiet) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Quiet", "baseRole": "member", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{quiet}");

    // The owner opens Messages and a DM with Bea while still the owner.
    direct_space(&setup).await;
    let (status, _) = send(
        app,
        bea_browser.request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, dm) = send(
        app,
        setup
            .browser
            .request("POST", "/api/v1/direct/dms", None, json!({"actorId": bea})),
    )
    .await;
    assert!(status.is_success(), "{status}: {dm}");
    let dm = dm["conversation"]["id"].as_str().expect("DM").to_owned();

    // Ownership goes to Cal, who gives the former owner a role without
    // "post messages".
    let (status, transferred) = send(
        app,
        setup.browser.request(
            "POST",
            &format!(
                "/api/v1/organizations/{}/transfer-ownership",
                setup.organization_id
            ),
            Some("transfer-to-cal"),
            json!({"actorId": cal}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transferred}");
    let (status, assigned) = assign(&setup, &cal_browser, owner, &quiet["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");

    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{dm}/messages"),
            Some("former-owner-dm"),
            json!({"content": text("Still the owner here?")}),
        ),
    )
    .await;
    assert!(!status.is_success(), "{status}: {refused}");
    let sent = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1::uuid AND plain_text = 'Still the owner here?'",
    )
    .bind(&dm)
    .fetch_one(&setup.pool)
    .await
    .expect("count messages");
    assert_eq!(sent, 0, "the former owner posted: {refused}");

    // Bea may post, and still can.
    say(&setup, &bea_browser, &dm, "bea-dm", text("Hello from Bea")).await;
}

/// Cancelling a launch returns it the way the list does: someone who queued
/// a launch and then lost "see tasks" gets it back without the task's key
/// or title; with "see tasks" the task is there.
#[tokio::test]
async fn cancelling_a_launch_without_see_tasks_reveals_no_task() {
    let Some(setup) = setup("awd74_launch_cancel").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret launch plan").await;
    // Two launches Bea queued, waiting on the task's blockers.
    let mut launches = Vec::new();
    for _ in 0..2 {
        let launch = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO launches (
                id, organization_id, project_id, work_item_id, requested_by_actor_id,
                source, start_when, position
            ) VALUES ($1, $2, $3, $4, $5, 'button', 'blockers_done',
                      (SELECT COALESCE(max(position), 0) + 1 FROM launches WHERE project_id = $3))
            "#,
        )
        .bind(launch)
        .bind(setup.organization_id)
        .bind(project)
        .bind(task)
        .bind(bea)
        .execute(&setup.pool)
        .await
        .expect("queued launch");
        launches.push(launch);
    }

    // Without "see tasks".
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Launchers", "baseRole": "member", "permissions": ["runs.launch"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-launchers",
        json!({"roleId": blind["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let (status, cancelled) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/launches/{}/cancel", launches[0]),
            Some("bea-cancel-blind"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    assert!(cancelled["workItem"].is_null(), "{cancelled}");
    assert!(
        !cancelled.to_string().contains("Secret launch plan"),
        "{cancelled}"
    );

    // With it.
    let (status, seeing) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Seeing launchers", "baseRole": "member", "permissions": ["tasks.view", "runs.launch"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{seeing}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-seeing-launchers",
        json!({"roleId": seeing["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let (status, cancelled) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/launches/{}/cancel", launches[1]),
            Some("bea-cancel-seeing"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(
        cancelled["workItem"]["title"], "Secret launch plan",
        "{cancelled}"
    );
}

/// Inviting takes "invite members", not "manage members": an admin whose
/// custom role may invite but not manage members is shown, per role,
/// which ones they may invite someone to (those within what they hold),
/// while giving roles to members stays off.
#[tokio::test]
async fn an_invite_only_admin_sees_which_roles_they_may_invite_to() {
    let Some(setup) = setup("awd74_invite_roles").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let mut created = Vec::new();
    for body in [
        json!({"name": "Inviters", "baseRole": "admin", "permissions": ["members.invite", "members.view", "tasks.view"]}),
        json!({"name": "Readers", "baseRole": "member", "permissions": ["tasks.view"]}),
        json!({"name": "Editors", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    ] {
        let (status, role) = create_role(&setup, &setup.browser, body).await;
        assert_eq!(status, StatusCode::CREATED, "{role}");
        created.push(role);
    }
    let (status, assigned) = assign(&setup, &setup.browser, alex, &created[0]["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");

    let listed = roles(&setup, &alex_browser).await;
    let capabilities =
        |key: &Value| role(&listed, key.as_str().expect("key"))["capabilities"].clone();
    // Within what Alex holds: may invite to it, not give it.
    let readers = capabilities(&created[1]["key"]);
    assert_eq!(readers["invite"], true, "{listed}");
    assert_eq!(readers["assign"], false, "{listed}");
    // Beyond it ("edit tasks", and built-in Member's wider set): neither.
    assert_eq!(
        capabilities(&created[2]["key"])["invite"],
        false,
        "{listed}"
    );
    assert_eq!(
        role(&listed, "member")["capabilities"]["invite"],
        false,
        "{listed}"
    );
    // Admin-based roles take an owner.
    assert_eq!(
        role(&listed, "admin")["capabilities"]["invite"],
        false,
        "{listed}"
    );
    assert_eq!(
        capabilities(&created[0]["key"])["invite"],
        false,
        "{listed}"
    );
}

/// Rotating an agent's credentials hands out the role it holds, custom ones
/// included, not its base role: an admin without "edit tasks" may rotate
/// them for an agent whose custom role leaves "edit tasks" out too, but not
/// for an agent holding plain Member.
#[tokio::test]
async fn rotating_an_agent_checks_the_role_it_holds() {
    let Some(setup) = setup("awd74_agent_rotate").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let (status, keepers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Agent keepers", "baseRole": "admin", "permissions": ["agents.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{keepers}");
    let (status, assigned) = assign(&setup, &setup.browser, alex, &keepers["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    sqlx::query(
        "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .bind(
        keepers["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .execute(&setup.pool)
    .await
    .expect("alex keeps agents in the project too");

    // Mara holds a custom role without "edit tasks"; Rook holds Member.
    let (mara, _) = create_agent(app, &setup.browser, &work, "Mara", "mara").await;
    let (rook, _) = create_agent(app, &setup.browser, &work, "Rook", "rook").await;
    let (status, quiet) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Quiet agents", "baseRole": "member", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{quiet}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        mara,
        "mara-quiet",
        json!({"roleId": quiet["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");

    let rotate = |connection: Uuid, version: i64| {
        alex_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/external-agents/{connection}/credentials/rotate"),
            None,
            json!({"expectedVersion": version}),
        )
    };
    let (connection, version, _) = agent_connection(&setup, mara).await;
    let (status, rotated) = send(app, rotate(connection, version)).await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let (connection, version, _) = agent_connection(&setup, rook).await;
    let (status, refused) = send(app, rotate(connection, version)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
}

/// An agent works in a project, so what it's given must be within the
/// giver's role there too: a workspace Admin whose role in this project
/// keeps "manage agents" but leaves out editing tasks and seeing code can't
/// create a Member agent here or rotate a Member agent's credentials, and
/// may rotate them for an agent whose role is within theirs.
#[tokio::test]
async fn agents_are_given_only_what_the_giver_holds_in_the_project() {
    let Some(setup) = setup("awd74_agent_project_grant").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    make_admin(&setup, alex).await;
    let (status, keepers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Agent keepers here", "baseRole": "admin", "permissions": ["agents.manage", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{keepers}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .bind(
        keepers["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .execute(&setup.pool)
    .await
    .expect("alex keeps only agents and tasks in the project");
    let agents = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM external_agent_connections WHERE project_id = $1",
        )
        .bind(project)
        .fetch_one(&pool)
        .await
        .expect("agents")
    };

    // A Member agent here holds more than Alex's role here.
    let (status, refused) = send(
        app,
        alex_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/external-agents"),
            None,
            json!({
                "connectionName": "Ivy",
                "displayName": "Ivy",
                "handle": "ivy",
                "role": "member",
                "scopes": ["read", "write", "events"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused.get("oneTimeCredential").is_none(), "{refused}");
    assert_eq!(agents(setup.pool.clone()).await, 0, "no agent was made");

    // The owner's agents: Rook holds Member, Mara a role within Alex's.
    let (rook, _) = create_agent(app, &setup.browser, &work, "Rook", "rook").await;
    let (mara, _) = create_agent(app, &setup.browser, &work, "Mara", "mara").await;
    let (status, readers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Reading agents", "baseRole": "member", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{readers}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        mara,
        "mara-reading",
        json!({"roleId": readers["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let rotate = |connection: Uuid, version: i64| {
        alex_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/external-agents/{connection}/credentials/rotate"),
            None,
            json!({"expectedVersion": version}),
        )
    };
    let (connection, version, _) = agent_connection(&setup, rook).await;
    let (status, refused) = send(app, rotate(connection, version)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused.get("oneTimeCredential").is_none(), "{refused}");
    let (connection, version, _) = agent_connection(&setup, mara).await;
    let (status, rotated) = send(app, rotate(connection, version)).await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
}

/// Changing someone's project role is held to what the caller holds in the
/// project, not only in the workspace: a workspace Admin whose role here
/// keeps "manage members" but leaves out editing tasks and seeing code can't
/// give Bea a role with more than that here, and may give her one within it.
#[tokio::test]
async fn project_roles_are_given_only_within_the_givers_project_role() {
    let Some(setup) = setup("awd74_member_project_grant").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let (bea, _) = add_person(&setup, &[&work], "bea").await;
    make_admin(&setup, alex).await;
    let role_id = |role: &Value| {
        role["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID")
    };
    let (status, keepers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Member keepers here", "baseRole": "admin", "permissions": ["members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{keepers}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .bind(role_id(&keepers))
    .execute(&setup.pool)
    .await
    .expect("alex keeps only members and tasks in the project");
    let (status, workers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Workers", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit", "code.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{workers}");
    let (status, readers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Readers", "baseRole": "member", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{readers}");
    let give = |role: &Value, version: i64, key: &str| {
        alex_browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{bea}"),
            Some(key),
            json!({"roleId": role["id"], "expectedVersion": version}),
        )
    };

    // More than Alex holds here: refused, and Bea's role stays.
    let (version, _) = project_membership(&setup, project, bea).await;
    let (status, refused) = send(app, give(&workers, version, "alex-gives-workers")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(project_role_id(&setup, &work, bea).await, None);

    // Within it: given.
    let (status, given) = send(app, give(&readers, version, "alex-gives-readers")).await;
    assert_eq!(status, StatusCode::OK, "{given}");
    assert_eq!(
        project_role_id(&setup, &work, bea).await,
        Some(role_id(&readers))
    );
}

/// Applying a workspace role to someone's projects is held, in each
/// project, to the caller's role there: a workspace Admin whose role in
/// Work keeps "manage members" but leaves out editing tasks and seeing code
/// gives Bea a fuller role in the workspace, but Work is skipped; a role
/// within theirs is applied there.
#[tokio::test]
async fn applying_a_role_to_projects_stays_within_the_callers_project_role() {
    let Some(setup) = setup("awd74_apply_project_grant").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let (bea, _) = add_person(&setup, &[&work], "bea").await;
    make_admin(&setup, alex).await;
    let role_id = |role: &Value| {
        role["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID")
    };
    let (status, keepers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Member keepers in Work", "baseRole": "admin", "permissions": ["members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{keepers}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .bind(role_id(&keepers))
    .execute(&setup.pool)
    .await
    .expect("alex keeps only members and tasks in Work");
    let (status, workers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Workers", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit", "code.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{workers}");
    let (status, readers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Readers", "baseRole": "member", "permissions": ["tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{readers}");

    // More than Alex holds in Work: the workspace role changes, Work doesn't.
    let (status, assigned) = assign(&setup, &alex_browser, bea, &workers["id"], true).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(assigned["projectsChanged"], 0, "{assigned}");
    assert_eq!(assigned["projectsSkipped"], 1, "{assigned}");
    assert!(
        assigned
            .to_string()
            .contains("more than your role in this project"),
        "{assigned}"
    );
    assert_eq!(project_role_id(&setup, &work, bea).await, None);

    // Within it: applied in Work too.
    let (status, assigned) = assign(&setup, &alex_browser, bea, &readers["id"], true).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(assigned["projectsChanged"], 1, "{assigned}");
    assert_eq!(
        project_role_id(&setup, &work, bea).await,
        Some(role_id(&readers))
    );
}

/// An outstanding project invitation is rechecked against its issuer's role
/// in the project when it's opened or accepted: once the issuer's role there
/// narrows below the invited role (while they may still invite), the token
/// no longer opens.
#[tokio::test]
async fn a_project_invitation_lapses_when_its_issuers_project_role_narrows_below_it() {
    let Some(setup) = setup("awd74_invite_project_grant").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let elsewhere = create_project(&setup, "ELSE", "Elsewhere").await;
    let (_, bea_browser) = add_person(&setup, &[&elsewhere], "bea").await;
    make_admin(&setup, alex).await;
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .execute(&setup.pool)
    .await
    .expect("alex is a project admin");
    let (status, issued) = send(
        app,
        alex_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/invitations"),
            None,
            json!({"role": "member", "expiresInHours": 24}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let token = invitation_token(&issued);
    let resolve = |token: &str| {
        setup.browser.request(
            "POST",
            "/api/v1/project-invitations/resolve",
            None,
            json!({"token": token}),
        )
    };
    let (status, resolved) = send(app, resolve(&token)).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");

    // Alex may still invite in Work, but no longer edit tasks or see code.
    let (status, narrowed) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Inviters only", "baseRole": "admin", "permissions": ["members.invite", "members.manage", "members.view", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{narrowed}");
    sqlx::query(
        "UPDATE project_memberships SET role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(alex)
    .bind(
        narrowed["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .execute(&setup.pool)
    .await
    .expect("narrow alex in the project");
    assert_eq!(workspace_role(&setup, alex).await, "admin");

    let (status, refused) = send(app, resolve(&token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    let (status, refused) = send(
        app,
        bea_browser.request(
            "POST",
            "/api/v1/project-invitations/accept-existing",
            None,
            json!({"token": token, "handle": "bea"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
}

/// Working on a task with a worker makes a thread named after the task, so
/// it needs "see tasks": a role that may edit tasks and start runs but not
/// see tasks is refused before any thread is made.
#[tokio::test]
async fn working_with_a_worker_needs_see_tasks() {
    let Some(setup) = setup("awd74_work_with_worker").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let channel = create_channel(&setup, &work, "build").await;
    let task = owner_task(&setup, &work, "Secret worker task").await;
    let (status, doers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Blind doers", "baseRole": "member", "permissions": ["tasks.edit", "runs.launch", "channels.post"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doers}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-blind-doers",
        json!({"roleId": doers["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let version = task_version(&setup, project, task).await;
    let (status, refused) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items/{task}/work-with-worker"),
            Some("bea-work-with-worker"),
            json!({"expectedVersion": version, "channelId": channel, "prompt": "Do it."}),
        ),
    )
    .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status}: {refused}"
    );
    assert!(
        !refused.to_string().contains("Secret worker task"),
        "{refused}"
    );
    let threads: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversations WHERE project_id = $1 AND strpos(name, 'Secret worker task') > 0",
    )
    .bind(project)
    .fetch_one(&setup.pool)
    .await
    .expect("threads");
    assert_eq!(threads, 0, "no thread named after the task");
}

/// Preparing or archiving a repository answers with the repository (owner,
/// name, clone URL, branch), so it needs "see code" as well as "manage code":
/// a role that manages code without seeing it is refused, and the repository
/// is left as it was.
#[tokio::test]
async fn managing_a_repository_needs_see_code() {
    let Some(setup) = setup("awd74_repository_manage").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let repository = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, trusted, preparation_state, prepared_at
        ) VALUES ($1, $2, 474747, 'Mischa-dev', 'managed-secret',
                  'https://github.com/Mischa-dev/managed-secret.git',
                  'main', $3, true, 'ready', now())
        "#,
    )
    .bind(repository)
    .bind(project)
    .bind(format!("/tmp/agent-workspace-test-{repository}.git"))
    .execute(&setup.pool)
    .await
    .expect("repository");
    let (status, keepers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Blind code keepers", "baseRole": "admin", "permissions": ["code.manage", "tasks.view"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{keepers}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin', role_id = $3 WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(bea)
    .bind(
        keepers["id"]
            .as_str()
            .expect("id")
            .parse::<Uuid>()
            .expect("UUID"),
    )
    .execute(&setup.pool)
    .await
    .expect("bea manages code without seeing it");
    let base = format!("/api/v1/projects/{work}/repositories/{repository}");
    let version: i64 = sqlx::query_scalar("SELECT version FROM repositories WHERE id = $1")
        .bind(repository)
        .fetch_one(&setup.pool)
        .await
        .expect("repository version");
    for (path, key, body) in [
        (format!("{base}/prepare"), "bea-prepare", json!({})),
        (
            format!("{base}/archive"),
            "bea-archive",
            json!({"expectedVersion": version}),
        ),
    ] {
        let (status, refused) =
            send(app, bea_browser.request("POST", &path, Some(key), body)).await;
        assert!(
            matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{path}: {status} {refused}"
        );
        assert!(
            !refused.to_string().contains("managed-secret"),
            "{path}: {refused}"
        );
    }
    let archived: bool =
        sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM repositories WHERE id = $1")
            .bind(repository)
            .fetch_one(&setup.pool)
            .await
            .expect("repository state");
    assert!(!archived, "the repository was left as it was");
}

/// A comment that references another task shows that task's title and
/// status only to someone who may see tasks: posting or editing it with a
/// role that may edit but not see tasks returns the reference by id and
/// key alone; with "see tasks" the title and status come back.
#[tokio::test]
async fn comment_references_show_titles_only_with_see_tasks() {
    let Some(setup) = setup("awd74_comment_refs").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let on = owner_task(&setup, &work, "Commented task").await;
    let secret = owner_task(&setup, &work, "Secret referenced task").await;
    let (status, editors) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Commenters", "baseRole": "member", "permissions": ["tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{editors}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-commenters",
        json!({"roleId": editors["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let content = |text: &str| {
        json!({"version": 1, "nodes": [
            {"type": "text", "text": text},
            {"type": "work_item_ref", "workItemId": secret}
        ]})
    };
    let comments = format!("/api/v1/projects/{work}/work-items/{on}/comments");

    // Without "see tasks": the reference, by id and key only.
    let (status, posted) = send(
        app,
        bea_browser.request(
            "POST",
            &comments,
            Some("bea-comment-blind"),
            json!({"content": content("See ")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{posted}");
    let reference = &posted["comment"]["referencedWorkItems"][0];
    assert_eq!(reference["id"], secret.to_string(), "{posted}");
    assert_eq!(reference["title"], "", "{posted}");
    assert_eq!(reference["statusCategory"], "", "{posted}");
    assert!(
        !posted.to_string().contains("Secret referenced task"),
        "{posted}"
    );
    let comment_id = posted["comment"]["id"]
        .as_str()
        .expect("comment")
        .to_owned();
    let (status, edited) = send(
        app,
        bea_browser.request(
            "PATCH",
            &format!("{comments}/{comment_id}"),
            None,
            json!({"content": content("Also see "), "expectedRevision": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(
        edited["comment"]["referencedWorkItems"][0]["title"], "",
        "{edited}"
    );
    assert!(
        !edited.to_string().contains("Secret referenced task"),
        "{edited}"
    );

    // AWD-95: replying to someone else's comment quotes it only for those
    // who may see tasks; Bea gets its id, without its author or words, now
    // and on a retry.
    let (status, owners) = send(
        app,
        setup.browser.request(
            "POST",
            &comments,
            Some("owner-private-words"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Budget is 40k"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{owners}");
    let owners = owners["comment"]["id"]
        .as_str()
        .expect("owner comment")
        .to_owned();
    for _ in 0..2 {
        let (status, replied) = send(
            app,
            bea_browser.request(
                "POST",
                &comments,
                Some("bea-blind-reply"),
                json!({"content": content("Noted, "), "replyToCommentId": owners}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{replied}");
        assert_eq!(replied["comment"]["replyTo"]["commentId"], owners.as_str());
        assert!(replied["comment"]["replyTo"]["text"].is_null(), "{replied}");
        assert!(
            replied["comment"]["replyTo"]["authorName"].is_null(),
            "{replied}"
        );
        assert!(!replied.to_string().contains("40k"), "{replied}");
    }

    // With "see tasks": the title and status come back.
    let (status, seeing) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Seeing commenters", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{seeing}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-seeing-commenters",
        json!({"roleId": seeing["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let (status, posted) = send(
        app,
        bea_browser.request(
            "POST",
            &comments,
            Some("bea-comment-seeing"),
            json!({"content": content("Now see ")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{posted}");
    let reference = &posted["comment"]["referencedWorkItems"][0];
    assert_eq!(reference["title"], "Secret referenced task", "{posted}");
    assert_ne!(reference["statusCategory"], "", "{posted}");
}

/// AI suggestions are about tasks, so seeing one takes "see tasks" in its
/// project: someone whose role there keeps "edit tasks" but not "see
/// tasks" can't list them there, and deciding a known one is refused as
/// not found, without its task, value or reason. With "see tasks" both work.
#[tokio::test]
async fn ai_suggestions_need_see_tasks() {
    let Some(setup) = setup("awd74_ai_suggestions").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "ELSE", "Elsewhere").await;
    let project = work.parse::<Uuid>().expect("project");
    // Bea sees tasks in Elsewhere, so her app may use the suggestion tools.
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    let first = owner_task(&setup, &work, "Suggested task").await;
    let second = owner_task(&setup, &work, "Related task").await;
    let suggestion = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO ai_suggestions (
            id, organization_id, project_id, work_item_id, kind,
            other_project_id, other_work_item_id, confidence, reason, sources,
            pair_key, origin, provider, model, prompt_id, prompt_version
        )
        VALUES ($1, $2, $3, $4, 'relates_to', $3, $5, 0.7, 'Secret suggestion reason',
                '[]'::jsonb, $6, 'analyze', 'fake', 'rules', 'dependency_judge', 1)
        "#,
    )
    .bind(suggestion)
    .bind(setup.organization_id)
    .bind(project)
    .bind(first)
    .bind(second)
    .bind(format!("awd74:{suggestion}"))
    .execute(&setup.pool)
    .await
    .expect("suggestion");
    let (status, editors) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Blind editors", "baseRole": "member", "permissions": ["tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{editors}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-blind",
        json!({"roleId": editors["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");

    // Without "see tasks" in Work: listing there is refused (it reads tasks),
    // and deciding is refused as not found.
    let listed = tool(
        app,
        &bea_app,
        &session,
        "ai_suggestions_list",
        json!({"projectId": work}),
    )
    .await;
    assert_ne!(listed["outcome"], "ok", "{listed}");
    assert!(
        !listed.to_string().contains("Secret suggestion reason"),
        "{listed}"
    );
    let decided = tool(
        app,
        &bea_app,
        &session,
        "ai_suggestion_decide",
        json!({"projectId": work, "suggestionId": suggestion, "decision": "dismiss"}),
    )
    .await;
    assert_eq!(decided["outcome"], "not_found", "{decided}");
    assert!(
        !decided.to_string().contains("Secret suggestion reason")
            && !decided.to_string().contains("Suggested task"),
        "{decided}"
    );
    let still = sqlx::query_scalar::<_, String>("SELECT status FROM ai_suggestions WHERE id = $1")
        .bind(suggestion)
        .fetch_one(&setup.pool)
        .await
        .expect("suggestion status");
    assert_eq!(still, "pending");

    // With it: listed, and deciding returns the whole suggestion.
    let (status, seeing) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Seeing editors", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{seeing}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-seeing",
        json!({"roleId": seeing["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let listed = tool(
        app,
        &bea_app,
        &session,
        "ai_suggestions_list",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(
        listed["value"]["suggestions"][0]["reason"], "Secret suggestion reason",
        "{listed}"
    );
    let decided = tool(
        app,
        &bea_app,
        &session,
        "ai_suggestion_decide",
        json!({"projectId": work, "suggestionId": suggestion, "decision": "dismiss"}),
    )
    .await;
    assert_eq!(decided["outcome"], "ok", "{decided}");
    assert_eq!(
        decided["value"]["suggestion"]["reason"], "Secret suggestion reason",
        "{decided}"
    );
}

/// A bulk change returns its cards the way a single task's change does:
/// someone who may archive tasks but not see them gets each card's id,
/// key, version, type and status, not its title, assignee or other
/// contents; with "see tasks" the whole card comes back.
#[tokio::test]
async fn bulk_changes_without_see_tasks_reveal_no_task() {
    let Some(setup) = setup("awd74_bulk_cards").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret bulk plan").await;
    sqlx::query("UPDATE work_items SET assignee_actor_id = $3 WHERE project_id = $1 AND id = $2")
        .bind(project)
        .bind(task)
        .bind(owner_id(&setup.pool).await)
        .execute(&setup.pool)
        .await
        .expect("assign the task");
    let (status, archivers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Archivers", "baseRole": "member", "permissions": ["tasks.delete"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{archivers}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-archivers",
        json!({"roleId": archivers["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let bulk = format!("/api/v1/projects/{work}/work-items/bulk");

    // Restoring a task that's already active changes nothing and returns
    // its card: without "see tasks", only what identifies it.
    let version = task_version(&setup, project, task).await;
    let (status, restored) = send(
        app,
        bea_browser.request(
            "POST",
            &bulk,
            Some("bea-bulk-blind"),
            json!({"items": [{"id": task, "expectedVersion": version}], "changes": {"archived": false}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    let card = &restored["items"][0];
    assert_eq!(card["id"], task.to_string(), "{restored}");
    assert!(card["key"].is_string(), "{restored}");
    assert!(card["status"]["id"].is_string(), "{restored}");
    assert_eq!(card["title"], "", "{restored}");
    assert!(card["assignee"].is_null(), "{restored}");
    assert!(
        !restored.to_string().contains("Secret bulk plan"),
        "{restored}"
    );

    // With "see tasks", the whole card.
    let (status, seeing) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Seeing archivers", "baseRole": "member", "permissions": ["tasks.view", "tasks.delete"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{seeing}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-seeing-archivers",
        json!({"roleId": seeing["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let version = task_version(&setup, project, task).await;
    let (status, restored) = send(
        app,
        bea_browser.request(
            "POST",
            &bulk,
            Some("bea-bulk-seeing"),
            json!({"items": [{"id": task, "expectedVersion": version}], "changes": {"archived": false}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(
        restored["items"][0]["title"], "Secret bulk plan",
        "{restored}"
    );
    assert!(restored["items"][0]["assignee"].is_object(), "{restored}");
}

/// Moving a task again returns where it went, under its current title
/// there: someone who may create tasks in that project but not see them
/// gets its key, not a title changed after the move; with "see tasks"
/// there, the title comes back.
#[tokio::test]
async fn a_repeated_move_shows_the_title_only_where_tasks_are_seen() {
    let Some(setup) = setup("awd74_move_replay").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let dest = create_project(&setup, "DEST", "Dest").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &dest], "bea").await;
    let task = owner_task(&setup, &work, "Move me").await;
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let arguments = json!({"taskId": task.to_string(), "toProjectId": dest});
    let moved = tool(app, &bea_app, &session, "task_move", arguments.clone()).await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["moved"][0]["title"], "Move me", "{moved}");
    let destination = moved["value"]["moved"][0]["id"]
        .as_str()
        .expect("destination")
        .parse::<Uuid>()
        .expect("destination id");
    sqlx::query("UPDATE collection_records SET title = 'Renamed where it went' WHERE id = $1")
        .bind(destination)
        .execute(&setup.pool)
        .await
        .expect("rename the moved task");

    // In Dest, Bea may create and change tasks but not see them.
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Blind creators", "baseRole": "member", "permissions": ["tasks.edit", "tasks.create"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let (status, changed) = set_project_role(
        &setup,
        &dest,
        bea,
        "bea-blind-creators",
        json!({"roleId": blind["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let again = tool(app, &bea_app, &session, "task_move", arguments.clone()).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["created"], false, "{again}");
    assert_eq!(
        again["value"]["moved"][0]["id"],
        destination.to_string(),
        "{again}"
    );
    assert_eq!(again["value"]["moved"][0]["title"], "", "{again}");
    assert!(
        !again.to_string().contains("Renamed where it went"),
        "{again}"
    );

    // With "see tasks" in Dest, the current title.
    let (status, seeing) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Seeing creators", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit", "tasks.create"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{seeing}");
    let (status, changed) = set_project_role(
        &setup,
        &dest,
        bea,
        "bea-seeing-creators",
        json!({"roleId": seeing["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let seen = tool(app, &bea_app, &session, "task_move", arguments).await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    assert_eq!(
        seen["value"]["moved"][0]["title"], "Renamed where it went",
        "{seen}"
    );
}

/// A board's link to a task shows the task's title only to someone who
/// sees tasks in its project; without "see tasks" the link keeps its key
/// and the board stays readable.
#[tokio::test]
async fn board_links_show_task_titles_only_where_tasks_are_seen() {
    let Some(setup) = setup("awd74_board_links").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret linked plan").await;
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    let created = tool(
        app,
        &owner_app,
        &owner_session,
        "whiteboard_create",
        json!({
            "title": "Plan board",
            "projectId": work,
            "linkTo": {"targetKind": "task", "targetId": task.to_string()},
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let board = created["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let read = tool(
        app,
        &bea_app,
        &session,
        "whiteboard_read",
        json!({"whiteboardId": board}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(
        read["value"]["links"][0]["title"], "Secret linked plan",
        "{read}"
    );

    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "No tasks", "baseRole": "viewer", "permissions": []}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-no-tasks-board",
        json!({"roleId": blind["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let read = tool(
        app,
        &bea_app,
        &session,
        "whiteboard_read",
        json!({"whiteboardId": board}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let link = &read["value"]["links"][0];
    assert_eq!(link["targetId"], task.to_string(), "{read}");
    assert!(link["key"].is_string(), "{read}");
    assert_eq!(link["title"], "Task", "{read}");
    assert!(!read.to_string().contains("Secret linked plan"), "{read}");
}

/// A GitHub import is the repository's issues, so it follows "see code":
/// someone who may create tasks but not see code can't start one, sees an
/// existing one without its repository (by MCP and in the browser's list),
/// and can't commit it.
#[tokio::test]
async fn github_imports_follow_see_code() {
    let Some(setup) = setup("awd74_github_imports").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let repository = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, trusted, preparation_state, prepared_at
        ) VALUES ($1, $2, 434343, 'Mischa-dev', 'import-repo',
                  'https://github.com/Mischa-dev/import-repo.git',
                  'main', $3, true, 'ready', now())
        "#,
    )
    .bind(repository)
    .bind(project)
    .bind(format!("/tmp/agent-workspace-test-{repository}.git"))
    .execute(&setup.pool)
    .await
    .expect("repository");
    let start =
        json!({"projectId": work, "source": "github", "repositoryId": repository.to_string()});

    // The owner sees code: the import names its repository.
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    let started = tool(
        app,
        &owner_app,
        &owner_session,
        "task_import",
        start.clone(),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    assert_eq!(
        started["value"]["import"]["repositoryName"], "Mischa-dev/import-repo",
        "{started}"
    );
    let import = started["value"]["import"]["id"].clone();
    let import_id = import
        .as_str()
        .expect("import")
        .parse::<Uuid>()
        .expect("import id");
    // As if the fetch had read one issue: a draft with its preview.
    sqlx::query(
        "UPDATE task_imports SET status = 'draft', rows = $2, row_count = 1, version = version + 1 WHERE id = $1",
    )
    .bind(import_id)
    .bind(json!([{"externalId": "7", "title": "Secret issue seven"}]))
    .execute(&setup.pool)
    .await
    .expect("fetched");
    let owner_read = tool(
        app,
        &owner_app,
        &owner_session,
        "task_import",
        json!({"projectId": work, "importId": import}),
    )
    .await;
    assert!(
        owner_read.to_string().contains("Secret issue seven"),
        "{owner_read}"
    );

    // Bea may see and create tasks here, but not see code.
    let (status, importers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Importers", "baseRole": "member", "permissions": ["tasks.view", "tasks.create", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{importers}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-importers",
        json!({"roleId": importers["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");

    let refused = tool(app, &bea_app, &session, "task_import", start).await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(!refused.to_string().contains("import-repo"), "{refused}");
    let imports: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_imports WHERE project_id = $1")
            .bind(project)
            .fetch_one(&setup.pool)
            .await
            .expect("imports");
    assert_eq!(imports, 1, "Bea's GitHub import was not started");

    let read = tool(
        app,
        &bea_app,
        &session,
        "task_import",
        json!({"projectId": work, "importId": import}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert!(
        read["value"]["import"]["repositoryName"].is_null(),
        "{read}"
    );
    assert!(read["value"]["import"]["repositoryId"].is_null(), "{read}");
    assert!(!read.to_string().contains("import-repo"), "{read}");
    assert!(!read.to_string().contains("Secret issue seven"), "{read}");
    assert!(
        !read.to_string().contains(&repository.to_string()),
        "{read}"
    );

    let commit = tool(
        app,
        &bea_app,
        &session,
        "task_import",
        json!({"projectId": work, "importId": import, "commit": true}),
    )
    .await;
    assert_eq!(commit["outcome"], "not_found", "{commit}");

    let (status, listed) = send(
        app,
        bea_browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/imports"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        listed["imports"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    assert!(!listed.to_string().contains("import-repo"), "{listed}");

    // The browser's single read, change and commit of the same draft.
    let path = format!("/api/v1/projects/{work}/imports/{import_id}");
    let (status, one) = send(app, bea_browser.request("GET", &path, None, Value::Null)).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert!(!one.to_string().contains("import-repo"), "{one}");
    assert!(!one.to_string().contains("Secret issue seven"), "{one}");
    let version = one["version"].clone();
    let (status, changed) = send(
        app,
        bea_browser.request(
            "PATCH",
            &path,
            Some("bea-import-cancel"),
            json!({"expectedVersion": version, "cancel": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{changed}");
    let (status, committed) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("{path}/commit"),
            Some("bea-import-commit"),
            json!({"expectedVersion": version}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{committed}");
    let state: String = sqlx::query_scalar("SELECT status FROM task_imports WHERE id = $1")
        .bind(import_id)
        .fetch_one(&setup.pool)
        .await
        .expect("import status");
    assert_eq!(state, "draft", "Bea neither canceled nor committed it");

    // Markdown and CSV need no code, and keep no repository they're passed,
    // nor show one an older import stored.
    let (status, markdown) = send(
        app,
        bea_browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/imports"),
            Some("bea-markdown-import"),
            json!({"source": "markdown", "text": "- Plain task", "repositoryId": repository}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{markdown}");
    let markdown_id = markdown["id"]
        .as_str()
        .expect("markdown import")
        .parse::<Uuid>()
        .expect("markdown import id");
    let stored: Option<Uuid> =
        sqlx::query_scalar("SELECT repository_id FROM task_imports WHERE id = $1")
            .bind(markdown_id)
            .fetch_one(&setup.pool)
            .await
            .expect("markdown repository");
    assert_eq!(stored, None, "a Markdown import keeps no repository");
    sqlx::query("UPDATE task_imports SET repository_id = $2 WHERE id = $1")
        .bind(markdown_id)
        .bind(repository)
        .execute(&setup.pool)
        .await
        .expect("an older Markdown import with a repository");
    let (status, older) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/imports/{markdown_id}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{older}");
    assert!(older["repositoryName"].is_null(), "{older}");

    // Cal sees code as a person, but their app's limit leaves it out.
    let (cal, cal_browser) = add_person(&setup, &[&work], "cal").await;
    let (status, limit) = send(
        app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{cal}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": ["tasks.view", "tasks.create", "tasks.edit"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limit}");
    let cal_app = connect_app(app, &cal_browser, "Cal's app", "full").await;
    let cal_session = initialize(app, &cal_app).await.expect("Cal's app");
    let refused = tool(
        app,
        &cal_app,
        &cal_session,
        "task_import",
        json!({"projectId": work, "source": "github", "repositoryId": repository.to_string()}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let through_app = tool(
        app,
        &cal_app,
        &cal_session,
        "task_import",
        json!({"projectId": work, "importId": import}),
    )
    .await;
    assert_eq!(through_app["outcome"], "ok", "{through_app}");
    assert!(
        !through_app.to_string().contains("import-repo"),
        "{through_app}"
    );
    let (status, as_person) = send(app, cal_browser.request("GET", &path, None, Value::Null)).await;
    assert_eq!(status, StatusCode::OK, "{as_person}");
    assert_eq!(
        as_person["repositoryName"], "Mischa-dev/import-repo",
        "{as_person}"
    );
}

/// The workspace member directory follows "See the workspace member
/// directory" (directory.view). Built-in roles hold it, so a plain Admin
/// and a plain Member see everyone, as before, while a plain Member still
/// can't open a project's member list (members.view). A custom role isn't
/// widened to it: without it, its holders see only themselves, whether it's
/// based on Member or on an Admin who kept "manage members", until someone
/// who manages roles gives it.
#[tokio::test]
async fn the_workspace_member_directory_follows_directory_view() {
    let Some(setup) = setup("awd74_workspace_directory").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (alex, alex_browser) = add_person(&setup, &[&work], "alex").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (cal, cal_browser) = add_person(&setup, &[&work], "cal").await;
    make_admin(&setup, alex).await;
    let path = format!("/api/v1/organizations/{}/members", setup.organization_id);
    let directory = |browser: &Browser| browser.request("GET", &path, None, Value::Null);
    let ids = |body: &Value| -> Vec<String> {
        body["members"]
            .as_array()
            .expect("members")
            .iter()
            .map(|member| member["actorId"].as_str().expect("actor").to_owned())
            .collect()
    };

    // Built-in roles: everyone, as before.
    for browser in [&alex_browser, &bea_browser] {
        let (status, everyone) = send(app, directory(browser)).await;
        assert_eq!(status, StatusCode::OK, "{everyone}");
        for actor in [alex, bea, cal] {
            assert!(ids(&everyone).contains(&actor.to_string()), "{everyone}");
        }
    }
    // A plain Member still can't open a project's member list.
    let (status, project_members) = send(
        app,
        bea_browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/members"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(!status.is_success(), "{status}: {project_members}");

    // A custom role based on Member, made without it, isn't given it.
    let (status, helpers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Helpers", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{helpers}");
    assert!(
        !keys(&helpers["permissions"]).contains(&"directory.view".to_owned()),
        "{helpers}"
    );
    let (status, assigned) = assign(&setup, &setup.browser, cal, &helpers["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let (status, own) = send(app, directory(&cal_browser)).await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(ids(&own), [cal.to_string()], "{own}");

    // So with an Admin's custom role that keeps "manage members" only.
    let (status, managers) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Managers", "baseRole": "admin", "permissions": ["members.manage"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{managers}");
    let (status, assigned) = assign(&setup, &setup.browser, alex, &managers["id"], false).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let (status, narrowed) = send(app, directory(&alex_browser)).await;
    assert_eq!(status, StatusCode::OK, "{narrowed}");
    assert_eq!(ids(&narrowed), [alex.to_string()], "{narrowed}");
    assert!(!narrowed.to_string().contains("\"bea\""), "{narrowed}");

    // The owner gives Helpers the directory: Cal sees everyone again.
    let (status, given) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("{}/{}", roles_path(&setup), helpers["id"].as_str().expect("id")),
            None,
            json!({"expectedVersion": helpers["version"], "permissions": ["tasks.view", "tasks.edit", "directory.view"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{given}");
    let (status, everyone) = send(app, directory(&cal_browser)).await;
    assert_eq!(status, StatusCode::OK, "{everyone}");
    for actor in [alex, bea, cal] {
        assert!(ids(&everyone).contains(&actor.to_string()), "{everyone}");
    }
}

/// Sending a delivery again through an app keeps to that app's limit even
/// when the webhook was made in the browser: an app without "see tasks"
/// can't resend a task's delivery; with it, the delivery is queued and
/// remembers that an app asked, so sending checks the limit again.
#[tokio::test]
async fn redelivering_through_an_app_keeps_to_its_limit() {
    let Some(setup) = setup("awd74_webhook_redeliver").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let project = work.parse::<Uuid>().expect("project");
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret webhook plan").await;
    // A webhook Bea made in the browser, and a task delivery it sent.
    let webhook = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO webhook_subscriptions (
            id, organization_id, project_id, owner_actor_id, name, url, event_types
        ) VALUES ($1, $2, $3, $4, 'Bea hook', 'https://example.com/hook', '{task.updated}')
        "#,
    )
    .bind(webhook)
    .bind(setup.organization_id)
    .bind(project)
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("webhook");
    let delivery = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO webhook_deliveries (
            id, subscription_id, organization_id, project_id, event_type, event_key,
            subject, state, attempts, delivered_at
        ) VALUES ($1, $2, $3, $4, 'task.updated', $5, $6, 'delivered', 1, now())
        "#,
    )
    .bind(delivery)
    .bind(webhook)
    .bind(setup.organization_id)
    .bind(project)
    .bind(format!("task:{task}:updated"))
    .bind(json!({"workItemId": task}))
    .execute(&setup.pool)
    .await
    .expect("delivery");
    let stored = |pool: sqlx::PgPool| async move {
        sqlx::query_as::<_, (String, bool)>(
            "SELECT state, made_through_app FROM webhook_deliveries WHERE id = $1",
        )
        .bind(delivery)
        .fetch_one(&pool)
        .await
        .expect("stored delivery")
    };
    let limit = |permissions: Value| {
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{bea}/agent-limit",
                setup.organization_id
            ),
            None,
            json!({"permissions": permissions}),
        )
    };
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let redeliver = json!({
        "operation": "redeliver",
        "webhookId": webhook.to_string(),
        "deliveryId": delivery.to_string(),
    });

    // Her app may change tasks but not see them: no resending.
    let (status, limited) = send(app, limit(json!(["tasks.edit", "channels.post"]))).await;
    assert_eq!(status, StatusCode::OK, "{limited}");
    let refused = tool(app, &bea_app, &session, "webhook_write", redeliver.clone()).await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert_eq!(
        stored(setup.pool.clone()).await,
        ("delivered".to_owned(), false)
    );

    // With "see tasks", it's queued, marked as an app's.
    let (status, limited) = send(
        app,
        limit(json!(["tasks.view", "tasks.edit", "channels.post"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limited}");
    let queued = tool(app, &bea_app, &session, "webhook_write", redeliver).await;
    assert_eq!(queued["outcome"], "ok", "{queued}");
    assert_eq!(
        stored(setup.pool.clone()).await,
        ("pending".to_owned(), true)
    );
}

/// A move copies a task's contents into the target, so it needs "see
/// tasks" where the task is: without it nothing moves. And the move's note
/// on the new task names linked repositories and pull requests only for a
/// mover who sees code there.
#[tokio::test]
async fn moving_a_task_needs_its_contents_and_code_to_be_seen() {
    let Some(setup) = setup("awd74_move_reads").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let dest = create_project(&setup, "DEST", "Dest").await;
    let project = work.parse::<Uuid>().expect("project");
    let target = dest.parse::<Uuid>().expect("target");
    let (bea, bea_browser) = add_person(&setup, &[&work, &dest], "bea").await;
    let repository = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, trusted, preparation_state, prepared_at
        ) VALUES ($1, $2, 464646, 'Mischa-dev', 'move-repo',
                  'https://github.com/Mischa-dev/move-repo.git',
                  'main', $3, true, 'ready', now())
        "#,
    )
    .bind(repository)
    .bind(project)
    .bind(format!("/tmp/agent-workspace-test-{repository}.git"))
    .execute(&setup.pool)
    .await
    .expect("repository");
    let linked_task = |title: &'static str| {
        let setup = &setup;
        let work = work.clone();
        async move {
            let task = owner_task(setup, &work, title).await;
            sqlx::query("UPDATE work_items SET description = 'Secret description' WHERE id = $1")
                .bind(task)
                .execute(&setup.pool)
                .await
                .expect("description");
            sqlx::query(
                r#"
                INSERT INTO work_item_repository_links
                    (project_id, work_item_id, repository_id, role, created_by_actor_id)
                VALUES ($1, $2, $3, 'primary', $4)
                "#,
            )
            .bind(project)
            .bind(task)
            .bind(repository)
            .bind(bea)
            .execute(&setup.pool)
            .await
            .expect("link the repository");
            task
        }
    };
    let in_target = |text: &'static str| {
        let pool = setup.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                r#"
                SELECT count(*) FROM task_rows item
                LEFT JOIN work_item_comments comment ON comment.work_item_id = item.id
                WHERE item.project_id = $1
                  AND (strpos(item.description, $2) > 0 OR strpos(comment.content::text, $2) > 0)
                "#,
            )
            .bind(target)
            .bind(text)
            .fetch_one(&pool)
            .await
            .expect("target contents")
        }
    };
    let set_work_role = |name: &'static str, permissions: Value, key: &'static str| {
        let setup = &setup;
        let work = work.clone();
        async move {
            let (status, role) = create_role(
                setup,
                &setup.browser,
                json!({"name": name, "baseRole": "member", "permissions": permissions}),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{role}");
            let (status, changed) =
                set_project_role(setup, &work, bea, key, json!({"roleId": role["id"]})).await;
            assert_eq!(status, StatusCode::OK, "{changed}");
        }
    };
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");

    // Without "see tasks" in Work, nothing moves.
    let hidden = linked_task("Hidden move source").await;
    set_work_role(
        "Blind movers",
        json!(["tasks.edit", "tasks.delete"]),
        "bea-blind-movers",
    )
    .await;
    let refused = tool(
        app,
        &bea_app,
        &session,
        "task_move",
        json!({"taskId": hidden.to_string(), "toProjectId": dest}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert_eq!(in_target("Secret description").await, 0, "nothing copied");

    // Seeing tasks but not code: it moves, and its note names no repository.
    let moved = linked_task("Moved without code").await;
    set_work_role(
        "Movers",
        json!(["tasks.view", "tasks.edit", "tasks.delete"]),
        "bea-movers",
    )
    .await;
    let done = tool(
        app,
        &bea_app,
        &session,
        "task_move",
        json!({"taskId": moved.to_string(), "toProjectId": dest}),
    )
    .await;
    assert_eq!(done["outcome"], "ok", "{done}");
    assert!(
        in_target("Moved from").await > 0,
        "the move's note is there"
    );
    assert_eq!(in_target("move-repo").await, 0, "no repository named");

    // The owner sees code: their move's note names it.
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    let named = linked_task("Moved with code").await;
    let done = tool(
        app,
        &owner_app,
        &owner_session,
        "task_move",
        json!({"taskId": named.to_string(), "toProjectId": dest}),
    )
    .await;
    assert_eq!(done["outcome"], "ok", "{done}");
    assert!(
        in_target("move-repo").await > 0,
        "the owner's note names it"
    );
}

/// Changing a task's links (task_relate, task_depend) answers with the task
/// and its links. Someone who may change tasks here but not see them gets
/// only its id and key, with no title, status or readiness, and none of its
/// links in this project, even for a change that changes nothing; with "see
/// tasks", all of it.
#[tokio::test]
async fn link_changes_name_the_task_only_where_tasks_are_seen() {
    let Some(setup) = setup("awd74_link_changes").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let task = owner_task(&setup, &work, "Secret linked task").await;
    let other = owner_task(&setup, &work, "Other linked task").await;
    // A task in the same project, already related to it and blocking it.
    let third = owner_task(&setup, &work, "Third linked secret").await;
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    for (name, arguments) in [
        (
            "task_relate",
            json!({"taskId": task.to_string(), "otherTaskId": third.to_string()}),
        ),
        (
            "task_depend",
            json!({"taskId": task.to_string(), "blockedBy": [third.to_string()]}),
        ),
    ] {
        let linked = tool(app, &owner_app, &owner_session, name, arguments).await;
        assert_eq!(linked["outcome"], "ok", "{name}: {linked}");
    }
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let calls = [
        (
            "task_relate",
            json!({"taskId": task.to_string(), "otherTaskId": other.to_string(), "remove": true}),
        ),
        (
            "task_depend",
            json!({"taskId": task.to_string(), "blockedBy": [other.to_string()], "remove": true}),
        ),
    ];

    // As a plain Member, Bea sees the task.
    for (name, arguments) in calls.clone() {
        let seen = tool(app, &bea_app, &session, name, arguments).await;
        assert_eq!(seen["outcome"], "ok", "{name}: {seen}");
        let changed = &seen["value"]["task"];
        assert_eq!(changed["title"], "Secret linked task", "{name}: {seen}");
        assert!(changed["status"].is_string(), "{name}: {seen}");
        assert!(changed["statusGroup"].is_string(), "{name}: {seen}");
        assert_eq!(changed["finished"], false, "{name}: {seen}");
        if name == "task_depend" {
            assert!(seen["value"]["ready"].is_boolean(), "{name}: {seen}");
        }
        assert!(
            seen.to_string().contains("Third linked secret"),
            "{name}: {seen}"
        );
    }

    // Changing tasks without seeing them: its id and key only.
    let (status, editors) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Blind editors", "baseRole": "member", "permissions": ["tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{editors}");
    let (status, changed) = set_project_role(
        &setup,
        &work,
        bea,
        "bea-blind-editors",
        json!({"roleId": editors["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    for (name, arguments) in calls {
        let blind = tool(app, &bea_app, &session, name, arguments).await;
        assert_eq!(blind["outcome"], "ok", "{name}: {blind}");
        let changed = &blind["value"]["task"];
        assert_eq!(changed["id"], task.to_string(), "{name}: {blind}");
        assert!(changed["key"].is_string(), "{name}: {blind}");
        for field in ["title", "status", "statusGroup", "finished"] {
            assert!(changed.get(field).is_none(), "{name} {field}: {blind}");
        }
        assert!(blind["value"].get("ready").is_none(), "{name}: {blind}");
        assert!(
            !blind.to_string().contains("Secret linked task"),
            "{name}: {blind}"
        );
        assert!(
            !blind.to_string().contains("Third linked secret"),
            "{name}: {blind}"
        );
        assert!(
            !blind.to_string().contains(&third.to_string()),
            "{name}: {blind}"
        );
    }
}

/// The tasks a link change lists (its relations, what blocks it and what it
/// blocks) follow "see tasks" in their own project, by role and, in an
/// app's call, by the app's limit there: a task in another project that the
/// app's limit hides there is left out, while the changed task still shows.
#[tokio::test]
async fn link_changes_list_linked_tasks_only_where_the_app_sees_them() {
    let Some(setup) = setup("awd74_linked_limit").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other_project = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other_project], "bea").await;
    let task = owner_task(&setup, &work, "Changed task").await;
    let spare = owner_task(&setup, &work, "Spare task").await;
    let elsewhere = owner_task(&setup, &other_project, "Elsewhere secret").await;
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    for (name, arguments) in [
        (
            "task_relate",
            json!({"taskId": task.to_string(), "otherTaskId": elsewhere.to_string()}),
        ),
        (
            "task_depend",
            json!({"taskId": task.to_string(), "blockedBy": [elsewhere.to_string()]}),
        ),
    ] {
        let linked = tool(app, &owner_app, &owner_session, name, arguments).await;
        assert_eq!(linked["outcome"], "ok", "{name}: {linked}");
    }
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let calls = [
        (
            "task_relate",
            json!({"taskId": task.to_string(), "otherTaskId": spare.to_string(), "remove": true}),
        ),
        (
            "task_depend",
            json!({"taskId": task.to_string(), "blocks": [spare.to_string()], "remove": true}),
        ),
    ];

    // Bea's app sees tasks in both projects: the linked task is listed.
    for (name, arguments) in calls.clone() {
        let listed = tool(app, &bea_app, &session, name, arguments).await;
        assert_eq!(listed["outcome"], "ok", "{name}: {listed}");
        assert!(
            listed.to_string().contains("Elsewhere secret"),
            "{name}: {listed}"
        );
    }

    // In Other, her role's limit leaves "see tasks" out of what apps do.
    let (status, limited) = create_role(
        &setup,
        &setup.browser,
        json!({
            "name": "Hands-on elsewhere",
            "baseRole": "member",
            "permissions": ["tasks.view", "tasks.edit"],
            "agentLimit": ["tasks.edit"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{limited}");
    let (status, changed) = set_project_role(
        &setup,
        &other_project,
        bea,
        "bea-hands-on-elsewhere",
        json!({"roleId": limited["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let left_out = |listed: &Value, name: &str| {
        assert_eq!(listed["outcome"], "ok", "{name}: {listed}");
        assert_eq!(
            listed["value"]["task"]["title"], "Changed task",
            "{name}: {listed}"
        );
        assert!(
            !listed.to_string().contains("Elsewhere secret"),
            "{name}: {listed}"
        );
        assert!(
            !listed.to_string().contains(&elsewhere.to_string()),
            "{name}: {listed}"
        );
    };
    for (name, arguments) in calls.clone() {
        let listed = tool(app, &bea_app, &session, name, arguments).await;
        left_out(&listed, name);
    }

    // Her role in Other itself leaves out "see tasks" (no app limit): the
    // linked task there is left out too.
    let (status, blind) = create_role(
        &setup,
        &setup.browser,
        json!({"name": "Blind elsewhere", "baseRole": "member", "permissions": ["tasks.edit"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blind}");
    let (status, changed) = set_project_role(
        &setup,
        &other_project,
        bea,
        "bea-blind-elsewhere",
        json!({"roleId": blind["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    for (name, arguments) in calls {
        let listed = tool(app, &bea_app, &session, name, arguments).await;
        left_out(&listed, name);
    }
}

/// The custom role `actor` holds in `project`, if any.
async fn project_role_id(setup: &Setup, project: &str, actor: Uuid) -> Option<Uuid> {
    sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT role_id FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project.parse::<Uuid>().expect("project"))
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("project role")
}

/// How many of `actor`'s notifications have `text` in their title.
async fn notified_of(setup: &Setup, actor: Uuid, text: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND strpos(detail->>'title', $2) > 0",
    )
    .bind(actor)
    .bind(text)
    .fetch_one(&setup.pool)
    .await
    .expect("count notifications")
}

/// A task's current version.
async fn task_version(setup: &Setup, project: Uuid, task: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE project_id = $1 AND id = $2")
        .bind(project)
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("version")
}

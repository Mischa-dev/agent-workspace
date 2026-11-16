//! K1.5: every account has one kind, and every agent and app one owner.
//! Old code that inserts `kind` only still gets an `account_kind`; an owner
//! chain never loops, never runs past three, and ends at a person or a
//! workspace (ruling R45).

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::support::{
    app::{create_agent, initialize, send, tool},
    world::WorldBuilder,
};

#[tokio::test]
async fn accounts_have_one_kind_and_agents_one_owner() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("k15_accounts")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let unset =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actors WHERE account_kind IS NULL")
            .fetch_one(pool)
            .await
            .expect("count");
    assert_eq!(unset, 0);
    let ownerless = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM actors WHERE account_kind IN ('agent', 'app') \
         AND num_nonnulls(owner_account_id, owner_workspace_id) <> 1",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(ownerless, 0, "every agent and app has exactly one owner");
    let owned = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM actors WHERE account_kind IN ('person', 'system') \
         AND num_nonnulls(owner_account_id, owner_workspace_id) <> 0",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(owned, 0, "people and the system have no owner");
    let rook = world.agents["rook"].actor;
    let (kind, owner) = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT account_kind, owner_workspace_id FROM actors WHERE id = $1",
    )
    .bind(rook)
    .fetch_one(pool)
    .await
    .expect("rook");
    assert_eq!(kind, "agent");
    assert_eq!(
        owner,
        Some(world.organization),
        "an agent belongs to its workspace"
    );

    // Old code inserting `kind` only.
    let worker = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'execution_worker')")
        .bind(worker)
        .execute(pool)
        .await
        .expect("old-style insert");
    let kind = sqlx::query_scalar::<_, String>("SELECT account_kind FROM actors WHERE id = $1")
        .bind(worker)
        .fetch_one(pool)
        .await
        .expect("kind");
    assert_eq!(kind, "agent");

    // An agent owned by a person; a loop is refused; a person has no owner.
    let lena = world.people["lena"].actor;
    sqlx::query("UPDATE actors SET owner_account_id = $2 WHERE id = $1")
        .bind(worker)
        .bind(lena)
        .execute(pool)
        .await
        .expect("a person may own an agent");
    let second = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO actors (id, kind, owner_account_id) VALUES ($1, 'external_agent', $2)",
    )
    .bind(second)
    .bind(worker)
    .execute(pool)
    .await
    .expect("an agent may own an agent");
    let looped = sqlx::query("UPDATE actors SET owner_account_id = $2 WHERE id = $1")
        .bind(worker)
        .bind(second)
        .execute(pool)
        .await;
    assert!(looped.is_err(), "an owner chain never loops");
    let owned_person = sqlx::query("UPDATE actors SET owner_account_id = $2 WHERE id = $1")
        .bind(lena)
        .bind(world.owner.actor)
        .execute(pool)
        .await;
    assert!(owned_person.is_err(), "a person has no owner");
    let both = sqlx::query("UPDATE actors SET owner_workspace_id = $2 WHERE id = $1")
        .bind(second)
        .bind(world.organization)
        .execute(pool)
        .await;
    assert!(both.is_err(), "one owner, not two");
    // Too long: lena <- worker <- second <- third <- fourth.
    let third = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO actors (id, kind, owner_account_id) VALUES ($1, 'external_agent', $2)",
    )
    .bind(third)
    .bind(second)
    .execute(pool)
    .await
    .expect("a chain of three");
    let fourth = sqlx::query(
        "INSERT INTO actors (id, kind, owner_account_id) VALUES ($1, 'external_agent', $2)",
    )
    .bind(Uuid::now_v7())
    .bind(third)
    .execute(pool)
    .await;
    assert!(fourth.is_err(), "a chain longer than three is refused");
    world.finish().await;
}

async fn post_json(app: &axum::Router, uri: &str, body: Value) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("body")))
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

/// K1.29: an admin adds a person who hasn't signed in yet. The account holds
/// its role at once and can't be signed in to; its one-time link lets the
/// person choose a sign-in name and password, once. Only the link's digest
/// is stored. A member without "invite people" can't add anyone, and nobody
/// gives a role they don't hold.
#[tokio::test]
async fn an_unclaimed_account_is_claimed_once_by_its_link() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("k129_unclaimed")
        .await
    else {
        return;
    };
    let route = format!("/api/v2/w/{}/ops/person_create", world.organization);
    let (status, added) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route,
            Some("add-nia"),
            json!({ "displayName": "Nia Okafor", "handle": "nia" }),
        ),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{status} {added}");
    let account = added["value"]["accountId"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .expect("account id");
    let token = added["value"]["claimToken"]
        .as_str()
        .expect("claim token")
        .to_owned();
    let pool = world.pool();
    let (role, enabled, claimed) = sqlx::query_as::<_, (String, bool, bool)>(
        "SELECT membership.role, membership.enabled, users.claimed_at IS NOT NULL \
         FROM organization_memberships membership JOIN users ON users.actor_id = membership.actor_id \
         WHERE membership.organization_id = $1 AND membership.actor_id = $2",
    )
    .bind(world.organization)
    .bind(account)
    .fetch_one(pool)
    .await
    .expect("the new membership");
    assert_eq!((role.as_str(), enabled, claimed), ("member", true, false));
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM account_claims WHERE account_id = $1 AND token_prefix = left($2, 16)",
    )
    .bind(account)
    .bind(&token)
    .fetch_one(pool)
    .await
    .expect("claim row");
    assert_eq!(stored, 1);
    let leaked = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE metadata::text LIKE '%' || $1 || '%'",
    )
    .bind(&token[16..])
    .fetch_one(pool)
    .await
    .expect("audit");
    assert_eq!(leaked, 0, "the token is never stored");

    // A wrong token, then the right one, then the right one again.
    let mut wrong = token.clone();
    wrong.replace_range(20..24, "AAAA");
    let claim = |token: &str, login: &str| {
        json!({ "token": token, "loginName": login, "password": "a long enough password" })
    };
    assert_eq!(
        post_json(&world.app, "/api/v1/account-claims/accept", claim(&wrong, "nia")).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post_json(&world.app, "/api/v1/account-claims/accept", claim(&token, "nia")).await,
        StatusCode::OK
    );
    assert_eq!(
        post_json(&world.app, "/api/v1/account-claims/accept", claim(&token, "nia2")).await,
        StatusCode::NOT_FOUND,
        "a link works once"
    );
    let (login, claimed) = sqlx::query_as::<_, (String, bool)>(
        "SELECT login_name, claimed_at IS NOT NULL FROM users WHERE actor_id = $1",
    )
    .bind(account)
    .fetch_one(pool)
    .await
    .expect("claimed");
    assert_eq!((login.as_str(), claimed), ("nia", true));
    assert_eq!(
        post_json(
            &world.app,
            "/api/v1/auth/login",
            json!({ "loginName": "nia", "password": "a long enough password" })
        )
        .await,
        StatusCode::OK
    );

    // A member can't add people; an admin can't give a role beyond theirs.
    let (status, refused) = send(
        &world.app,
        world.people["lena"].browser.request(
            "POST",
            &route,
            Some("lena-adds"),
            json!({ "displayName": "Someone", "handle": "someone" }),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{status} {refused}");
    let owner_role = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM workspace_roles WHERE organization_id = $1 AND key = 'owner'",
    )
    .bind(world.organization)
    .fetch_one(pool)
    .await
    .expect("owner role");
    let (status, refused) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route,
            Some("owner-on-arrival"),
            json!({ "displayName": "Boss", "handle": "boss", "roleId": owner_role }),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{status} {refused}");
}

/// K1.29: every person is made by `accounts::create_person`, so the account
/// says who made it and has a sign-in record.
#[test]
fn people_are_created_in_one_place() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path.strip_prefix(&root).expect("relative").to_string_lossy().to_string();
            if relative == "accounts/people.rs" || relative == "seed.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            if source.contains("INSERT INTO users (") || source.contains("INSERT INTO users(") {
                offenders.push(relative);
            }
        }
    }
    offenders.sort();
    // Adoption paths give an existing account its sign-in; nothing else
    // inserts users.
    assert_eq!(offenders, vec!["bootstrap.rs".to_owned()], "{offenders:?}");
}

/// K1.6: an agent belongs to the workspace, not to the project it was first
/// added to. Removing it from that project leaves it working in another it
/// belongs to; the project it left is closed to it.
#[tokio::test]
async fn an_agent_removed_from_its_first_project_still_works_in_another() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("SIDE")
        .build("k16_anchor")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let side = world.project("SIDE").to_owned();
    let (agent, token) = create_agent(&world.app, &world.owner.browser, &work, "Scout", "scout").await;
    let (status, added) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{side}/members"),
            None,
            json!({"actorId": agent, "role": "member"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(&work)
    .bind(agent)
    .fetch_one(world.pool())
    .await
    .expect("membership version");
    let (status, removed) = send(
        &world.app,
        world.owner.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/members/{agent}"),
            Some("remove-scout"),
            json!({"expectedVersion": version, "enabled": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    let session = initialize(&world.app, &token)
        .await
        .expect("the agent still connects after leaving its first project");
    let listed = tool(
        &world.app,
        &token,
        &session,
        "conversation_list",
        json!({"projectId": side}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let closed = tool(
        &world.app,
        &token,
        &session,
        "conversation_list",
        json!({"projectId": work}),
    )
    .await;
    assert_ne!(closed["outcome"], "ok", "{closed}");
    world.finish().await;
}

/// K1.6: an agent made at workspace level holds the role it was given, is
/// owned by the workspace, names its maker as a manager and starts in the
/// workspace's own space only: it sees no project until it joins one. A
/// member without "create agents" can't make one.
#[tokio::test]
async fn an_agent_made_at_workspace_level_starts_with_workspace_things_only() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("k16_workspace_agent")
        .await
    else {
        return;
    };
    let route = format!("/api/v2/w/{}/ops/agent_create", world.organization);
    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route,
            Some("make-ivy"),
            json!({ "displayName": "Ivy", "handle": "ivy" }),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let account = made["value"]["accountId"]
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .expect("account id");
    let token = made["value"]["credential"]
        .as_str()
        .expect("credential")
        .to_owned();
    let pool = world.pool();
    let (kind, owner, role) = sqlx::query_as::<_, (String, Option<Uuid>, String)>(
        "SELECT account.account_kind, account.owner_workspace_id, membership.role \
         FROM actors account JOIN organization_memberships membership \
           ON membership.actor_id = account.id AND membership.organization_id = $2 \
         WHERE account.id = $1",
    )
    .bind(account)
    .bind(world.organization)
    .fetch_one(pool)
    .await
    .expect("the agent");
    assert_eq!(
        (kind.as_str(), owner, role.as_str()),
        ("agent", Some(world.organization), "member")
    );
    let manages = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM account_managers WHERE account_id = $1 AND manager_account_id = $2)",
    )
    .bind(account)
    .bind(world.owner.actor)
    .fetch_one(pool)
    .await
    .expect("managers");
    assert!(manages, "its maker manages it");
    let session = initialize(&world.app, &token).await.expect("agent MCP");
    let projects = tool(&world.app, &token, &session, "projects_list", json!({})).await;
    let work = world.project("WORK").to_owned();
    let ids = projects["value"]["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .filter_map(|project| project["id"].as_str())
        .collect::<Vec<_>>();
    assert!(!ids.contains(&work.as_str()), "{projects}");
    let closed = tool(
        &world.app,
        &token,
        &session,
        "conversation_list",
        json!({"projectId": work}),
    )
    .await;
    assert_ne!(closed["outcome"], "ok", "{closed}");
    let (_, refused) = send(
        &world.app,
        world.people["lena"].browser.request(
            "POST",
            &route,
            Some("lena-makes"),
            json!({ "displayName": "Lena's", "handle": "lenas" }),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    world.finish().await;
}

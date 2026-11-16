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

use super::support::{app::send, world::WorldBuilder};

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

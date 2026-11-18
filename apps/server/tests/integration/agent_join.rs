//! A2.18 (K1 3.11): an agent you manage joins another workspace with a role
//! you pick, as the workspace's `access.outside_accounts` setting allows:
//! at once, after an admin there approves, or not at all. Someone who
//! doesn't manage it can't add it, and isn't told it exists.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::Person,
    world::{Workspace, WorldBuilder},
};

/// A second workspace, B, with the world's owner as its owner and lena as
/// an admin there.
pub(super) async fn second_workspace(world: &Workspace) -> Uuid {
    let pool = world.pool();
    let workspace = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Second', $2)")
        .bind(workspace)
        .bind(format!("second-{}", workspace.simple()))
        .execute(pool)
        .await
        .expect("workspace");
    for (account, role) in [
        (world.owner.actor, "owner"),
        (world.people["lena"].actor, "admin"),
    ] {
        sqlx::query(
            "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, $3)",
        )
        .bind(workspace)
        .bind(account)
        .bind(role)
        .execute(pool)
        .await
        .expect("membership");
    }
    workspace
}

async fn op(
    world: &Workspace,
    who: &Person,
    workspace: Uuid,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    send(
        &world.app,
        who.browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await
}

async fn member(world: &Workspace, workspace: Uuid, account: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2 AND enabled)",
    )
    .bind(workspace)
    .bind(account)
    .fetch_one(world.pool())
    .await
    .expect("member")
}

async fn outside_accounts(world: &Workspace, workspace: Uuid, value: &str) {
    sqlx::query(
        r#"
        INSERT INTO settings (id, organization_id, scope, key, value)
        VALUES (gen_random_uuid(), $1, 'workspace', 'access.outside_accounts', to_jsonb($2::text))
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(workspace)
    .bind(value)
    .execute(world.pool())
    .await
    .expect("setting");
    sqlx::query(
        "UPDATE settings SET value = to_jsonb($2::text) \
         WHERE organization_id = $1 AND scope = 'workspace' AND key = 'access.outside_accounts'",
    )
    .bind(workspace)
    .bind(value)
    .execute(world.pool())
    .await
    .expect("setting");
}

#[tokio::test]
async fn an_agent_you_manage_joins_another_workspace_as_it_allows() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .agent("scout", "member")
        .agent("wren", "member")
        .build("agent_join")
        .await
    else {
        return;
    };
    let second = second_workspace(&world).await;
    let owner = &world.owner;
    let lena = &world.people["lena"];
    let rook = world.agents["rook"].actor;

    // Listed for the owner (they manage the workspace's agents), not for
    // lena, who manages none of them.
    let (status, listed) = op(&world, owner, second, "account_joinable_list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let ids: Vec<&str> = listed["value"]["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .filter_map(|account| account["accountId"].as_str())
        .collect();
    assert!(ids.contains(&rook.to_string().as_str()), "{listed}");
    let (_, theirs) = op(&world, lena, second, "account_joinable_list", json!({})).await;
    assert_eq!(theirs["value"]["accounts"], json!([]), "{theirs}");

    // Someone who doesn't manage it can't add it, and learns nothing.
    let (_, refused) = op(
        &world,
        lena,
        second,
        "account_workspace_add",
        json!({"account": rook}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    assert!(!member(&world, second, rook).await);

    // By default it joins at once, keeping its name; again changes nothing.
    let (status, joined) = op(
        &world,
        owner,
        second,
        "account_workspace_add",
        json!({"account": rook}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["value"]["status"], "joined", "{joined}");
    assert!(joined["value"]["spaceId"].is_string(), "{joined}");
    assert!(member(&world, second, rook).await);
    let handle = sqlx::query_scalar::<_, String>(
        "SELECT handle FROM organization_actor_profiles WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(second)
    .bind(rook)
    .fetch_one(world.pool())
    .await
    .expect("profile");
    assert_eq!(handle, "rook");
    let (_, again) = op(
        &world,
        owner,
        second,
        "account_workspace_add",
        json!({"account": rook}),
    )
    .await;
    assert_eq!(again["value"]["status"], "already_member", "{again}");

    // A workspace that says no refuses agents owned outside it.
    outside_accounts(&world, second, "no").await;
    let scout = world.agents["scout"].actor;
    let (_, no) = op(
        &world,
        owner,
        second,
        "account_workspace_add",
        json!({"account": scout}),
    )
    .await;
    assert_eq!(no["outcome"], "permission_denied", "{no}");
    assert!(!member(&world, second, scout).await);

    // One that asks an admin: the join waits for lena's approval.
    outside_accounts(&world, second, "ask_admin").await;
    let wren = world.agents["wren"].actor;
    let (_, asked) = op(
        &world,
        owner,
        second,
        "account_workspace_add",
        json!({"account": wren}),
    )
    .await;
    assert_eq!(asked["value"]["status"], "asked", "{asked}");
    let request = asked["value"]["requestId"]
        .as_str()
        .expect("request")
        .to_owned();
    assert!(!member(&world, second, wren).await);
    let (_, repeat) = op(
        &world,
        owner,
        second,
        "account_workspace_add",
        json!({"account": wren}),
    )
    .await;
    assert_eq!(
        repeat["value"]["requestId"],
        request.as_str(),
        "one request: {repeat}"
    );
    let (status, answered) = op(
        &world,
        lena,
        second,
        "request_respond",
        json!({"requestId": request, "optionId": "approve"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["outcome"], "ok", "{answered}");
    assert!(member(&world, second, wren).await);

    world.finish().await;
}

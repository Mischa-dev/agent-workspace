//! K1.7: an agent account's outside reference. Attaching, rebinding and
//! detaching are managing the agent; a pair belongs to one account at a
//! time; a host only proposes a new pair, and nothing changes until a
//! manager accepts it. A rebind keeps the account, its roles and what it
//! wrote.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{app::send, world::WorldBuilder};

#[tokio::test]
async fn outside_references_follow_managers_and_keep_the_account() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .agent("scout", "member")
        .build("k17_agent_refs")
        .await
    else {
        return;
    };
    let rook = world.agents["rook"].actor;
    let scout = world.agents["scout"].actor;
    let call = |person: &str, op: &str, key: &str, body: Value| {
        let browser = if person == "owner" {
            &world.owner.browser
        } else {
            &world.people[person].browser
        };
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{op}", world.organization),
            Some(key),
            body,
        )
    };
    let role_before = role_of(&world, rook).await;

    // The owner manages every agent of the workspace and attaches one.
    let (status, attached) = send(
        &world.app,
        call(
            "owner",
            "agent_ref_set",
            "attach-rook",
            json!({"agent": rook, "issuer": "host-one", "externalId": "rook-a"}),
        ),
    )
    .await;
    assert_eq!(attached["outcome"], "ok", "{status} {attached}");
    assert_eq!(attached["value"]["active"]["externalId"], "rook-a");

    // The same pair can't be another agent's too.
    let (_, taken) = send(
        &world.app,
        call(
            "owner",
            "agent_ref_set",
            "attach-scout",
            json!({"agent": scout, "issuer": "host-one", "externalId": "rook-a"}),
        ),
    )
    .await;
    assert_eq!(taken["outcome"], "invalid_request", "{taken}");

    // A member who doesn't manage Rook can't attach, rebind or detach.
    for (op, key, body) in [
        (
            "agent_ref_update",
            "lena-rebind",
            json!({"agent": rook, "issuer": "lena", "externalId": "x"}),
        ),
        ("agent_ref_remove", "lena-detach", json!({"agent": rook})),
    ] {
        let (_, refused) = send(&world.app, call("lena", op, key, body)).await;
        assert_ne!(refused["outcome"], "ok", "lena ran {op}: {refused}");
    }

    // A host proposes a new pair: it only waits.
    let mut transaction = world.pool().begin().await.expect("begin");
    let proposed = agent_workspace_server::accounts::refs::propose_from_host(
        &mut transaction,
        Uuid::now_v7(),
        rook,
        "host-two",
        "rook-b",
    )
    .await
    .expect("propose");
    transaction.commit().await.expect("commit");
    assert!(proposed);
    let (active, pending) = refs_of(&world, rook).await;
    assert_eq!(
        active.as_deref(),
        Some("host-one/rook-a"),
        "never applied on the host's word"
    );
    assert_eq!(pending.as_deref(), Some("host-two/rook-b"));

    // A member can't accept it for the managers.
    let (_, refused) = send(
        &world.app,
        call(
            "lena",
            "agent_ref_decide",
            "lena-accept",
            json!({"agent": rook, "accept": true}),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert_eq!(
        refs_of(&world, rook).await.0.as_deref(),
        Some("host-one/rook-a")
    );

    // A manager accepts: the pair moves, the account stays.
    let (status, accepted) = send(
        &world.app,
        call(
            "owner",
            "agent_ref_decide",
            "owner-accept",
            json!({"agent": rook, "accept": true}),
        ),
    )
    .await;
    assert_eq!(accepted["outcome"], "ok", "{status} {accepted}");
    assert_eq!(accepted["value"]["active"]["externalId"], "rook-b");
    assert_eq!(
        accepted["value"]["active"]["lineage"][0]["externalId"],
        "rook-a"
    );
    assert!(accepted["value"]["pending"].is_null());
    assert_eq!(accepted["value"]["agent"], rook.to_string());
    assert_eq!(role_of(&world, rook).await, role_before, "its role stays");

    // A manager rebinds directly; the lineage grows, newest first.
    let (_, rebound) = send(
        &world.app,
        call(
            "owner",
            "agent_ref_update",
            "owner-rebind",
            json!({"agent": rook, "issuer": "host-three", "externalId": "rook-c"}),
        ),
    )
    .await;
    assert_eq!(rebound["outcome"], "ok", "{rebound}");
    let lineage = rebound["value"]["active"]["lineage"]
        .as_array()
        .expect("lineage");
    assert_eq!(lineage.len(), 2);
    assert_eq!(lineage[0]["externalId"], "rook-b");

    // A rejected proposal changes nothing; the old pair is free again.
    let mut transaction = world.pool().begin().await.expect("begin");
    agent_workspace_server::accounts::refs::propose_from_host(
        &mut transaction,
        Uuid::now_v7(),
        rook,
        "host-four",
        "rook-d",
    )
    .await
    .expect("propose");
    transaction.commit().await.expect("commit");
    let (_, rejected) = send(
        &world.app,
        call(
            "owner",
            "agent_ref_decide",
            "owner-reject",
            json!({"agent": rook, "accept": false}),
        ),
    )
    .await;
    assert_eq!(rejected["outcome"], "ok", "{rejected}");
    assert_eq!(
        refs_of(&world, rook).await,
        (Some("host-three/rook-c".to_owned()), None)
    );
    let (_, scout_attached) = send(
        &world.app,
        call(
            "owner",
            "agent_ref_set",
            "attach-scout-free",
            json!({"agent": scout, "issuer": "host-one", "externalId": "rook-a"}),
        ),
    )
    .await;
    assert_eq!(scout_attached["outcome"], "ok", "{scout_attached}");

    // Detaching leaves the account without one.
    let (_, detached) = send(
        &world.app,
        call(
            "owner",
            "agent_ref_remove",
            "owner-detach",
            json!({"agent": rook}),
        ),
    )
    .await;
    assert_eq!(detached["outcome"], "ok", "{detached}");
    assert!(detached["value"]["active"].is_null());
    world.finish().await;
}

async fn refs_of(
    world: &super::support::world::Workspace,
    account: Uuid,
) -> (Option<String>, Option<String>) {
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT status, issuer, external_id FROM agent_outside_refs \
         WHERE account_id = $1 AND status <> 'detached'",
    )
    .bind(account)
    .fetch_all(world.pool())
    .await
    .expect("refs");
    let pick = |status: &str| {
        rows.iter()
            .find(|row| row.0 == status)
            .map(|row| format!("{}/{}", row.1, row.2))
    };
    (pick("active"), pick("pending_rebind"))
}

async fn role_of(world: &super::support::world::Workspace, account: Uuid) -> Option<Uuid> {
    sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT role_id FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(account)
    .fetch_one(world.pool())
    .await
    .expect("role")
}

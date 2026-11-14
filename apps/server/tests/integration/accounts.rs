//! K1.5: every account has one kind, and every agent and app one owner.
//! Old code that inserts `kind` only still gets an `account_kind`; an owner
//! chain never loops, never runs past three, and ends at a person or a
//! workspace (ruling R45).

use uuid::Uuid;

use super::support::world::WorldBuilder;

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

//! K4: spaces. The new names are views over today's tables (K4.1), and
//! every statement form the code uses works through them; a personal space
//! needs an owner and there is one per account per workspace.

use uuid::Uuid;

use super::support::world::setup;

#[tokio::test]
async fn views_write_through() {
    let Some(setup) = setup("k41_views").await else {
        return;
    };
    let pool = &setup.pool;
    let workspace = setup.organization_id;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("owner");

    // INSERT, with renamed columns.
    let space = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, visibility, created_by_actor_id) \
         VALUES ($1, $2, 'VIEW', 'Through the view', 'team', 'workspace', $3)",
    )
    .bind(space)
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .expect("insert through spaces");
    let (key, kind, visibility) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT project_key, kind, visibility FROM projects WHERE id = $1",
    )
    .bind(space)
    .fetch_one(pool)
    .await
    .expect("base row");
    assert_eq!(
        (key.as_str(), kind.as_str(), visibility.as_str()),
        ("VIEW", "team", "workspace")
    );

    // INSERT ... ON CONFLICT DO NOTHING, and ON CONFLICT on renamed columns.
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name) VALUES ($1, $2, 'VIEW', 'Again') \
         ON CONFLICT DO NOTHING",
    )
    .bind(space)
    .bind(workspace)
    .execute(pool)
    .await
    .expect("on conflict do nothing");
    let member = Uuid::now_v7();
    for name in ["Owner", "Owner again"] {
        sqlx::query(
            "INSERT INTO space_members (id, space_id, actor_id, role, display_name, handle) \
             VALUES ($1, $2, $3, 'owner', $4, 'owner-handle') \
             ON CONFLICT (space_id, actor_id) DO UPDATE SET display_name = EXCLUDED.display_name",
        )
        .bind(member)
        .bind(space)
        .bind(owner)
        .bind(name)
        .execute(pool)
        .await
        .expect("upsert through space_members");
    }
    let membership_name = sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(space)
    .bind(owner)
    .fetch_one(pool)
    .await
    .expect("membership");
    assert!(!membership_name.is_empty());

    // UPDATE ... FROM, and row locks of an alias of the view.
    let renamed = sqlx::query(
        "UPDATE spaces s SET description = 'From the workspace' FROM workspaces w \
         WHERE s.id = $1 AND w.id = s.workspace_id",
    )
    .bind(space)
    .execute(pool)
    .await
    .expect("update from")
    .rows_affected();
    assert_eq!(renamed, 1);
    let mut tx = pool.begin().await.expect("begin");
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT s.id FROM spaces s JOIN workspace_members m ON m.workspace_id = s.workspace_id \
         WHERE s.id = $1 AND m.actor_id = $2 FOR UPDATE OF s",
    )
    .bind(space)
    .bind(owner)
    .fetch_one(&mut *tx)
    .await
    .expect("for update of a view alias");
    assert_eq!(locked, space);
    sqlx::query_scalar::<_, Uuid>(
        "SELECT m.actor_id FROM workspace_members m WHERE m.workspace_id = $1 FOR SHARE OF m",
    )
    .bind(workspace)
    .fetch_all(&mut *tx)
    .await
    .expect("for share of a view alias");
    tx.rollback().await.expect("rollback");

    // DELETE through the view (a membership: a project gets side rows from
    // triggers, so a project is never deleted this way).
    let deleted = sqlx::query("DELETE FROM space_members WHERE space_id = $1 AND actor_id = $2")
        .bind(space)
        .bind(owner)
        .execute(pool)
        .await
        .expect("delete through space_members")
        .rows_affected();
    assert_eq!(deleted, 1);

    // The workspace view reads the base table.
    let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM workspaces WHERE id = $1")
        .bind(workspace)
        .fetch_one(pool)
        .await
        .expect("workspace");
    assert!(!slug.is_empty());
    // The workspace's own space is open to the workspace.
    let closed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM spaces WHERE kind = 'workspace' AND visibility <> 'workspace'",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(closed, 0);
}

#[tokio::test]
async fn kinds() {
    let Some(setup) = setup("k41_kinds").await else {
        return;
    };
    let pool = &setup.pool;
    let workspace = setup.organization_id;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("owner");
    let without_owner = sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind) VALUES ($1, $2, 'MINE', 'Mine', 'personal')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .execute(pool)
    .await;
    assert!(without_owner.is_err(), "a personal space needs its owner");
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, owner_actor_id) \
         VALUES ($1, $2, 'MINE', 'Mine', 'personal', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .expect("one personal space");
    let second = sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, owner_actor_id) \
         VALUES ($1, $2, 'MINETWO', 'Mine too', 'personal', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await;
    assert!(
        second.is_err(),
        "one personal space per account per workspace"
    );
    let owned_project = sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, owner_actor_id) VALUES ($1, $2, 'OWNED', 'Owned', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await;
    assert!(owned_project.is_err(), "only personal spaces have an owner");
}

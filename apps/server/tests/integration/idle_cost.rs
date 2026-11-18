//! H1.42: nothing runs per workspace. The silent, costly failure: a loop
//! that queries once per workspace, or scans every workspace on each pass,
//! which costs nothing on a test server and everything on a hosted one
//! with thousands of quiet workspaces. Counted as database work, from
//! PostgreSQL's own statistics for the test database, so the rest of the
//! suite running beside it doesn't blur the numbers.

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::cluster_support::cluster;

/// How long each idle stretch is counted for.
const WINDOW: Duration = Duration::from_secs(20);
/// The workspaces added for the second count.
const WORKSPACES: i64 = 10_000;

/// Transactions finished and rows read in the test database so far.
async fn counts(pool: &PgPool) -> (i64, i64) {
    let mut connection = pool.acquire().await.expect("connection");
    // A fresh read of the statistics, not this connection's snapshot.
    sqlx::query("SELECT pg_stat_clear_snapshot()")
        .execute(&mut *connection)
        .await
        .expect("clear snapshot");
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT (xact_commit + xact_rollback)::bigint, (tup_returned + tup_fetched)::bigint \
         FROM pg_stat_database WHERE datname = current_database()",
    )
    .fetch_one(&mut *connection)
    .await
    .expect("statistics")
}

/// What the database did over one idle window.
async fn idle(pool: &PgPool) -> (i64, i64) {
    let (transactions, rows) = counts(pool).await;
    tokio::time::sleep(WINDOW).await;
    // Busy connections report their statistics within a second of going
    // idle; let the last of them land.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (transactions_after, rows_after) = counts(pool).await;
    (transactions_after - transactions, rows_after - rows)
}

/// Until the log has nothing staged, for at most a minute.
async fn settle(pool: &PgPool) {
    for _ in 0..120 {
        let staged = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event_staging")
            .fetch_one(pool)
            .await
            .expect("staged");
        if staged == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn ten_thousand_quiet_workspaces_cost_what_one_does() {
    let Some(cluster) = cluster("h1_42_idle_cost", 1).await else {
        return;
    };
    let pool = &cluster.pool;
    settle(pool).await;
    let (one_transactions, one_rows) = idle(pool).await;

    // Ten thousand empty workspaces, made in one batch as
    // `lifecycle::create` makes one: the row, its owner and its plan. (Most
    // of this test's time is here, in the triggers that give each its
    // workspace space and roles.)
    let owner =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(pool)
            .await
            .expect("owner");
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::query(
        "INSERT INTO organizations (id, name, slug) \
         SELECT gen_random_uuid(), 'Quiet ' || n, 'quiet-' || n FROM generate_series(1, $1) n",
    )
    .bind(WORKSPACES)
    .execute(&mut *transaction)
    .await
    .expect("workspaces");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) \
         SELECT id, $1, 'owner', true FROM organizations WHERE slug LIKE 'quiet-%'",
    )
    .bind(owner)
    .execute(&mut *transaction)
    .await
    .expect("owners");
    sqlx::query(
        "INSERT INTO workspace_plans (workspace_id, plan_id) \
         SELECT organization.id, plan.id FROM organizations organization \
         JOIN plans plan ON plan.is_default WHERE organization.slug LIKE 'quiet-%'",
    )
    .execute(&mut *transaction)
    .await
    .expect("plans");
    transaction.commit().await.expect("commit");
    sqlx::query("ANALYZE organizations, organization_memberships, workspace_plans")
        .execute(pool)
        .await
        .expect("analyze");
    settle(pool).await;
    let (many_transactions, many_rows) = idle(pool).await;

    eprintln!(
        "idle over {}s: one workspace {one_transactions} transactions, {one_rows} rows read; \
         {WORKSPACES} more {many_transactions} transactions, {many_rows} rows read",
        WINDOW.as_secs()
    );
    // A query per workspace on any pass shows as thousands of transactions;
    // a scan of every workspace on each pass, as hundreds of thousands of
    // rows. The margins leave room for timing, not for either.
    assert!(
        many_transactions <= one_transactions * 3 / 2 + 50,
        "transactions grew with workspaces: {one_transactions} -> {many_transactions}"
    );
    assert!(
        many_rows <= one_rows * 2 + 4 * WORKSPACES,
        "rows read grew with workspaces: {one_rows} -> {many_rows}"
    );
}

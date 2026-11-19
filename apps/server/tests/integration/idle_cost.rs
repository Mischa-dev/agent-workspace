//! H1.42: nothing runs per workspace. The silent, costly failure: a loop
//! that queries once per workspace, or scans every workspace on each pass,
//! which costs nothing on a test server and everything on a hosted one
//! with thousands of quiet workspaces. Counted as database work, from
//! PostgreSQL's own statistics for the test database, so the rest of the
//! suite running beside it doesn't blur the numbers.

use std::{collections::HashMap, time::Duration};

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

/// Rows read and scans so far, table by table.
async fn tables(pool: &PgPool) -> HashMap<String, (i64, i64)> {
    let mut connection = pool.acquire().await.expect("connection");
    sqlx::query("SELECT pg_stat_clear_snapshot()")
        .execute(&mut *connection)
        .await
        .expect("clear snapshot");
    sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT relname::text, \
                (COALESCE(seq_tup_read, 0) + COALESCE(idx_tup_fetch, 0))::bigint, \
                (COALESCE(seq_scan, 0) + COALESCE(idx_scan, 0))::bigint \
         FROM pg_stat_user_tables",
    )
    .fetch_all(&mut *connection)
    .await
    .expect("table statistics")
    .into_iter()
    .map(|(table, rows, scans)| (table, (rows, scans)))
    .collect()
}

/// The tables read most over a window, so a failure names its cause.
fn busiest(before: &HashMap<String, (i64, i64)>, after: &HashMap<String, (i64, i64)>) -> String {
    let mut grown = after
        .iter()
        .map(|(table, (rows, scans))| {
            let (rows_before, scans_before) = before.get(table).copied().unwrap_or_default();
            (rows - rows_before, scans - scans_before, table.as_str())
        })
        .filter(|(rows, scans, _)| *rows > 0 || *scans > 0)
        .collect::<Vec<_>>();
    grown.sort_unstable_by(|a, b| b.cmp(a));
    grown
        .iter()
        .take(8)
        .map(|(rows, scans, table)| format!("{table} {rows} rows in {scans} scans"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// What the database did over one idle window, and the tables it read
/// most.
async fn idle(pool: &PgPool) -> (i64, i64, String) {
    let (transactions, rows) = counts(pool).await;
    let before = tables(pool).await;
    let maintenance_before = maintenance(pool).await;
    let sessions_before = sessions(pool).await;
    // What the other connections to the database are doing, sampled each
    // second, so a failure names the statements behind its transactions
    // even when they read no table.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for _ in 0..WINDOW.as_secs() {
        tokio::time::sleep(Duration::from_secs(1)).await;
        for statement in activity(pool).await {
            *seen.entry(statement).or_default() += 1;
        }
    }
    // Busy connections report their statistics within a second of going
    // idle; let the last of them land.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (transactions_after, rows_after) = counts(pool).await;
    let after = tables(pool).await;
    let maintenance = maintenance(pool).await - maintenance_before;
    let (rollbacks, connections) = sessions(pool).await;
    let (rollbacks, connections) = (
        rollbacks - sessions_before.0,
        connections - sessions_before.1,
    );
    let mut seen = seen.into_iter().collect::<Vec<_>>();
    seen.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let seen = seen
        .iter()
        .take(10)
        .map(|(statement, times)| format!("{times}x {statement}"))
        .collect::<Vec<_>>()
        .join(" | ");
    // What the server did in the window: jobs it ran, consumers it moved.
    let work = sqlx::query_scalar::<_, Option<String>>(
        "SELECT concat_ws('; ', \
             (SELECT 'jobs: ' || string_agg(kind || ' ' || state || ' x' || n, ', ') FROM ( \
                 SELECT kind, state, count(*) AS n FROM background_jobs \
                 WHERE updated_at > now() - make_interval(secs => $1) \
                 GROUP BY kind, state ORDER BY count(*) DESC LIMIT 12) jobs), \
             (SELECT 'consumers: ' || string_agg(name, ', ') FROM event_consumers \
              WHERE updated_at > now() - make_interval(secs => $1)))",
    )
    .bind((WINDOW.as_secs() + 2) as f64)
    .fetch_one(pool)
    .await
    .expect("window work")
    .unwrap_or_default();
    (
        transactions_after - transactions,
        rows_after - rows,
        format!(
            "{}; autovacuum passes {maintenance}; rollbacks {rollbacks}; \
             new sessions {connections}; {work}; sampled: {seen}",
            busiest(&before, &after)
        ),
    )
}

/// Rolled back transactions and sessions opened on the test database so
/// far.
async fn sessions(pool: &PgPool) -> (i64, i64) {
    let mut connection = pool.acquire().await.expect("connection");
    sqlx::query("SELECT pg_stat_clear_snapshot()")
        .execute(&mut *connection)
        .await
        .expect("clear snapshot");
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT xact_rollback::bigint, sessions::bigint \
         FROM pg_stat_database WHERE datname = current_database()",
    )
    .fetch_one(&mut *connection)
    .await
    .expect("session statistics")
}

/// The other connections to the test database: their kind, state and the
/// start of their current or last statement.
async fn activity(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT concat_ws(' ', backend_type, state, \
                left(regexp_replace(query, '\\s+', ' ', 'g'), 90)) \
         FROM pg_stat_activity \
         WHERE datname = current_database() AND pid <> pg_backend_pid()",
    )
    .fetch_all(pool)
    .await
    .expect("activity")
}

/// Autovacuum and autoanalyze passes over the test database's tables so
/// far: each is a transaction of its own, so one landing in a window shows.
async fn maintenance(pool: &PgPool) -> i64 {
    let mut connection = pool.acquire().await.expect("connection");
    sqlx::query("SELECT pg_stat_clear_snapshot()")
        .execute(&mut *connection)
        .await
        .expect("clear snapshot");
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(autovacuum_count + autoanalyze_count), 0)::bigint \
         FROM pg_stat_user_tables",
    )
    .fetch_one(&mut *connection)
    .await
    .expect("maintenance statistics")
}

/// Until the server has caught up with what making the workspaces wrote,
/// for at most three minutes: nothing staged, every consumer of the log
/// that is moving at its head, and no job due or running. Catching up on
/// a backlog isn't idle work; anything still running after it is.
/// Waits until nothing is staged, no consumer that ran in the last minute
/// is behind the head, and no job is running or due; then a few seconds
/// more. Fails, naming what is still busy, if that takes over ten minutes:
/// an idle measurement taken while work is still catching up says nothing.
async fn settle(pool: &PgPool) {
    let busy_now = || async {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT NULLIF(concat_ws('; ', \
                 (SELECT 'staged ' || count(*) FROM event_staging HAVING count(*) > 0), \
                 (SELECT 'behind: ' || string_agg(name || ' by ' || \
                         ((SELECT next_position - 1 FROM event_position_counter) - position), ', ') \
                  FROM event_consumers \
                  WHERE position < (SELECT next_position - 1 FROM event_position_counter) \
                    AND updated_at > now() - interval '1 minute'), \
                 (SELECT 'jobs: ' || string_agg(kind || ' ' || state, ', ') FROM background_jobs \
                  WHERE state = 'running' OR (state = 'queued' AND run_at <= now()))), '')",
        )
        .fetch_one(pool)
        .await
        .expect("busy")
    };
    let mut busy = None;
    for _ in 0..1200 {
        busy = busy_now().await;
        if busy.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(busy.is_none(), "never settled: {busy:?}");
    tokio::time::sleep(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn ten_thousand_quiet_workspaces_cost_what_one_does() {
    let Some(cluster) = cluster("h1_42_idle_cost", 1).await else {
        return;
    };
    let pool = &cluster.pool;
    settle(pool).await;
    let (one_transactions, one_rows, one_tables) = idle(pool).await;

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
    // What autovacuum does on a server in time: every table that making the
    // workspaces wrote to is vacuumed and analyzed once. Stale statistics
    // would plan the idle polls by position as scans of the whole table,
    // and autovacuum catching up inside the window is a transaction per
    // table, which is the batch insert's cost, not idle work.
    sqlx::query("VACUUM (ANALYZE)")
        .execute(pool)
        .await
        .expect("vacuum");
    settle(pool).await;
    let (many_transactions, many_rows, many_tables) = idle(pool).await;

    eprintln!(
        "idle over {}s: one workspace {one_transactions} transactions, {one_rows} rows read; \
         {WORKSPACES} more {many_transactions} transactions, {many_rows} rows read\n\
         busiest with one: {one_tables}\nbusiest with many: {many_tables}",
        WINDOW.as_secs()
    );
    // A query per workspace on any pass shows as thousands of transactions;
    // a scan of every workspace on each pass, as hundreds of thousands of
    // rows. The margins leave room for timing, not for either.
    assert!(
        many_transactions <= one_transactions * 3 / 2 + 50,
        "transactions grew with workspaces: {one_transactions} -> {many_transactions}; \
         busiest: {many_tables}"
    );
    assert!(
        many_rows <= one_rows * 2 + 4 * WORKSPACES,
        "rows read grew with workspaces: {one_rows} -> {many_rows}; busiest: {many_tables}"
    );
}

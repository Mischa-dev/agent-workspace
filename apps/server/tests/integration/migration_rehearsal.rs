//! The release preflight's rehearsal on data (`preflight::rehearse`): the
//! migrations still to run are applied to the database's own rows in a
//! transaction that is rolled back. The silent, costly failure: a migration
//! that passes every test on empty tables and stops at the deploy on rows
//! only the live database has, and a rehearsal that lets it through or
//! keeps something it applied.

use std::borrow::Cow;

use agent_workspace_server::{
    preflight::{Rehearsal, rehearse},
    tenancy,
};
use sqlx::{
    Executor, PgPool,
    migrate::{Migration, MigrationType, Migrator},
    postgres::PgPoolOptions,
};
use uuid::Uuid;

use super::support::db::schema_pool;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The migration an upgrade of the live data stopped on (#363)...
const STOPPED: i64 = 20261117050000;
/// ...and the one written afterwards, sorting before it, that repairs the
/// rows it stopped on.
const REPAIR: i64 = 20261117045900;

/// An empty schema of its own, and the pool that made it.
async fn schema(name: &str) -> Option<(PgPool, PgPool, String)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("admin");
    let schema = format!("{name}_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("schema");
    let pool = schema_pool(&database_url, &schema).await;
    Some((admin, pool, schema))
}

async fn finish(admin: PgPool, pool: PgPool, schema: String) {
    pool.close().await;
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop schema");
}

/// As the preflight command runs it.
async fn rehearsed(pool: &PgPool, pending: &[Migration]) -> Rehearsal {
    tenancy::system("server.operator", rehearse(pool, pending))
        .await
        .expect("rehearse")
}

fn migration(version: i64, sql: &'static str) -> Migration {
    Migration::new(
        version,
        Cow::Borrowed("made for the test"),
        MigrationType::Simple,
        Cow::Borrowed(sql),
        false,
    )
}

/// The rows that stopped the upgrade, as the live database had them: a
/// credential still holding `approval_respond` on a connection that had been
/// removed.
const A_CREDENTIAL_ON_A_REMOVED_CONNECTION: &str = r"
INSERT INTO organizations (id, name, slug)
VALUES ('01900000-0000-7000-8000-000000000001', 'Old', 'old');
INSERT INTO actors (id, kind, account_kind)
VALUES ('01900000-0000-7000-8000-000000000002', 'human', 'person'),
       ('01900000-0000-7000-8000-000000000003', 'external_agent', 'agent');
INSERT INTO projects (id, organization_id, project_key, name)
VALUES ('01900000-0000-7000-8000-000000000004', '01900000-0000-7000-8000-000000000001',
        'OLD', 'Old');
INSERT INTO external_agent_connections (
    id, project_id, actor_id, connection_name, runtime_kind, runtime_version, scopes,
    created_by_actor_id, updated_by_actor_id)
VALUES ('01900000-0000-7000-8000-000000000005', '01900000-0000-7000-8000-000000000004',
        '01900000-0000-7000-8000-000000000003', 'Old agent', 'custom', '1',
        ARRAY['read', 'approval_respond'], '01900000-0000-7000-8000-000000000002',
        '01900000-0000-7000-8000-000000000002');
INSERT INTO mcp_client_credentials (
    id, project_id, actor_id, client_name, token_prefix, token_digest, scopes, expires_at,
    external_agent_connection_id)
VALUES ('01900000-0000-7000-8000-000000000006', '01900000-0000-7000-8000-000000000004',
        '01900000-0000-7000-8000-000000000003', 'Old agent', 'aw_ext_oldagent',
        decode(repeat('ab', 32), 'hex'), ARRAY['read', 'approval_respond'],
        now() + interval '30 days', '01900000-0000-7000-8000-000000000005');
UPDATE external_agent_connections SET status = 'removed', removed_at = now()
 WHERE id = '01900000-0000-7000-8000-000000000005';
";

#[tokio::test]
async fn the_rows_that_stopped_an_upgrade_stop_its_rehearsal_and_nothing_is_kept() {
    let Some((admin, pool, schema)) = schema("rehearsal_live_rows").await else {
        return;
    };
    let before = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version < REPAIR)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    before.run(&pool).await.expect("the schema of that day");
    let mut seeding = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('app.tenancy', 'system', true)")
        .execute(&mut *seeding)
        .await
        .expect("scope");
    sqlx::raw_sql(A_CREDENTIAL_ON_A_REMOVED_CONNECTION)
        .execute(&mut *seeding)
        .await
        .expect("the live rows");
    seeding.commit().await.expect("commit");
    // What a rehearsal must leave exactly as it found it: the migrations
    // recorded, the tables, and the rows the stopped migration had already
    // changed before the statement that stopped it.
    let state = || async {
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT (SELECT count(*) FROM _sqlx_migrations),
                    (SELECT count(*) FROM pg_tables WHERE schemaname = current_schema()),
                    (SELECT count(*) FROM external_agent_connections
                      WHERE 'approval_respond' = ANY(scopes)),
                    (SELECT count(*) FROM mcp_client_credentials
                      WHERE 'approval_respond' = ANY(scopes))",
        )
        .fetch_one(&pool)
        .await
        .expect("state")
    };
    let found = state().await;
    assert_eq!((found.2, found.3), (1, 1), "the rows are there");

    // As the release was first built: without the repair.
    let unrepaired: Vec<Migration> = MIGRATOR
        .iter()
        .filter(|migration| migration.version >= STOPPED)
        .cloned()
        .collect();
    let Rehearsal::Stops {
        migration: stopped,
        reason,
    } = rehearsed(&pool, &unrepaired).await
    else {
        panic!("the rehearsal let the migration through");
    };
    assert!(stopped.starts_with(&STOPPED.to_string()), "{stopped}");
    assert_eq!(
        reason,
        "MCP credential Actor class and external connection do not match"
    );
    assert_eq!(state().await, found, "a stopped rehearsal kept something");

    // With the repair, every migration since applies, and still none is kept.
    let repaired: Vec<Migration> = MIGRATOR
        .iter()
        .filter(|migration| migration.version >= REPAIR)
        .cloned()
        .collect();
    assert_eq!(
        rehearsed(&pool, &repaired).await,
        Rehearsal::Applies(repaired.len())
    );
    assert_eq!(
        state().await,
        found,
        "a rehearsal that passed kept something"
    );

    // The rehearsal's word holds: the migrator itself applies them.
    MIGRATOR.run(&pool).await.expect("the upgrade");

    finish(admin, pool, schema).await;
}

/// A constraint that waits for the commit stops a migration at its commit.
/// The rehearsal never commits, so it checks them after each migration, and
/// then leaves them waiting again for the next.
#[tokio::test]
async fn what_a_commit_would_refuse_stops_the_rehearsal() {
    let Some((admin, pool, schema)) = schema("rehearsal_commit").await else {
        return;
    };
    let tables = migration(
        1,
        "CREATE TABLE rehearsed_parent (id int PRIMARY KEY);
         CREATE TABLE rehearsed_child (
             id int PRIMARY KEY,
             parent int CONSTRAINT waits_for_commit REFERENCES rehearsed_parent (id)
                 DEFERRABLE INITIALLY DEFERRED,
             other int CONSTRAINT checked_at_once REFERENCES rehearsed_parent (id)
                 DEFERRABLE INITIALLY IMMEDIATE
         );",
    );
    // Fine at its own commit: the parent arrives before it.
    let child_first = migration(
        2,
        "INSERT INTO rehearsed_child VALUES (1, 1, NULL);
         INSERT INTO rehearsed_parent VALUES (1);
         SET CONSTRAINTS checked_at_once DEFERRED;
         INSERT INTO rehearsed_child VALUES (2, 1, 2);
         INSERT INTO rehearsed_parent VALUES (2);",
    );
    // Refused at once in a transaction of its own: the one before it having
    // deferred the constraint mustn't carry over.
    let needs_it_at_once = migration(
        3,
        "DO $$ BEGIN
             INSERT INTO rehearsed_child VALUES (3, 1, 3);
             RAISE EXCEPTION 'the constraint was still deferred';
         EXCEPTION WHEN foreign_key_violation THEN NULL;
         END $$;",
    );
    let no_parent = migration(4, "INSERT INTO rehearsed_child VALUES (4, 9, NULL);");
    let after_it = migration(5, "INSERT INTO rehearsed_parent VALUES (9);");

    let fine = [
        tables.clone(),
        child_first.clone(),
        needs_it_at_once.clone(),
    ];
    assert_eq!(rehearsed(&pool, &fine).await, Rehearsal::Applies(3));

    let refused = [
        tables.clone(),
        child_first,
        needs_it_at_once,
        no_parent,
        after_it,
    ];
    let Rehearsal::Stops {
        migration: stopped,
        reason,
    } = rehearsed(&pool, &refused).await
    else {
        panic!("a migration its commit would refuse was let through");
    };
    assert!(stopped.starts_with("4 "), "{stopped}");
    assert!(reason.contains("waits_for_commit"), "{reason}");

    // One that ends its transaction would keep what came before it, so
    // nothing is tried.
    let commits = [tables, migration(2, "COMMIT;")];
    assert!(matches!(
        rehearsed(&pool, &commits).await,
        Rehearsal::NotTried(_)
    ));

    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_tables WHERE schemaname = current_schema()",
    )
    .fetch_one(&pool)
    .await
    .expect("tables");
    assert_eq!(kept, 0, "a rehearsal kept a table");

    finish(admin, pool, schema).await;
}

//! Helpers the crate's own tests and its integration tests share.

use sqlx::PgPool;

pub mod source;

/// Drops a test's PostgreSQL schema and everything in it.
///
/// `DROP SCHEMA ... CASCADE` takes a lock on every table, index, constraint
/// and function in the schema inside one transaction: about 5,600 with every
/// migration applied. PostgreSQL's shared lock table holds roughly
/// `max_locks_per_transaction` × `max_connections` (64 × 100 at the
/// defaults), so a few tests tearing down at once failed with "out of shared
/// memory". Dropping the tables one at a time first keeps each transaction to
/// a few hundred locks.
pub async fn drop_schema(pool: &PgPool, schema: &str) -> Result<(), sqlx::Error> {
    let tables = sqlx::query_scalar::<_, String>(
        r#"
        SELECT format('DROP TABLE IF EXISTS %I.%I CASCADE', n.nspname, c.relname)
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.oid = to_regnamespace($1)
          AND c.relkind IN ('r', 'p')
          AND NOT c.relispartition
        "#,
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;
    for statement in tables {
        retry_deadlock(pool, &statement).await?;
    }
    retry_deadlock(pool, &format!("DROP SCHEMA {schema} CASCADE")).await
}

/// A drop can deadlock with work the test left running (a run finishing in
/// the background); the straggler loses on its own soon, so try again.
async fn retry_deadlock(pool: &PgPool, statement: &str) -> Result<(), sqlx::Error> {
    let mut tries = 0;
    loop {
        match sqlx::query(statement).execute(pool).await {
            Err(sqlx::Error::Database(error))
                if error.code().as_deref() == Some("40P01") && tries < 5 =>
            {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            result => return result.map(|_| ()),
        }
    }
}

//! Helpers the crate's own tests and its integration tests share.

use sqlx::PgPool;

pub mod source;

/// A signed download link (K5.12) to `file_id` for `account`, as they would
/// read it in the browser, without the origin: what an isolation test opens.
pub fn file_link(state: &crate::AppState, account: uuid::Uuid, file_id: uuid::Uuid) -> String {
    let (url, _) = crate::files::link::link(
        state,
        crate::files::Reader::Browser { actor_id: account },
        file_id,
        crate::files::link::Variant::Original,
        crate::files::link::DEFAULT_TTL,
    )
    .expect("a file link");
    let origin = crate::events::public_origin(state);
    url.strip_prefix(&origin).expect("same origin").to_owned()
}

/// Drops a test's PostgreSQL schema and everything in it.
///
/// `DROP SCHEMA ... CASCADE` takes a lock on every table, index, constraint
/// and function in the schema inside one transaction: about 5,600 with every
/// migration applied. PostgreSQL's shared lock table holds roughly
/// `max_locks_per_transaction` × `max_connections` (64 × 100 at the
/// defaults), so a few tests tearing down at once failed with "out of shared
/// memory". Dropping the tables one at a time first keeps each transaction to
/// a few hundred locks.
///
/// A test's app can still be at work when the test ends: a run its scheduler
/// admitted, driven by a task the test never joins, writing through the
/// schema's own pool. Its locks and the drop's can deadlock (AWD-183), so any
/// other session holding a lock in the schema is stopped first, and again
/// before each retry.
pub async fn drop_schema(pool: &PgPool, schema: &str) -> Result<(), sqlx::Error> {
    stop_schema_users(pool, schema).await?;
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
        retry_deadlock(pool, schema, &statement).await?;
    }
    retry_deadlock(pool, schema, &format!("DROP SCHEMA {schema} CASCADE")).await
}

/// Ends every other session holding a lock on something in `schema`, saying
/// which statement it was in so a test that leaves work behind can be found.
async fn stop_schema_users(pool: &PgPool, schema: &str) -> Result<(), sqlx::Error> {
    let users = sqlx::query_as::<_, (i32, String)>(
        r#"
        SELECT DISTINCT activity.pid, coalesce(left(activity.query, 160), '')
        FROM pg_locks held
        JOIN pg_class class ON class.oid = held.relation
        JOIN pg_stat_activity activity ON activity.pid = held.pid
        WHERE class.relnamespace = to_regnamespace($1)
          AND held.pid <> pg_backend_pid()
        "#,
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;
    for (pid, statement) in users {
        eprintln!("dropping {schema}: stopping session {pid}, still in: {statement}");
        // Not ours to end (another role's) is left to the retry.
        let _ = sqlx::query("SELECT pg_terminate_backend($1)")
            .bind(pid)
            .execute(pool)
            .await;
    }
    Ok(())
}

/// A drop can still deadlock with a session that took its first lock after
/// the sweep; stop it and try again.
async fn retry_deadlock(pool: &PgPool, schema: &str, statement: &str) -> Result<(), sqlx::Error> {
    let mut tries = 0;
    loop {
        match sqlx::query(statement).execute(pool).await {
            Err(sqlx::Error::Database(error))
                if error.code().as_deref() == Some("40P01") && tries < 5 =>
            {
                tries += 1;
                stop_schema_users(pool, schema).await?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            result => return result.map(|_| ()),
        }
    }
}

/// Bytes staged for a workspace before their row exists, for tests of
/// [`crate::files::stage_content`] and [`crate::files::abandon_content`].
pub struct StagedBytes(crate::files::StagedContent, uuid::Uuid);

impl StagedBytes {
    /// Where the bytes are in the store.
    pub fn key(&self) -> &str {
        self.0.key()
    }
}

/// Stages `bytes` for `workspace` as an upload does before its transaction.
pub async fn stage_bytes(
    state: &crate::AppState,
    workspace: uuid::Uuid,
    bytes: &[u8],
) -> Result<StagedBytes, crate::error::AppError> {
    // In the workspace's scope, as the upload's own request would be.
    crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: None,
            workspace,
        },
        crate::files::stage_content(state, workspace, bytes),
    )
    .await
    .map(|content| StagedBytes(content, workspace))
}

/// Takes a stage back, as a refused upload does.
pub async fn abandon_bytes(state: &crate::AppState, staged: &StagedBytes) {
    crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: None,
            workspace: staged.1,
        },
        crate::files::abandon_content(state, &staged.0),
    )
    .await;
}

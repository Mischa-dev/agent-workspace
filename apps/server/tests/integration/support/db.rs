//! Test databases. The first test of a run migrates a template database
//! named after the migrations (`aw_tpl_<12 hex>`), once, under a lock; every
//! test then gets its own copy (`aw_t_<uuid>`), which takes a moment instead
//! of a full migration, and drops it when done. Where the database role can't
//! create databases, a test gets a migrated schema instead.

use std::{sync::Arc, time::Duration};

use sha2::{Digest, Sha256};
use sqlx::{
    Connection, Executor, PgConnection, PgPool, migrate::Migrator, postgres::PgPoolOptions,
};
use uuid::Uuid;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub(crate) async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |connection, _metadata| {
            let schema = Arc::clone(&schema);
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema.as_str())
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect schema pool")
}

/// A database for one test: `pool` reaches only it. `finish` drops it.
pub(crate) struct Db {
    pub(crate) pool: PgPool,
    /// The configured database's URL, for the app's configuration.
    pub(crate) url: String,
    base: String,
    admin: PgPool,
    teardown: Teardown,
}

/// Drops a test's database (or schema) when the last holder lets go of it,
/// a panicking test included, for fixtures such as `Setup` that have no
/// `finish`.
pub(crate) struct DropWhenDone {
    base: String,
    teardown: Teardown,
}

impl Drop for DropWhenDone {
    fn drop(&mut self) {
        let base = self.base.clone();
        let teardown = std::mem::replace(&mut self.teardown, Teardown::Schema(String::new()));
        // Drop can't wait on the test's runtime, so a thread of its own does.
        let dropped = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                // A busy server (every test holding connections) can refuse
                // one more for a moment, so try a few times, then say which
                // database is left (`checks.sh` sweeps leftovers too).
                let mut last_error = String::new();
                for attempt in 1..=10_u64 {
                    let dropped = async {
                        let admin = PgPoolOptions::new()
                            .max_connections(1)
                            .connect(&base)
                            .await?;
                        let result = match &teardown {
                            Teardown::Database(name) => {
                                sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
                                    .execute(&admin)
                                    .await
                                    .map(drop)
                            }
                            Teardown::Schema(schema) if !schema.is_empty() => {
                                agent_workspace_server::test_support::drop_schema(&admin, schema)
                                    .await
                            }
                            Teardown::Schema(_) => Ok(()),
                        };
                        admin.close().await;
                        result
                    }
                    .await;
                    match dropped {
                        Ok(()) => return,
                        Err(error) => last_error = error.to_string(),
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200 * attempt)).await;
                }
                let left = match &teardown {
                    Teardown::Database(name) | Teardown::Schema(name) => name,
                };
                eprintln!("test database {left} was not dropped: {last_error}");
            });
        });
        let _ = dropped.join();
    }
}

enum Teardown {
    Database(String),
    Schema(String),
}

/// A fresh, migrated database for the test `name`, or None (and a note)
/// when no database is configured.
pub(crate) async fn db(name: &str) -> Option<Db> {
    let Ok(base) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&base)
        .await
        .expect("connect to PostgreSQL");
    let template = template_name(
        MIGRATOR
            .iter()
            .map(|migration| (migration.version, migration.checksum.as_ref())),
    );
    match ensure_template(&base, &template).await {
        Ok(()) => Some(copy_of(&base, admin, &template).await),
        Err(reason) => {
            eprintln!("test databases: {reason}; this test migrates a schema instead");
            Some(migrated_schema(&base, admin, name).await)
        }
    }
}

impl Db {
    /// The pool, and a guard that drops the database when the last clone of
    /// it goes.
    pub(crate) fn until_dropped(self) -> (PgPool, String, std::sync::Arc<DropWhenDone>) {
        (
            self.pool,
            self.url,
            std::sync::Arc::new(DropWhenDone {
                base: self.base,
                teardown: self.teardown,
            }),
        )
    }

    /// Drops the test's database.
    pub(crate) async fn finish(self) {
        self.pool.close().await;
        match self.teardown {
            Teardown::Database(name) => {
                sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
                    .execute(&self.admin)
                    .await
                    .expect("drop the test database");
            }
            Teardown::Schema(schema) => {
                agent_workspace_server::test_support::drop_schema(&self.admin, &schema)
                    .await
                    .expect("drop schema");
            }
        }
    }
}

/// The template's name for a set of migrations: a new set (a migration
/// added or changed) gets a new template.
pub(crate) fn template_name<'a>(migrations: impl Iterator<Item = (i64, &'a [u8])>) -> String {
    let mut hash = Sha256::new();
    for (version, checksum) in migrations {
        hash.update(version.to_be_bytes());
        hash.update(checksum);
    }
    format!("aw_tpl_{}", &hex::encode(hash.finalize())[..12])
}

/// `base` with its database swapped for `database`.
fn with_database(base: &str, database: &str) -> String {
    let (head, query) = base
        .split_once('?')
        .map_or((base, None), |(head, query)| (head, Some(query)));
    let head = head.rsplit_once('/').map_or(head, |(server, _)| server);
    match query {
        Some(query) => format!("{head}/{database}?{query}"),
        None => format!("{head}/{database}"),
    }
}

/// Makes the template `name` unless it's there: under a session lock on a
/// connection of its own (CREATE DATABASE can't run in a transaction), so
/// tests starting together make it once. It's built under another name and
/// renamed when migrated, so a run killed halfway leaves no broken template.
pub(crate) async fn ensure_template(base: &str, name: &str) -> Result<(), String> {
    let mut connection = PgConnection::connect(base)
        .await
        .map_err(|error| format!("connect: {error}"))?;
    let lock = format!("aw-test-template:{name}");
    connection
        .execute(sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))").bind(&lock))
        .await
        .map_err(|error| format!("lock: {error}"))?;
    let made = make_template(&mut connection, base, name).await;
    let _ = connection
        .execute(sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))").bind(&lock))
        .await;
    let _ = connection.close().await;
    made
}

async fn make_template(
    connection: &mut PgConnection,
    base: &str,
    name: &str,
) -> Result<(), String> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)",
    )
    .bind(name)
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| format!("look for the template: {error}"))?;
    if exists {
        return Ok(());
    }
    let building = format!("{name}_new");
    connection
        .execute(format!("DROP DATABASE IF EXISTS {building} WITH (FORCE)").as_str())
        .await
        .map_err(|error| format!("clear a half-built template: {error}"))?;
    connection
        .execute(format!("CREATE DATABASE {building}").as_str())
        .await
        .map_err(|error| format!("create a database: {error}"))?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&with_database(base, &building))
        .await
        .map_err(|error| format!("connect to the template: {error}"))?;
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|error| format!("migrate the template: {error}"))?;
    pool.close().await;
    connection
        .execute(format!("ALTER DATABASE {building} RENAME TO {name}").as_str())
        .await
        .map_err(|error| format!("name the template: {error}"))?;
    Ok(())
}

/// A copy of the template for one test, with synchronous commits off: a
/// test database needn't survive a crash.
async fn copy_of(base: &str, admin: PgPool, template: &str) -> Db {
    let database = format!("aw_t_{}", Uuid::now_v7().simple());
    // Copies of one template can't start while another is being taken, so
    // a test that meets one waits its turn.
    let mut attempt = 0;
    loop {
        match sqlx::query(&format!("CREATE DATABASE {database} TEMPLATE {template}"))
            .execute(&admin)
            .await
        {
            Ok(_) => break,
            Err(error) if attempt < 50 && error.to_string().contains("being accessed") => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // Another run's `checks.sh` swept the template away: make it again.
            Err(error) if attempt < 50 && error.to_string().contains("does not exist") => {
                attempt += 1;
                if let Err(reason) = ensure_template(base, template).await {
                    panic!("remake the test template: {reason}");
                }
            }
            Err(error) => panic!("copy the test template: {error}"),
        }
    }
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                connection.execute("SET synchronous_commit = off").await?;
                Ok(())
            })
        })
        .connect(&with_database(base, &database))
        .await
        .expect("connect to the test database");
    Db {
        pool,
        url: with_database(base, &database),
        base: base.to_owned(),
        admin,
        teardown: Teardown::Database(database),
    }
}

async fn migrated_schema(base: &str, admin: PgPool, name: &str) -> Db {
    let schema = format!("{name}_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    let pool = schema_pool(base, &schema).await;
    MIGRATOR.run(&pool).await.expect("apply migrations");
    Db {
        pool,
        url: base.to_owned(),
        base: base.to_owned(),
        admin,
        teardown: Teardown::Schema(schema),
    }
}

#[tokio::test]
async fn two_first_calls_make_one_template() {
    let Ok(base) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        return;
    };
    let name = format!("aw_tpl_test_{}", &Uuid::now_v7().simple().to_string()[20..]);
    let (first, second) =
        tokio::join!(ensure_template(&base, &name), ensure_template(&base, &name));
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base)
        .await
        .expect("connect");
    let made = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_database WHERE datname LIKE $1 || '%'",
    )
    .bind(&name)
    .fetch_one(&admin)
    .await
    .expect("count templates");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
        .execute(&admin)
        .await
        .expect("drop the test template");
    if first.is_err() && second.is_err() {
        eprintln!("this role can't create databases: {first:?}");
        return;
    }
    assert_eq!((first, second), (Ok(()), Ok(())));
    assert_eq!(made, 1, "one template, and nothing half-built beside it");
}

#[test]
fn a_changed_migration_set_gets_a_new_template() {
    let today = template_name([(1, &b"a"[..]), (2, &b"b"[..])].into_iter());
    assert_eq!(
        today,
        template_name([(1, &b"a"[..]), (2, &b"b"[..])].into_iter())
    );
    assert_ne!(
        today,
        template_name([(1, &b"a"[..]), (2, &b"c"[..])].into_iter())
    );
    assert_ne!(
        today,
        template_name([(1, &b"a"[..]), (2, &b"b"[..]), (3, &b"d"[..])].into_iter())
    );
    assert!(today.starts_with("aw_tpl_") && today.len() == "aw_tpl_".len() + 12);
    assert_eq!(
        with_database("postgres://u:p@h:5432/base?sslmode=disable", "aw_t_1"),
        "postgres://u:p@h:5432/aw_t_1?sslmode=disable"
    );
}

#[tokio::test]
async fn a_test_database_has_every_migration() {
    let Some(db) = db("support_db").await else {
        return;
    };
    let recorded =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.pool)
            .await
            .expect("count migrations");
    let expected = MIGRATOR
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .count();
    assert_eq!(recorded, i64::try_from(expected).expect("count"));
    db.finish().await;
}

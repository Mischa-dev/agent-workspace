//! Test databases.

use std::sync::Arc;

use sqlx::{PgPool, postgres::PgPoolOptions};

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

/// A database for one test: `pool` reaches only it. Today it's a schema of
/// the configured database with every migration applied; `finish` drops it.
pub(crate) struct Db {
    pub(crate) pool: PgPool,
    /// The configured database's URL, for the app's configuration.
    pub(crate) url: String,
    admin: PgPool,
    schema: String,
}

/// A fresh, migrated database named after the test, or None (and a note)
/// when no database is configured.
pub(crate) async fn db(name: &str) -> Option<Db> {
    let Ok(url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{name}_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    let pool = schema_pool(&url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    Some(Db {
        pool,
        url,
        admin,
        schema,
    })
}

impl Db {
    /// Drops the test's database.
    pub(crate) async fn finish(self) {
        self.pool.close().await;
        agent_workspace_server::test_support::drop_schema(&self.admin, &self.schema)
            .await
            .expect("drop schema");
    }
}

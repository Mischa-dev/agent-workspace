//! H1.3: every table belongs to a workspace or says why it doesn't. Row
//! rules, the workspace export and the isolation guards read a table's
//! tenant column from `tenancy::tables`; a table missing from it would be
//! left out of all three without anyone noticing.

use std::collections::{BTreeMap, BTreeSet};

use agent_workspace_server::tenancy::tables::{
    GLOBAL_TABLES, MISSING_WORKSPACE_COLUMN, MUST_BE_GLOBAL, ORGANIZATION_ID, TENANT_TABLES,
    WORKSPACE_ID,
};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

/// A fresh schema with every migration applied, and a pool whose
/// connections use it. None when no database is configured.
pub(crate) async fn migrated_schema(prefix: &str) -> Option<(PgPool, String)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let search_path = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let schema = search_path.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&database_url)
        .await
        .expect("connect to the test schema");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    Some((pool, schema))
}

/// Every base table in `schema`, with its columns.
async fn tables(pool: &PgPool, schema: &str) -> BTreeMap<String, BTreeSet<String>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT c.table_name, c.column_name
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON t.table_schema = c.table_schema AND t.table_name = c.table_name
        WHERE c.table_schema = $1 AND t.table_type = 'BASE TABLE'
        "#,
    )
    .bind(schema)
    .fetch_all(pool)
    .await
    .expect("read the schema");
    let mut tables = BTreeMap::<String, BTreeSet<String>>::new();
    for (table, column) in rows {
        tables.entry(table).or_default().insert(column);
    }
    tables
}

/// What is wrong with the lists against the schema's tables, in words.
fn problems(tables: &BTreeMap<String, BTreeSet<String>>) -> Vec<String> {
    let mut problems = Vec::new();
    let tenant = TENANT_TABLES.iter().copied().collect::<BTreeMap<_, _>>();
    let global = GLOBAL_TABLES.iter().copied().collect::<BTreeMap<_, _>>();
    let missing = MISSING_WORKSPACE_COLUMN
        .iter()
        .copied()
        .collect::<BTreeMap<_, _>>();
    for (table, columns) in tables {
        let table = table.as_str();
        let lists = [
            tenant.contains_key(table),
            global.contains_key(table),
            missing.contains_key(table),
        ]
        .into_iter()
        .filter(|listed| *listed)
        .count();
        match lists {
            0 => problems.push(format!(
                "{table}: add it to tenancy::tables, as a tenant table with its \
                 `workspace_id` column or as a global table with the reason"
            )),
            1 => {}
            _ => problems.push(format!("{table}: listed more than once")),
        }
        if MUST_BE_GLOBAL.contains(&table) && !global.contains_key(table) {
            problems.push(format!(
                "{table}: accounts, credentials, sign-in state and hosts are global"
            ));
        }
        if let Some(column) = tenant.get(table)
            && !columns.contains(*column)
        {
            problems.push(format!("{table}: has no {column} column"));
        }
        if missing.contains_key(table)
            && (columns.contains(WORKSPACE_ID) || columns.contains(ORGANIZATION_ID))
        {
            problems.push(format!(
                "{table}: has its workspace column now; move it to TENANT_TABLES"
            ));
        }
    }
    for table in tenant
        .keys()
        .chain(global.keys())
        .chain(missing.keys())
        .filter(|table| !tables.contains_key(**table))
    {
        problems.push(format!("{table}: listed but not in the schema"));
    }
    problems
}

#[tokio::test]
async fn every_table_belongs_to_a_workspace_or_says_why() {
    let Some((pool, schema)) = migrated_schema("tenancy_tables").await else {
        return;
    };
    let mut tables = tables(&pool, &schema).await;
    // sqlx keeps its ledger in the first schema on the search path.
    tables.entry("_sqlx_migrations".to_owned()).or_default();
    let found = problems(&tables);
    assert!(found.is_empty(), "{found:#?}");

    // The guard has teeth: a new table in no list fails it, and so does an
    // account table listed as a workspace's.
    let mut with_new = tables.clone();
    with_new.insert(
        "new_things".to_owned(),
        BTreeSet::from(["id".to_owned(), "project_id".to_owned()]),
    );
    assert!(
        problems(&with_new)
            .iter()
            .any(|problem| problem.starts_with("new_things"))
    );
    let mut with_hosts = tables.clone();
    with_hosts.insert(
        "hosts".to_owned(),
        BTreeSet::from(["id".to_owned(), "workspace_id".to_owned()]),
    );
    assert!(
        problems(&with_hosts)
            .iter()
            .any(|problem| problem.starts_with("hosts"))
    );

    // No membership or role row names the direct scope (R20), once a
    // workspace can be one.
    let has_kind = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = $1 AND table_name = 'organizations' AND column_name = 'kind'
        )
        "#,
    )
    .bind(&schema)
    .fetch_one(&pool)
    .await
    .expect("read the schema");
    if has_kind {
        let named = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT (SELECT count(*) FROM organization_memberships m
                    JOIN organizations o ON o.id = m.organization_id WHERE o.kind = 'direct')
                 + (SELECT count(*) FROM workspace_roles r
                    JOIN organizations o ON o.id = r.organization_id WHERE o.kind = 'direct')
            "#,
        )
        .fetch_one(&pool)
        .await
        .expect("count direct-scope memberships");
        assert_eq!(named, 0, "a membership or role names the direct scope");
    }
    agent_workspace_server::test_support::drop_schema(&pool, &schema)
        .await
        .expect("drop the test schema");
    pool.close().await;
}

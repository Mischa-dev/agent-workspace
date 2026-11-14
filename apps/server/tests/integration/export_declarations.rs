//! H1.8: every workspace table is exported by its module or says why not,
//! and every reference column is declared. A table or reference left out of
//! the declarations would be silently missing from every workspace export.

use std::collections::{BTreeMap, BTreeSet};

use agent_workspace_server::{
    export::{self, ExportDecl},
    tenancy::tables::{GLOBAL_TABLES, MISSING_WORKSPACE_COLUMN, TENANT_TABLES},
};
use sqlx::PgPool;
use uuid::Uuid;

use super::tenancy_tables::migrated_schema;

/// Columns that hold a secret or a credential's digest: never exported.
const SECRET_MARKERS: &[&str] = &[
    "token_digest",
    "token_hash",
    "token_prefix",
    "token_hint",
    "secret_id",
    "webhook_secret",
    "code_hash",
    "password",
    "ciphertext",
    "wrapped_key",
    "wrap_nonce",
    "private_key",
    "api_key",
];

struct Schema {
    columns: BTreeMap<String, BTreeSet<String>>,
    /// Table, column: the columns that are part of a foreign key.
    references: BTreeSet<(String, String)>,
}

async fn schema(pool: &PgPool, schema: &str) -> Schema {
    let mut columns = BTreeMap::<String, BTreeSet<String>>::new();
    for (table, column) in sqlx::query_as::<_, (String, String)>(
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
    .expect("columns")
    {
        columns.entry(table).or_default().insert(column);
    }
    let references = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT class.relname::text, attribute.attname::text
        FROM pg_constraint constraint_row
        JOIN pg_class class ON class.oid = constraint_row.conrelid
        JOIN pg_namespace namespace ON namespace.oid = class.relnamespace
        CROSS JOIN LATERAL unnest(constraint_row.conkey) AS key(attnum)
        JOIN pg_attribute attribute
          ON attribute.attrelid = constraint_row.conrelid AND attribute.attnum = key.attnum
        WHERE constraint_row.contype = 'f' AND namespace.nspname = $1
        "#,
    )
    .bind(schema)
    .fetch_all(pool)
    .await
    .expect("foreign keys")
    .into_iter()
    .collect();
    Schema {
        columns,
        references,
    }
}

/// What is wrong with the declarations against the schema, in words.
fn problems(declarations: &[&ExportDecl], schema: &Schema) -> Vec<String> {
    let mut problems = Vec::new();
    let workspace_tables = TENANT_TABLES
        .iter()
        .map(|(table, _)| *table)
        .chain(MISSING_WORKSPACE_COLUMN.iter().map(|(table, _)| *table))
        .collect::<BTreeSet<_>>();
    let mut declared = BTreeMap::<&str, usize>::new();
    let modules = declarations
        .iter()
        .map(|declaration| declaration.module)
        .collect::<Vec<_>>();
    for (position, declaration) in declarations.iter().enumerate() {
        for after in declaration.after {
            if !modules[..position].contains(after) {
                problems.push(format!(
                    "{}: imports after {after}, which isn't an earlier module",
                    declaration.module
                ));
            }
        }
        for (table, reason) in declaration.not_exported {
            *declared.entry(table).or_default() += 1;
            if reason.trim().is_empty() {
                problems.push(format!("{table}: not exported, but says no reason"));
            }
        }
        for kind in declaration.kinds {
            *declared.entry(kind.table).or_default() += 1;
            let Some(columns) = schema.columns.get(kind.table) else {
                continue;
            };
            for (column, target) in kind.references {
                if !columns.contains(*column) {
                    problems.push(format!("{}.{column}: no such column", kind.table));
                }
                if !schema.columns.contains_key(*target) {
                    problems.push(format!(
                        "{}.{column}: refers to no table {target}",
                        kind.table
                    ));
                }
            }
            for column in kind.exclude.iter().chain(kind.text_references) {
                if !columns.contains(*column) {
                    problems.push(format!("{}.{column}: no such column", kind.table));
                }
            }
            for column in columns {
                let decided = kind.references.iter().any(|(name, _)| name == column)
                    || kind.exclude.contains(&column.as_str());
                let tenant = column == "organization_id" || column == "workspace_id";
                if schema
                    .references
                    .contains(&(kind.table.to_owned(), column.clone()))
                    && !decided
                    && !tenant
                {
                    problems.push(format!(
                        "{}.{column}: a reference with no export decision; declare the kind \
                         it points at, or exclude it",
                        kind.table
                    ));
                }
                if SECRET_MARKERS.iter().any(|marker| column.contains(marker))
                    && !kind.exclude.contains(&column.as_str())
                {
                    problems.push(format!(
                        "{}.{column}: looks like a secret; exclude it",
                        kind.table
                    ));
                }
            }
        }
    }
    for table in schema.columns.keys().map(String::as_str) {
        let global = GLOBAL_TABLES.iter().any(|(name, _)| *name == table);
        match (global, declared.get(table).copied().unwrap_or(0)) {
            (true, 0) => {}
            (true, _) => problems.push(format!("{table}: global, so no workspace exports it")),
            (false, 0) => problems.push(format!(
                "{table}: export it as a kind of its module in export/registry.rs, \
                 or list it there as not exported with the reason"
            )),
            (false, 1) => {}
            (false, _) => problems.push(format!("{table}: declared more than once")),
        }
        if !global && !workspace_tables.contains(table) {
            problems.push(format!("{table}: not in tenancy::tables"));
        }
    }
    for table in declared.keys() {
        if !schema.columns.contains_key(*table) {
            problems.push(format!("{table}: declared but not in the schema"));
        }
    }
    problems
}

#[tokio::test]
async fn every_workspace_table_is_exported_or_says_why() {
    let Some((pool, name)) = migrated_schema("export_declarations").await else {
        return;
    };
    let mut found = schema(&pool, &name).await;
    found
        .columns
        .entry("_sqlx_migrations".to_owned())
        .or_default();
    let declarations = export::parts()
        .map(|part| part.declaration())
        .collect::<Vec<_>>();
    let wrong = problems(&declarations, &found);
    assert!(wrong.is_empty(), "{wrong:#?}");

    // The guard has teeth: a new table no module declares fails it, and so
    // does a new reference column.
    let mut with_table = Schema {
        columns: found.columns.clone(),
        references: found.references.clone(),
    };
    with_table.columns.insert(
        "new_things".to_owned(),
        BTreeSet::from(["id".to_owned(), "workspace_id".to_owned()]),
    );
    assert!(
        problems(&declarations, &with_table)
            .iter()
            .any(|problem| problem.starts_with("new_things: export it"))
    );
    let mut with_reference = Schema {
        columns: found.columns.clone(),
        references: found.references.clone(),
    };
    with_reference
        .columns
        .get_mut("work_items")
        .expect("work_items")
        .insert("reviewer_actor_id".to_owned());
    with_reference
        .references
        .insert(("work_items".to_owned(), "reviewer_actor_id".to_owned()));
    assert!(
        problems(&declarations, &with_reference)
            .iter()
            .any(|problem| problem.starts_with("work_items.reviewer_actor_id"))
    );

    // The generic exporter takes one workspace's rows and nothing else's,
    // and the importer puts them back as they were.
    let mut workspaces = Vec::new();
    for slug in ["export-a", "export-b"] {
        let workspace = Uuid::now_v7();
        sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
            .bind(workspace)
            .bind(slug)
            .bind(format!("{slug}-{}", workspace.simple()))
            .execute(&pool)
            .await
            .expect("workspace");
        sqlx::query(
            "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'EX', $3)",
        )
        .bind(Uuid::now_v7())
        .bind(workspace)
        .bind(format!("{slug} project"))
        .execute(&pool)
        .await
        .expect("project");
        workspaces.push(workspace);
    }
    let (module, kind) = declarations
        .iter()
        .find_map(|declaration| {
            declaration
                .kinds
                .iter()
                .find(|kind| kind.table == "projects")
                .map(|kind| (*declaration, kind))
        })
        .expect("projects are declared");
    let mut connection = pool.acquire().await.expect("connection");
    let lines = export::export_kind(&mut connection, module, kind, workspaces[0])
        .await
        .expect("export");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].data["name"], "export-a project");
    let before = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT to_jsonb(p) FROM projects p WHERE organization_id = $1",
    )
    .bind(workspaces[0])
    .fetch_one(&mut *connection)
    .await
    .expect("row");
    drop(connection);

    // Into a fresh schema, where the workspace exists and its project doesn't.
    let (target, target_name) = migrated_schema("export_import")
        .await
        .expect("a second schema");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'export-a', $2)")
        .bind(workspaces[0])
        .bind(format!("export-a-{}", workspaces[0].simple()))
        .execute(&target)
        .await
        .expect("workspace");
    let mut connection = target.acquire().await.expect("connection");
    let inserted = export::import_kind(&mut connection, module, kind, &lines)
        .await
        .expect("import");
    assert_eq!(inserted, 1);
    let after = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT to_jsonb(p) FROM projects p WHERE organization_id = $1",
    )
    .bind(workspaces[0])
    .fetch_one(&mut *connection)
    .await
    .expect("row");
    assert_eq!(after, before);
    drop(connection);
    agent_workspace_server::test_support::drop_schema(&target, &target_name)
        .await
        .expect("drop the second schema");
    target.close().await;

    agent_workspace_server::test_support::drop_schema(&pool, &name)
        .await
        .expect("drop the test schema");
    pool.close().await;
}

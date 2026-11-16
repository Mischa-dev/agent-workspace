//! H1.21, H1.22: row security as a second net under the isolation tests.
//! The silent failures: a tenant table with no policy (so the net has a
//! hole nobody sees), a scope that lets one workspace read or write
//! another's rows once strict, an unscoped query that still reads in strict
//! mode, and observe mode refusing something before strict mode is meant to
//! be on.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use agent_workspace_server::tenancy::tables::{DIRECT_SCOPE_TABLES, TENANT_TABLES};

use super::{a1_runs_record::seed, support::db::row_rules_role, tenancy_tables::migrated_schema};

/// What is wrong with `tables`' row rules, in words: each must have row
/// security on and forced, and one policy named `tenancy` that applies
/// `tenancy_allows` to its tenant column for reads and writes (and, on a
/// table that may hold direct-scope rows, the direct branch too).
async fn policy_problems(pool: &PgPool, tables: &[(&str, &str)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (table, column) in tables {
        let found = sqlx::query_as::<_, (bool, bool, Option<String>, Option<String>)>(
            r#"
            SELECT class.relrowsecurity, class.relforcerowsecurity,
                   pg_get_expr(policy.polqual, policy.polrelid),
                   pg_get_expr(policy.polwithcheck, policy.polrelid)
            FROM pg_class class
            LEFT JOIN pg_policy policy ON policy.polrelid = class.oid AND policy.polname = 'tenancy'
            WHERE class.oid = $1::regclass
            "#,
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .expect("read a table's row rules");
        let rule = format!("tenancy_allows({column})");
        let direct = DIRECT_SCOPE_TABLES.contains(table);
        let covers = |expression: &str| {
            expression.contains(&rule) && (!direct || expression.contains("tenancy_direct_allows"))
        };
        match found {
            (true, true, Some(using), Some(check)) if covers(&using) && covers(&check) => {}
            other => problems.push(format!(
                "{table}: {other:?}; protect it in its migration with \
                 SELECT tenancy_protect('{table}', '{column}'{})",
                if direct { ", '<its conversation>'" } else { "" }
            )),
        }
    }
    problems
}

/// One transaction as the server's role would run it: bound by row rules,
/// in `scope` (None: no scope set), strict or observing.
async fn as_server<'p>(
    pool: &'p PgPool,
    role: Option<&str>,
    scope: Option<&str>,
    strict: bool,
) -> Transaction<'p, Postgres> {
    let mut transaction = pool.begin().await.expect("begin");
    if let Some(role) = role {
        sqlx::query(&format!("SET LOCAL ROLE {role}"))
            .execute(&mut *transaction)
            .await
            .expect("take the server's role");
    }
    if strict {
        sqlx::query("SET LOCAL app.tenancy_strict = 'on'")
            .execute(&mut *transaction)
            .await
            .expect("strict");
    }
    if let Some(scope) = scope {
        sqlx::query("SELECT set_config('app.tenancy', $1, true)")
            .bind(scope)
            .execute(&mut *transaction)
            .await
            .expect("scope");
    }
    transaction
}

async fn count(
    pool: &PgPool,
    role: Option<&str>,
    scope: Option<&str>,
    strict: bool,
    query: &str,
    project: Uuid,
) -> i64 {
    let mut transaction = as_server(pool, role, scope, strict).await;
    let found = sqlx::query_scalar::<_, i64>(query)
        .bind(project)
        .fetch_one(&mut *transaction)
        .await
        .expect("count under a scope");
    transaction.rollback().await.expect("rollback");
    found
}

#[tokio::test]
async fn rows_follow_the_scope_once_strict_and_nothing_is_refused_before() {
    let Some((pool, schema)) = migrated_schema("tenancy_row_rules").await else {
        return;
    };
    let root = tempfile::tempdir().expect("data root");
    let seed = seed(&pool, &root).await;
    let role = row_rules_role(&pool, &schema).await;
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Other', $2)")
        .bind(other)
        .bind(format!("other-{}", other.simple()))
        .execute(&pool)
        .await
        .expect("another workspace");

    // A message in the seeded thread, so the counts below have rows.
    sqlx::query(
        r#"
        INSERT INTO messages (id, project_id, conversation_id, author_actor_id, content, plain_text)
        VALUES ($1, $2, $3, $4,
                jsonb_build_object('version', 1, 'nodes',
                    jsonb_build_array(jsonb_build_object('type', 'text', 'text', 'Hello'))),
                'Hello')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(seed.project_id)
    .bind(seed.thread_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("a message in the thread");

    // The guard: every tenant table has its policy.
    let problems = policy_problems(&pool, TENANT_TABLES).await;
    assert!(problems.is_empty(), "{problems:#?}");

    let conversations = "SELECT count(*) FROM conversations WHERE project_id = $1";
    let messages = "SELECT count(*) FROM messages WHERE project_id = $1";
    let all = count(&pool, None, None, false, conversations, seed.project_id).await;
    assert!(all >= 2, "the seed has a channel and a thread");
    let all_messages = count(&pool, None, None, false, messages, seed.project_id).await;
    assert!(all_messages >= 1);
    let own = format!("w:|{}", seed.organization_id);
    let elsewhere = format!("w:|{other}");
    let member_of_both = format!(
        "m:{}|{},{}",
        seed.human_actor_id, seed.organization_id, other
    );
    let member_elsewhere = format!("m:{}|{other}", seed.human_actor_id);
    let cases: &[(Option<&str>, bool, i64)] = &[
        // Observe mode refuses nothing, scoped or not.
        (None, false, all),
        (Some(&own), false, all),
        (Some(&elsewhere), false, all),
        (Some("a"), false, all),
        // Strict: a scope reads its own workspace's rows only, and an
        // unscoped query reads nothing.
        (None, true, 0),
        (Some(&own), true, all),
        (Some(&elsewhere), true, 0),
        (Some(&member_of_both), true, all),
        (Some(&member_elsewhere), true, 0),
        (Some("a"), true, 0),
        (Some("s:jobs.claim"), true, all),
    ];
    for (scope, strict, expected) in cases {
        assert_eq!(
            count(&pool, role, *scope, *strict, conversations, seed.project_id).await,
            *expected,
            "conversations under {scope:?}, strict {strict}"
        );
        let expected_messages = if *expected == 0 { 0 } else { all_messages };
        assert_eq!(
            count(&pool, role, *scope, *strict, messages, seed.project_id).await,
            expected_messages,
            "messages under {scope:?}, strict {strict}"
        );
    }

    // Writes follow the same rule: once strict, another workspace's scope
    // changes nothing and can't move a row into it.
    let mut transaction = as_server(&pool, role, Some(&elsewhere), true).await;
    let changed =
        sqlx::query("UPDATE conversations SET updated_at = updated_at WHERE project_id = $1")
            .bind(seed.project_id)
            .execute(&mut *transaction)
            .await
            .expect("update under another scope")
            .rows_affected();
    assert_eq!(changed, 0);
    transaction.rollback().await.expect("rollback");
    let mut transaction = as_server(&pool, role, Some(&own), true).await;
    let moved = sqlx::query("UPDATE conversations SET workspace_id = $2 WHERE project_id = $1")
        .bind(seed.project_id)
        .bind(other)
        .execute(&mut *transaction)
        .await;
    assert!(moved.is_err(), "a row was written outside the scope");
    transaction.rollback().await.expect("rollback");

    // The direct scope (R20): a DM's rows are invisible under every member
    // and workspace scope, including a scope naming every workspace its
    // participants belong to as their admins, and visible to a participant
    // from any scope. K4 makes the direct scope and owns the helper's body;
    // here the seeded thread stands in for a DM, moved
    // into a stand-in direct workspace, and the helper names one participant.
    let direct = Uuid::now_v7();
    let dm = seed.thread_id;
    let mut transaction = pool.begin().await.expect("begin");
    for statement in [
        format!(
            "INSERT INTO organizations (id, name, slug) VALUES ('{direct}', 'Direct', 'direct-{}')",
            direct.simple()
        ),
        "ALTER TABLE messages DISABLE TRIGGER USER".to_owned(),
        format!("UPDATE conversations SET workspace_id = '{direct}' WHERE id = '{dm}'"),
        format!("UPDATE messages SET workspace_id = '{direct}' WHERE conversation_id = '{dm}'"),
        "ALTER TABLE messages ENABLE TRIGGER USER".to_owned(),
        format!(
            "CREATE OR REPLACE FUNCTION tenancy_direct_allows(workspace UUID, conversation UUID) \
             RETURNS BOOLEAN LANGUAGE sql STABLE AS $$ SELECT workspace = '{direct}' \
             AND conversation = '{}' AND tenancy_account() = '{}' $$",
            dm, seed.collaborator_actor_id
        ),
    ] {
        sqlx::query(&statement)
            .execute(&mut *transaction)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    let in_dm =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE conversation_id = $1")
            .bind(dm)
            .fetch_one(&mut *transaction)
            .await
            .expect("the DM's messages");
    assert!(in_dm > 0, "the stand-in DM has messages");
    if let Some(role) = role {
        sqlx::query(&format!("SET LOCAL ROLE {role}"))
            .execute(&mut *transaction)
            .await
            .expect("role");
    }
    sqlx::query("SET LOCAL app.tenancy_strict = 'on'")
        .execute(&mut *transaction)
        .await
        .expect("strict");
    let human = seed.human_actor_id;
    let collaborator = seed.collaborator_actor_id;
    let organization = seed.organization_id;
    let dm_scopes = [
        (format!("w:|{organization}"), 0),
        (format!("w:{human}|{organization}"), 0),
        (format!("m:{human}|{organization},{other}"), 0),
        (format!("m:{collaborator}|{organization}"), in_dm),
        (format!("m:{collaborator}|{other}"), in_dm),
        (format!("w:{collaborator}|{other}"), in_dm),
        ("a".to_owned(), 0),
    ];
    for (scope, expected) in dm_scopes {
        sqlx::query("SELECT set_config('app.tenancy', $1, true)")
            .bind(&scope)
            .execute(&mut *transaction)
            .await
            .expect("scope");
        let seen = sqlx::query_as::<_, (i64, i64)>(
            "SELECT (SELECT count(*) FROM messages WHERE conversation_id = $1), \
                    (SELECT count(*) FROM conversations WHERE id = $1)",
        )
        .bind(dm)
        .fetch_one(&mut *transaction)
        .await
        .expect("read the DM");
        assert_eq!(
            seen,
            (expected, i64::from(expected > 0)),
            "the DM under {scope}"
        );
    }
    transaction.rollback().await.expect("rollback");

    agent_workspace_server::test_support::drop_schema(&pool, &schema)
        .await
        .expect("drop the test schema");
    pool.close().await;
}

//! H1.21, H1.22: row security as a second net under the isolation tests.
//! The silent failures: a tenant table with no policy (so the net has a
//! hole nobody sees), a scope that lets one workspace read or write
//! another's rows once strict, an unscoped query that still reads in strict
//! mode, and observe mode refusing something before strict mode is meant to
//! be on.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{a1_runs_record::seed, support::db::row_rules_role, tenancy_tables::migrated_schema};

/// H1.4's tables, the first batch with row rules (H1.21).
const CHAT_TABLES: &[&str] = &[
    "conversations",
    "conversation_entries",
    "conversation_members",
    "conversation_notification_levels",
    "conversation_pins",
    "conversation_read_states",
    "messages",
    "message_link_previews",
    "message_mentions",
    "message_reactions",
    "message_revisions",
    "message_work_item_references",
    "reply_requests",
    "notifications",
    "notification_occurrences",
    "notification_project_mutes",
    "outbox_events",
    "person_questions",
    "project_decisions",
    "push_deliveries",
    "thing_references",
];

/// What is wrong with `tables`' row rules, in words: each must have row
/// security on and forced, and one policy named `tenancy` that applies
/// `tenancy_allows` to its tenant column for reads and writes.
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
        match found {
            (true, true, Some(using), Some(check))
                if using.contains(&rule) && check.contains(&rule) => {}
            other => problems.push(format!("{table}: {other:?}, expected {rule}")),
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
async fn chat_rows_follow_the_scope_once_strict_and_nothing_is_refused_before() {
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

    // The guard: every chat table has its policy.
    let chat = CHAT_TABLES
        .iter()
        .map(|table| (*table, "workspace_id"))
        .collect::<Vec<_>>();
    let problems = policy_problems(&pool, &chat).await;
    assert!(problems.is_empty(), "{problems:#?}");

    let conversations = "SELECT count(*) FROM conversations WHERE project_id = $1";
    let messages = "SELECT count(*) FROM messages WHERE project_id = $1";
    let all = count(&pool, None, None, false, conversations, seed.project_id).await;
    assert!(all >= 2, "the seed has a channel and a thread");
    let all_messages = count(&pool, None, None, false, messages, seed.project_id).await;
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

    // The direct scope's branch (R20) reaches every chat table through the
    // row's conversation: a participant reads their DM from any scope. K4
    // owns the helper's body; here a stand-in names one conversation and one
    // account.
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::query(&format!(
        "CREATE OR REPLACE FUNCTION tenancy_direct_allows(workspace UUID, conversation UUID) \
         RETURNS BOOLEAN LANGUAGE sql STABLE AS $$ SELECT conversation = '{}' AND tenancy_account() = '{}' $$",
        seed.thread_id, seed.collaborator_actor_id
    ))
    .execute(&mut *transaction)
    .await
    .expect("stand-in direct branch");
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
    sqlx::query("SELECT set_config('app.tenancy', $1, true)")
        .bind(format!("m:{}|{other}", seed.collaborator_actor_id))
        .execute(&mut *transaction)
        .await
        .expect("a participant elsewhere");
    let seen = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM conversations WHERE project_id = $1),
               (SELECT count(*) FROM messages WHERE conversation_id = $2)
                 - (SELECT count(*) FROM messages WHERE project_id = $1)
        "#,
    )
    .bind(seed.project_id)
    .bind(seed.thread_id)
    .fetch_one(&mut *transaction)
    .await
    .expect("read the DM as a participant");
    assert_eq!(
        seen,
        (1, 0),
        "only the participant's conversation, and its messages"
    );
    transaction.rollback().await.expect("rollback");

    agent_workspace_server::test_support::drop_schema(&pool, &schema)
        .await
        .expect("drop the test schema");
    pool.close().await;
}

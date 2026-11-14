//! H1.20: the tenancy scope reaches PostgreSQL as `app.tenancy`, set only
//! inside a transaction. The costly failures are silent: a job running with
//! another workspace's scope, an account's scope naming a workspace it left,
//! or one transaction's scope still set on a pooled connection when the next
//! transaction (someone else's request, behind a transaction-mode pooler)
//! gets it.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use agent_workspace_server::{
    AppState,
    config::AppConfig,
    db,
    jobs::{self, JobError, JobRegistry, NewJob, WorkerOptions},
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    tenancy::{self, Tenancy},
};
use sqlx::PgPool;
use uuid::Uuid;

use super::tenancy_tables::migrated_schema;

async fn workspace(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Scope', $2)")
        .bind(id)
        .bind(format!("scope-{}", id.simple()))
        .execute(pool)
        .await
        .expect("workspace");
    id
}

async fn account(pool: &PgPool, kind: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, $2)")
        .bind(id)
        .bind(kind)
        .execute(pool)
        .await
        .expect("account");
    id
}

async fn join(pool: &PgPool, workspace: Uuid, account: Uuid, enabled: bool) {
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'member', $3)",
    )
    .bind(workspace)
    .bind(account)
    .bind(enabled)
    .execute(pool)
    .await
    .expect("membership");
}

/// `app.tenancy` as a scoped transaction sees it.
async fn seen(pool: &PgPool) -> String {
    db::scoped_one(pool, async |connection| {
        sqlx::query_scalar::<_, String>("SELECT coalesce(current_setting('app.tenancy', true), '')")
            .fetch_one(connection)
            .await
    })
    .await
    .expect("read the scope")
}

fn sorted(mut ids: Vec<Uuid>) -> String {
    ids.sort_unstable();
    ids.iter().map(Uuid::to_string).collect::<Vec<_>>().join(",")
}

#[tokio::test]
async fn each_scope_reaches_the_database_and_ends_with_its_transaction() {
    let Some((pool, schema)) = migrated_schema("tenancy_scope").await else {
        return;
    };
    let a = workspace(&pool).await;
    let b = workspace(&pool).await;
    let left = workspace(&pool).await;
    // An agent in two workspaces, and one it was removed from.
    let agent = account(&pool, "external_agent").await;
    join(&pool, a, agent, true).await;
    join(&pool, b, agent, true).await;
    join(&pool, left, agent, false).await;

    // An account's scope is every workspace it belongs to now, of any kind
    // of account, looked up when first needed.
    let member = tenancy::scoped(
        Tenancy::Member {
            account: agent,
            workspaces: None,
        },
        seen(&pool),
    )
    .await;
    assert_eq!(member, format!("m:{agent}|{}", sorted(vec![a, b])));

    // A request starts anonymous; signing in makes it the account's.
    let request = tenancy::scoped(Tenancy::Anonymous, async {
        let before = seen(&pool).await;
        tenancy::signed_in(agent);
        (before, seen(&pool).await)
    })
    .await;
    assert_eq!(request.0, "a");
    assert_eq!(request.1, member);

    let narrowed = tenancy::scoped(
        Tenancy::Workspace {
            account: Some(agent),
            workspace: a,
        },
        seen(&pool),
    )
    .await;
    assert_eq!(narrowed, format!("w:{agent}|{a}"));

    // Outside any scope a scoped transaction carries none (observe mode).
    assert_eq!(seen(&pool).await, "");

    // Nothing outlives its transaction: on one connection, the next
    // transaction and plain statements see no scope (what a
    // transaction-mode pooler relies on).
    let single = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect({
            let schema = schema.clone();
            move |connection, _| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(
                        connection,
                        format!("SET search_path TO {schema}, public").as_str(),
                    )
                    .await?;
                    Ok(())
                })
            }
        })
        .connect(&std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL"))
        .await
        .expect("one connection");
    let inside = tenancy::scoped(
        Tenancy::Workspace {
            account: None,
            workspace: b,
        },
        seen(&single),
    )
    .await;
    assert_eq!(inside, format!("w:|{b}"));
    let after = sqlx::query_scalar::<_, String>(
        "SELECT coalesce(current_setting('app.tenancy', true), '')",
    )
    .fetch_one(&single)
    .await
    .expect("read after");
    assert_eq!(after, "", "a scope outlived its transaction");
    // A rolled-back transaction leaves nothing either.
    let transaction = tenancy::scoped(Tenancy::System("jobs.claim"), db::scoped_tx(&single))
        .await
        .expect("begin");
    drop(transaction);
    assert_eq!(seen(&single).await, "");
    single.close().await;

    // A job runs in the scope of the workspace it was queued for.
    let config = AppConfig::from_values(&BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL"),
    )]))
    .expect("config");
    let supervisors =
        agent_workspace_server::provider::SupervisorRegistry::new(&config).expect("supervisors");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
        supervisors,
    };
    let scopes = Arc::new(Mutex::new(Vec::new()));
    let mut registry = JobRegistry::new();
    registry.register("tenancy.test", {
        let scopes = Arc::clone(&scopes);
        move |context: jobs::JobContext| {
            let scopes = Arc::clone(&scopes);
            async move {
                let scope = seen(&context.state.pool).await;
                scopes.lock().expect("scopes").push(scope);
                Ok::<(), JobError>(())
            }
        }
    });
    jobs::enqueue(&pool, NewJob::new("tenancy.test").workspace(b))
        .await
        .expect("queue a workspace job");
    jobs::enqueue(&pool, NewJob::new("tenancy.test"))
        .await
        .expect("queue a server job");
    let ran = jobs::run_due(&state, &registry, &WorkerOptions::default())
        .await
        .expect("run jobs");
    assert_eq!(ran, 2);
    let mut scopes = scopes.lock().expect("scopes").clone();
    scopes.sort();
    assert_eq!(scopes, ["s:jobs.server".to_owned(), format!("w:|{b}")]);

    agent_workspace_server::test_support::drop_schema(&pool, &schema)
        .await
        .expect("drop the test schema");
    pool.close().await;
}

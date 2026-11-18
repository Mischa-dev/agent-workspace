//! Shared integration fixtures: the app, its databases, the callers that
//! reach it (a person, an agent, a connected app) and worlds built from
//! them. New tests start here; the older files re-export what moved.
//! Later workstreams build on this, so not every helper has a caller yet.
#![allow(dead_code)]

pub(crate) mod app;
pub(crate) mod callers;
pub(crate) mod db;
pub(crate) mod live;
pub(crate) mod requests;
pub(crate) mod test_host;
pub(crate) mod two_workspaces;
pub(crate) mod world;

/// Moves every agent holding a role with `pages.edit` to a "suggests" copy
/// of it (`sql/agents_suggest.sql`). Tests that set an agent up and
/// then expect its edits to others' pages to be suggestions call this after
/// the setup.
pub(crate) async fn agents_suggest(pool: &sqlx::PgPool) {
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::raw_sql(include_str!("sql/agents_suggest.sql"))
        .execute(&mut *transaction)
        .await
        .expect("suggest roles");
    transaction.commit().await.expect("commit");
}

/// Every change notice `outbox::enqueue` staged in the log, whether the
/// sequencer has given it a position yet or not, with the columns tests ask
/// about. Staged notices are numbered after the log's head in staging
/// order, which is the order they will get. Use it with an alias:
/// `FROM {NOTICES} notice`.
pub(crate) const NOTICES: &str = "(SELECT id AS event_id, legacy_type AS event_type, \
    space_id AS project_id, \
    CASE WHEN legacy_type = 'conversation.member.removed' THEN NULL \
         ELSE (payload ->> 'conversationId')::uuid END AS conversation_id, \
    CASE WHEN legacy_type = 'conversation.read_state.updated' THEN audience_account_id \
         ELSE COALESCE(thing_id, audience_account_id) END AS resource_id, \
    audience, payload, created_at, \
    event_cursor \
    FROM (SELECT position AS event_cursor, id, legacy_type, space_id, thing_id, \
                 audience_account_id, audience, payload, created_at FROM events \
          UNION ALL \
          SELECT (SELECT next_position FROM event_position_counter) + ordinal, id, legacy_type, \
                 space_id, thing_id, audience_account_id, audience, payload, created_at \
          FROM event_staging) logged \
    WHERE legacy_type IS NOT NULL)";

/// The run record of a built-in run a test writes by hand (the server opens
/// it with `runs::builtin::open`). Call it before inserting the run's
/// `agent_runs` row, whose id references it; `state` is the built-in state
/// the test gives that row.
pub(crate) async fn builtin_run_record(
    executor: impl sqlx::PgExecutor<'_>,
    run_id: uuid::Uuid,
    conversation_id: uuid::Uuid,
    execution_worker_id: uuid::Uuid,
    state: &str,
) {
    let built_in = state
        .parse::<agent_workspace_server::domain::RunState>()
        .expect("a built-in run state");
    let state = agent_workspace_server::runs::state::RunState::from_legacy(built_in, None);
    sqlx::query(
        r#"
        INSERT INTO runs (
            id, workspace_id, space_id, account_id, title, cause_kind, conversation_id,
            state, finished_at
        )
        SELECT $1, project.organization_id, project.id, worker.actor_id, 'Run', 'thread',
               conversation.id, $4, CASE WHEN $5 THEN now() END
          FROM conversations conversation
          JOIN projects project ON project.id = conversation.project_id
          JOIN execution_workers worker ON worker.id = $3
         WHERE conversation.id = $2
        "#,
    )
    .bind(run_id)
    .bind(conversation_id)
    .bind(execution_worker_id)
    .bind(state.as_str())
    .bind(state.is_terminal())
    .execute(executor)
    .await
    .expect("built-in run record");
}

/// Move a built-in run's record to the built-in state a test set by hand on
/// its `agent_runs` row (the server moves both through
/// `hosts::builtin::transition`).
pub(crate) async fn builtin_run_moved(
    executor: impl sqlx::PgExecutor<'_>,
    run_id: uuid::Uuid,
    state: &str,
) {
    let built_in = state
        .parse::<agent_workspace_server::domain::RunState>()
        .expect("a built-in run state");
    let state = agent_workspace_server::runs::state::RunState::from_legacy(built_in, None);
    sqlx::query(
        "UPDATE runs SET state = $2, finished_at = CASE WHEN $3 THEN now() END WHERE id = $1",
    )
    .bind(run_id)
    .bind(state.as_str())
    .bind(state.is_terminal())
    .execute(executor)
    .await
    .expect("built-in run moved");
}

/// The events heard by one account alone (`Audience::Account`), oldest
/// first, as `(type, thing id)`, after the log has given every staged event
/// its position.
pub(crate) async fn addressed_events(
    pool: &sqlx::PgPool,
    account: uuid::Uuid,
) -> Vec<(String, Option<uuid::Uuid>)> {
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("sequence events");
    sqlx::query_as::<_, (String, Option<uuid::Uuid>)>(
        "SELECT type, thing_id FROM events WHERE audience_account_id = $1 ORDER BY position",
    )
    .bind(account)
    .fetch_all(pool)
    .await
    .expect("addressed events")
}

/// How many events of `kind` one account alone heard.
pub(crate) async fn addressed_count(pool: &sqlx::PgPool, account: uuid::Uuid, kind: &str) -> usize {
    addressed_events(pool, account)
        .await
        .iter()
        .filter(|(found, _)| found == kind)
        .count()
}

/// What `account` hears on its default connection since that connection
/// was made: the first call makes it, so call once before the change and
/// again after. Events are sequenced first.
pub(crate) async fn heard(
    state: &agent_workspace_server::AppState,
    account: uuid::Uuid,
) -> Vec<serde_json::Value> {
    agent_workspace_server::events::catch_up(&state.pool)
        .await
        .expect("sequence events");
    let caller = agent_workspace_server::access::Caller::person(
        account,
        agent_workspace_server::policy::ActorKind::ExternalAgent,
    );
    agent_workspace_server::tenancy::system(
        "events.log",
        agent_workspace_server::delivery::poll(
            state,
            &caller,
            None,
            None,
            100,
            std::time::Duration::ZERO,
        ),
    )
    .await
    .expect("events")
    .events
}

/// Run the jobs that are due now (such as `runs.say` posting what a run's
/// agent said in its thread), as the workers would.
pub(crate) async fn run_due_jobs(state: &agent_workspace_server::AppState) {
    use agent_workspace_server::jobs;
    for _ in 0..2 {
        jobs::run_due(
            state,
            &jobs::default_registry(),
            &jobs::WorkerOptions {
                concurrency: 1,
                poll_interval: std::time::Duration::from_millis(20),
                lease: std::time::Duration::from_secs(30),
                worker_id: "integration-tests".to_owned(),
            },
        )
        .await
        .expect("run due jobs");
    }
}

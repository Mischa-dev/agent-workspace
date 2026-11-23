//! Shared integration fixtures: the app, its databases, the callers that
//! reach it (a person, an agent, a connected app) and worlds built from
//! them. New tests start here; the older files re-export what moved.
//! Later workstreams build on this, so not every helper has a caller yet.
#![allow(dead_code)]

pub(crate) mod app;
pub(crate) mod callers;
pub(crate) mod db;
pub(crate) mod legacy;
pub(crate) mod live;
pub(crate) mod long;
pub(crate) mod requests;
pub(crate) mod tasks;
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
    executor: impl sqlx::Acquire<'_, Database = sqlx::Postgres>,
    run_id: uuid::Uuid,
    conversation_id: uuid::Uuid,
    execution_worker_id: uuid::Uuid,
    state: &str,
) {
    let built_in = state
        .parse::<agent_workspace_server::domain::RunState>()
        .expect("a built-in run state");
    let state = agent_workspace_server::runs::state::RunState::from_legacy(built_in, None);
    let mut connection = executor.acquire().await.expect("connection");
    sqlx::query(
        r#"
        INSERT INTO runs (
            id, workspace_id, space_id, account_id, title, cause_kind, conversation_id,
            state, finished_at, host_id
        )
        SELECT $1, project.organization_id, project.id, worker.actor_id, 'Run', 'thread',
               conversation.id, $4, CASE WHEN $5 THEN now() END, host.id
          FROM conversations conversation
          JOIN projects project ON project.id = conversation.project_id
          JOIN execution_workers worker ON worker.id = $3
          LEFT JOIN hosts host ON host.owner_workspace_id = project.organization_id AND host.builtin
         WHERE conversation.id = $2
        "#,
    )
    .bind(run_id)
    .bind(conversation_id)
    .bind(execution_worker_id)
    .bind(state.as_str())
    .bind(state.is_terminal())
    .execute(&mut *connection)
    .await
    .expect("built-in run record");
    // As `runs::builtin::open` does: the run's agent joins a members-only
    // channel it works in.
    sqlx::query(
        r#"
        INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
        SELECT root.project_id, root.id, worker.actor_id, worker.actor_id
          FROM conversations conversation
          JOIN conversations root ON root.id = COALESCE(conversation.parent_channel_id, conversation.id)
          JOIN execution_workers worker ON worker.id = $2
         WHERE conversation.id = $1 AND root.kind = 'channel' AND root.visibility = 'members'
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(conversation_id)
    .bind(execution_worker_id)
    .execute(&mut *connection)
    .await
    .expect("the run's agent joins its channel");
}

/// Give a built-in run the start the built-in host carried out for it, as
/// the scheduler and its driver do, so the host can report on it with the
/// credential the start brings.
pub(crate) async fn builtin_run_started(pool: &sqlx::PgPool, run_id: uuid::Uuid) {
    let workspace = run_workspace(pool, run_id).await;
    as_job(workspace, async {
        let mut transaction = agent_workspace_server::db::scoped_tx(pool)
            .await
            .expect("transaction");
        agent_workspace_server::scheduler::issue_start(&mut transaction, run_id, "test")
            .await
            .expect("start");
        transaction.commit().await.expect("commit");
    })
    .await;
    sqlx::query(
        "UPDATE agent_runs SET start_command_id = (SELECT id FROM host_commands WHERE run_id = $1 AND kind = 'start') WHERE id = $1",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .expect("keep the start");
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

/// Runs `work` as the job runner runs a job of `workspace`: in that
/// workspace's scope, so under strict row rules (H1.63) it sees that
/// workspace's rows and no other's. For tests that call a job's function
/// directly instead of through the queue.
pub(crate) async fn as_job<F: std::future::Future>(workspace: uuid::Uuid, work: F) -> F::Output {
    agent_workspace_server::tenancy::scoped(
        agent_workspace_server::tenancy::Tenancy::Workspace {
            account: None,
            workspace,
        },
        work,
    )
    .await
}

/// Runs `work` as the job runner runs a job queued for no workspace (a
/// periodic sweep): the server's own work, server-wide. For tests that
/// call such a job's function directly instead of through the queue.
pub(crate) async fn as_server_job<F: std::future::Future>(work: F) -> F::Output {
    agent_workspace_server::tenancy::system("jobs.server", work).await
}

/// Runs `work` as `account` would in a request of theirs: in the scope of
/// every workspace they belong to. For tests that call a function a
/// signed-in request runs.
pub(crate) async fn as_member<F: std::future::Future>(account: uuid::Uuid, work: F) -> F::Output {
    agent_workspace_server::tenancy::scoped(
        agent_workspace_server::tenancy::Tenancy::Member {
            account,
            workspaces: None,
        },
        work,
    )
    .await
}

/// The workspace a space is in.
pub(crate) async fn workspace_of_space(pool: &sqlx::PgPool, space: uuid::Uuid) -> uuid::Uuid {
    sqlx::query_scalar("SELECT organization_id FROM projects WHERE id = $1")
        .bind(space)
        .fetch_one(pool)
        .await
        .expect("the space's workspace")
}

/// Runs an export as its job would: in the scope of the workspace it's for.
pub(crate) async fn run_export(
    state: &agent_workspace_server::AppState,
    transfer: uuid::Uuid,
) -> Result<(), agent_workspace_server::error::AppError> {
    let workspace: uuid::Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workspace_transfers WHERE id = $1")
            .bind(transfer)
            .fetch_one(&state.pool)
            .await
            .expect("the export's workspace");
    as_job(
        workspace,
        agent_workspace_server::export::transfer::run_export(state, transfer),
    )
    .await
}

/// The workspace a run belongs to, for running its host's work in that
/// workspace's scope ([`as_job`]).
pub(crate) async fn run_workspace(pool: &sqlx::PgPool, run_id: uuid::Uuid) -> uuid::Uuid {
    sqlx::query_scalar("SELECT workspace_id FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("the run's workspace")
}

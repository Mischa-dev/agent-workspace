//! A1.1: the run record. The silent failures: a run the built-in host
//! started before the upgrade missing from `runs`, or carried across in the
//! wrong state; the mirror letting `runs` drift from `agent_runs`; and the
//! activities backfill copying an event twice.

use std::collections::BTreeMap;

use agent_workspace_server::{
    config::AppConfig,
    domain::RunState as Legacy,
    runs::{self, RunState},
    seed::{DevSeed, dev_seed},
};
use sqlx::{PgPool, migrate::Migrator};
use uuid::Uuid;

use super::agent_interface::schema_pool;

/// The first migration of the run record.
const RUNS_MIGRATION: i64 = 20261114072611;

/// A schema migrated up to (not including) the run record, and the full
/// migrator to finish with.
async fn schema_before_runs(prefix: &str) -> Option<(PgPool, Migrator, tempfile::TempDir)> {
    schema_before(prefix, RUNS_MIGRATION).await
}

/// A schema migrated up to (not including) `version`, and the full
/// migrator to finish with.
pub(super) async fn schema_before(
    prefix: &str,
    version: i64,
) -> Option<(PgPool, Migrator, tempfile::TempDir)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    let full = sqlx::migrate!("./migrations");
    let mut before = sqlx::migrate!("./migrations");
    before.migrations = before
        .migrations
        .iter()
        .filter(|migration| migration.version < version)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    before.run(&pool).await.expect("migrate to before runs");
    let root = tempfile::tempdir().expect("data root");
    Some((pool, full, root))
}

pub(super) async fn seed(pool: &PgPool, root: &tempfile::TempDir) -> DevSeed {
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            "postgres://unused".to_owned(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("development config");
    let seed = dev_seed(pool, &config).await.expect("seed");
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(seed.execution_profile_id)
        .execute(pool)
        .await
        .expect("authenticate profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(pool)
    .await
    .expect("prepare repository");
    seed
}

pub(super) async fn insert_run(
    pool: &PgPool,
    seed: &DevSeed,
    state: &str,
    reason: Option<&str>,
    prompt: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    let terminal = matches!(state, "completed" | "failed" | "canceled" | "interrupted");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, failure_reason, started_at, finished_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '{}'::jsonb, $9, now(),
                CASE WHEN $10 THEN now() END)
        "#,
    )
    .bind(id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .bind(state)
    .bind(prompt)
    .bind(reason)
    .bind(terminal)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("insert {state} run: {error}"));
    id
}

async fn insert_event(pool: &PgPool, run: Uuid, kind: &str, payload: serde_json::Value) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (id, agent_run_id, event_key, kind, summary, payload)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(run)
    .bind(format!("test:{id}"))
    .bind(kind)
    .bind(format!("A {kind} event."))
    .bind(payload)
    .execute(pool)
    .await
    .expect("insert event");
    id
}

async fn run_state(pool: &PgPool, run: Uuid) -> (String, Option<String>, bool) {
    sqlx::query_as::<_, (String, Option<String>, bool)>(
        "SELECT state, status_line, finished_at IS NOT NULL FROM runs WHERE id = $1",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .expect("runs row")
}

#[tokio::test]
async fn existing_runs_usage_and_events_are_carried_across_once() {
    let Some((pool, full, root)) = schema_before_runs("a1_record_upgrade").await else {
        return;
    };
    let seed = seed(&pool, &root).await;
    let completed = insert_run(&pool, &seed, "completed", None, "Fix the login bug\nDetails").await;
    let failed = insert_run(&pool, &seed, "failed", Some("boom"), "Try again").await;
    let canceled = insert_run(&pool, &seed, "canceled", None, "Never mind").await;
    let stopped = insert_run(
        &pool,
        &seed,
        "interrupted",
        Some(runs::state::STOPPED_BY_REQUESTER),
        "Stopped by a person",
    )
    .await;
    let lost = insert_run(
        &pool,
        &seed,
        "interrupted",
        Some("The server restarted."),
        "Lost in a restart",
    )
    .await;
    let working = insert_run(&pool, &seed, "working", None, "Still going").await;
    let mut events = Vec::new();
    for (kind, payload) in [
        ("run_state_changed", serde_json::json!({"from": "queued", "to": "working"})),
        ("command_completed", serde_json::json!({"command": "cargo test", "exitCode": 0})),
        ("agent_note", serde_json::json!({"text": "Looking at the tests."})),
        ("run_completed", serde_json::json!({})),
    ] {
        events.push(insert_event(&pool, completed, kind, payload).await);
    }
    sqlx::query(
        r#"
        INSERT INTO agent_run_usage (
            agent_run_id, model, organization_id, project_id, adapter_kind, usage_date,
            input_tokens, output_tokens, cached_input_tokens, estimated_cost, currency,
            reported_cost_usd
        )
        VALUES ($1, 'model-a', $2, $3, 'test_adapter', current_date, 1000, 200, 50, 0.5,
                'USD', 0.4)
        "#,
    )
    .bind(completed)
    .bind(seed.organization_id)
    .bind(seed.project_id)
    .execute(&pool)
    .await
    .expect("insert usage");

    full.run(&pool).await.expect("migrate the run record");

    for (run, expected, finished) in [
        (completed, "done", true),
        (failed, "failed", true),
        (canceled, "cancelled", true),
        (stopped, "cancelled", true),
        (lost, "interrupted", true),
        (working, "working", false),
    ] {
        let (state, _, has_finished) = run_state(&pool, run).await;
        assert_eq!(state, expected, "run {run}");
        assert_eq!(has_finished, finished, "run {run}");
    }
    let (title, account, workspace, space, cause) =
        sqlx::query_as::<_, (String, Uuid, Uuid, Option<Uuid>, String)>(
            "SELECT title, account_id, workspace_id, space_id, cause_kind FROM runs WHERE id = $1",
        )
        .bind(completed)
        .fetch_one(&pool)
        .await
        .expect("completed run");
    assert_eq!(title, "Fix the login bug");
    assert_eq!(account, seed.worker_actor_id, "the agent doing the work");
    assert_eq!(workspace, seed.organization_id);
    assert_eq!(space, Some(seed.project_id));
    assert_eq!(cause, "thread");

    let usage = sqlx::query_as::<_, (i64, i64, String, String, Option<f64>, Option<String>)>(
        r#"SELECT input_tokens, output_tokens, attribution, confidence, reported_cost,
                  reported_currency
           FROM run_usage WHERE run_id = $1"#,
    )
    .bind(completed)
    .fetch_one(&pool)
    .await
    .expect("usage copied");
    assert_eq!(
        usage,
        (
            1000,
            200,
            "reported".to_owned(),
            "exact".to_owned(),
            Some(0.4),
            Some("USD".to_owned())
        )
    );

    // The activities wait for the backfill job, which is queued.
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind = $1 AND state = 'queued'",
    )
    .bind(runs::BACKFILL_ACTIVITIES_KIND)
    .fetch_one(&pool)
    .await
    .expect("backfill job");
    assert_eq!(queued, 1);
    let last = runs::backfill_activities(&pool, None)
        .await
        .expect("backfill");
    assert_eq!(last, events.iter().max().copied());
    runs::backfill_activities(&pool, None)
        .await
        .expect("backfill again");
    let copied = sqlx::query_as::<_, (Uuid, i64, String, serde_json::Value)>(
        r#"SELECT legacy_event_id, seq, kind, payload FROM run_activities
           WHERE run_id = $1 ORDER BY seq"#,
    )
    .bind(completed)
    .fetch_all(&pool)
    .await
    .expect("activities");
    assert_eq!(copied.len(), events.len(), "each event copied once");
    let kinds = copied
        .iter()
        .map(|(_, _, kind, _)| kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        ["run_started", "tool_call", "text_message", "run_finished"]
    );
    assert_eq!(copied[3].3["outcome"], "done");
    assert_eq!(copied[1].3["legacyKind"], "command_completed");
    assert_eq!(
        copied.iter().map(|row| row.1).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
}

#[tokio::test]
async fn the_mirror_keeps_runs_equal_to_built_in_runs() {
    let Some((pool, full, root)) = schema_before_runs("a1_record_mirror").await else {
        return;
    };
    full.run(&pool).await.expect("migrate");
    let seed = seed(&pool, &root).await;

    // The SQL mapping and the Rust one agree, state by state.
    for legacy in Legacy::ALL {
        for reason in [None, Some(runs::state::STOPPED_BY_REQUESTER)] {
            let sql = sqlx::query_scalar::<_, String>("SELECT runs_state_from_legacy($1, $2)")
                .bind(legacy.to_string())
                .bind(reason)
                .fetch_one(&pool)
                .await
                .expect("map");
            assert_eq!(
                sql,
                RunState::from_legacy(legacy, reason).as_str(),
                "{legacy} {reason:?}"
            );
        }
    }

    let run = insert_run(&pool, &seed, "queued", None, "Mirror me").await;
    assert_eq!(run_state(&pool, run).await.0, "queued");
    let version = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>("SELECT version FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .expect("version")
    };
    let first = version(pool.clone()).await;
    for (legacy, expected) in [
        ("starting", "starting"),
        ("working", "working"),
        ("blocked", "waiting"),
        ("recovering", "working"),
    ] {
        sqlx::query("UPDATE agent_runs SET state = $2, updated_at = now() WHERE id = $1")
            .bind(run)
            .bind(legacy)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("move to {legacy}: {error}"));
        assert_eq!(run_state(&pool, run).await.0, expected, "after {legacy}");
    }
    assert_eq!(
        run_state(&pool, run).await.1.as_deref(),
        Some("Recovering after a restart")
    );
    assert!(version(pool.clone()).await > first, "each change bumps the version");

    let event = insert_event(
        &pool,
        run,
        "approval_requested",
        serde_json::json!({"summary": "Run the migration?"}),
    )
    .await;
    let (kind, seq) = sqlx::query_as::<_, (String, i64)>(
        "SELECT kind, seq FROM run_activities WHERE legacy_event_id = $1",
    )
    .bind(event)
    .fetch_one(&pool)
    .await
    .expect("mirrored activity");
    assert_eq!((kind.as_str(), seq), ("interrupt", 1));

    sqlx::query(
        "UPDATE agent_runs SET state = 'interrupted', failure_reason = $2, finished_at = now() WHERE id = $1",
    )
    .bind(run)
    .bind(runs::state::STOPPED_BY_REQUESTER)
    .execute(&pool)
    .await
    .expect("stop");
    let (state, _, finished) = run_state(&pool, run).await;
    assert_eq!((state.as_str(), finished), ("cancelled", true));

    sqlx::query(
        r#"
        INSERT INTO agent_run_usage (
            agent_run_id, model, organization_id, project_id, adapter_kind, usage_date,
            input_tokens, output_tokens
        )
        VALUES ($1, 'model-b', $2, $3, 'test_adapter', current_date, 10, 2)
        "#,
    )
    .bind(run)
    .bind(seed.organization_id)
    .bind(seed.project_id)
    .execute(&pool)
    .await
    .expect("usage");
    sqlx::query(
        "UPDATE agent_run_usage SET input_tokens = 30 WHERE agent_run_id = $1 AND model = 'model-b'",
    )
    .bind(run)
    .execute(&pool)
    .await
    .expect("more usage");
    let tokens = sqlx::query_scalar::<_, i64>(
        "SELECT input_tokens FROM run_usage WHERE run_id = $1 AND model = 'model-b'",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .expect("mirrored usage");
    assert_eq!(tokens, 30);
}

/// A1.2: a run in a private thread isn't listed or found for a member of
/// the project who isn't in the conversation; archiving still waits for it.
#[tokio::test]
async fn a_run_in_a_private_thread_stays_private() {
    let Some((pool, full, root)) = schema_before_runs("a1_record_private").await else {
        return;
    };
    full.run(&pool).await.expect("migrate");
    let seed = seed(&pool, &root).await;
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1")
        .bind(seed.channel_id)
        .execute(&pool)
        .await
        .expect("make the channel private");
    sqlx::query(
        r#"
        INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
        VALUES ($1, $2, $3, $3)
        "#,
    )
    .bind(seed.project_id)
    .bind(seed.channel_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("add the person");
    let run = insert_run(&pool, &seed, "working", None, "Private work").await;

    let member = seed.human_actor_id;
    let outsider = seed.collaborator_actor_id;
    assert_eq!(
        runs::store::space_of_visible(&pool, member, run).await.expect("member"),
        Some(seed.project_id)
    );
    assert_eq!(
        runs::store::space_of_visible(&pool, outsider, run).await.expect("outsider"),
        None
    );
    let listed = |viewer: Uuid| {
        let pool = pool.clone();
        async move {
            runs::store::visible_in_space(&pool, viewer, seed.project_id, 50)
                .await
                .expect("list")
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(listed(member).await, vec![run]);
    assert!(listed(outsider).await.is_empty(), "not listed for a non-member");
    assert!(
        runs::store::active_under_space(&pool, seed.project_id)
            .await
            .expect("active"),
        "archiving the project still waits for it"
    );
}

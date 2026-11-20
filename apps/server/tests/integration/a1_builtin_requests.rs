//! A1.14, A1.16: a run on the built-in host asks through requests, like a
//! run on any host: an `interrupt` it reports, answered through the request
//! operations, heard by the host as an `answer` command. The silent
//! failures: a denied approval that leaves the run waiting, an answer the
//! host never hears, and an approval that reaches nobody's inbox.

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    config::AppConfig,
    hosts::builtin::requests::{Outcome, outcome_of},
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    agent_interface::send,
    mcp_code::{run, seeded, thread},
};

async fn blocked_run(
    pool: &PgPool,
    seed: &agent_workspace_server::seed::DevSeed,
    name: &str,
) -> Uuid {
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now(), trusted = true WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(pool)
    .await
    .expect("repository ready");
    let asking = thread(pool, seed, seed.channel_id, name).await;
    let (run_id, _) = run(pool, seed, asking, name).await;
    for next in ["starting", "working", "blocked"] {
        sqlx::query("UPDATE agent_runs SET state = $2 WHERE id = $1")
            .bind(run_id)
            .bind(next)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("run to {next}: {error}"));
    }
    run_id
}

async fn run_state(pool: &PgPool, run_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("run state")
}

async fn record_state(pool: &PgPool, run_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT state FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("run record state")
}

/// The answer commands the built-in host got for the run.
async fn answers(pool: &PgPool, run_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM host_commands WHERE run_id = $1 AND kind = 'answer'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .expect("answer commands")
}

/// The server as the built-in host's driver sees it.
fn app_state(pool: &PgPool) -> AppState {
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database");
    let config = AppConfig::from_values(&BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url,
    )]))
    .expect("configuration");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    }
}

/// Give the run its start, and let the host take it.
async fn started(state: &AppState, run_id: Uuid) {
    crate::support::builtin_run_started(&state.pool, run_id).await;
    agent_workspace_server::scheduler::drain_now(state)
        .await
        .expect("the host takes the start");
}

#[tokio::test]
async fn an_approval_reaches_the_owner_and_the_host_hears_the_answer() {
    let Some((setup, seed, _temporary)) = seeded("a1_builtin_approve").await else {
        return;
    };
    let pool = &setup.pool;
    let run_id = blocked_run(pool, &seed, "builtin-approve").await;
    let approval =
        crate::support::requests::approval(pool, run_id, "builtin-approve", "Run npm install?")
            .await;
    // Raised twice (a restart re-asks), it is still one request.
    let again =
        crate::support::requests::approval(pool, run_id, "builtin-approve", "Run npm install?")
            .await;
    assert_eq!(again, approval);

    // The space's owner has it in their inbox.
    let cards = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE subject_kind = 'request' AND subject_id = $1 AND recipient_actor_id = $2",
    )
    .bind(approval)
    .bind(seed.human_actor_id)
    .fetch_one(pool)
    .await
    .expect("cards");
    assert_eq!(cards, 1);
    assert_eq!(record_state(pool, run_id).await, "waiting");

    let (status, answered) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("builtin-approve"),
            json!({"requestId": approval, "optionId": "approve"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(
        record_state(pool, run_id).await,
        "working",
        "an approved run goes on"
    );
    assert_eq!(answers(pool, run_id).await, 1, "the host hears the answer");
    let resolved = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM run_activities WHERE run_id = $1 AND kind = 'interrupt_resolved'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .expect("steps");
    assert_eq!(resolved, 1, "the run's steps say it was approved");
}

#[tokio::test]
async fn a_denied_approval_stops_the_run_and_its_other_requests() {
    let Some((setup, seed, _temporary)) = seeded("a1_builtin_deny").await else {
        return;
    };
    let pool = &setup.pool;
    let run_id = blocked_run(pool, &seed, "builtin-deny").await;
    let state = app_state(pool);
    started(&state, run_id).await;
    let approval =
        crate::support::requests::approval(pool, run_id, "builtin-deny", "Force-push main?").await;
    // A harness may name an option by its whole label, longer than a
    // request's option ids go: it gets a number and keeps its label.
    let long = "release/".repeat(12);
    let question = crate::support::requests::question(
        pool,
        run_id,
        "builtin-deny-question",
        "Which branch?",
        &[("main", "main"), (&long, &long)],
    )
    .await;
    let options =
        sqlx::query_scalar::<_, serde_json::Value>("SELECT options FROM requests WHERE id = $1")
            .bind(question)
            .fetch_one(pool)
            .await
            .expect("question options");
    assert_eq!(options[1]["id"], "option-2", "{options}");
    assert_eq!(options[1]["label"], long.as_str(), "{options}");

    let (status, denied) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/request_respond",
            Some("builtin-deny"),
            json!({"requestId": approval, "optionId": "deny"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{denied}");
    // The host hears the answer and stops the run, with no turn waiting.
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("the host hears the answer");
    assert_eq!(
        run_state(pool, run_id).await,
        "interrupted",
        "a denied run stops"
    );
    assert_eq!(record_state(pool, run_id).await, "cancelled");
    // What else the run asked is withdrawn.
    let left = sqlx::query_scalar::<_, String>("SELECT status FROM requests WHERE id = $1")
        .bind(question)
        .fetch_one(pool)
        .await
        .expect("question");
    assert_eq!(left, "cancelled");
}

#[test]
fn the_harness_hears_what_was_answered() {
    let approval = |picked: &str| {
        json!({"kind": "approval", "status": "answered",
               "options": [{"id": "approve", "label": "Allow", "style": "approve"},
                           {"id": "deny", "label": "Deny", "style": "deny"}],
               "answer": {"optionId": picked}})
    };
    assert_eq!(outcome_of(&approval("approve")), Outcome::Approve);
    assert_eq!(outcome_of(&approval("deny")), Outcome::Deny);
    assert_eq!(
        outcome_of(&json!({"kind": "approval", "status": "open"})),
        Outcome::Waiting
    );
    assert_eq!(
        outcome_of(&json!({"kind": "question", "status": "answered",
                           "answer": {"optionId": "main", "label": "main", "text": "and rebase"}})),
        Outcome::Text("main\n\nand rebase".to_owned())
    );
    assert_eq!(
        outcome_of(&json!({"kind": "question", "status": "expired"})),
        Outcome::Deny
    );
}

/// A1.17: a built-in run's tokens go through the host protocol like any
/// host's, so they count against budgets, and the workspace's catalog
/// prices every report, even one with its own cost. A retry adds nothing.
#[tokio::test]
async fn built_in_tokens_are_reported_priced_and_counted_once() {
    let Some((setup, seed, _temporary)) = seeded("a1_builtin_tokens").await else {
        return;
    };
    let pool = &setup.pool;
    let run_id = blocked_run(pool, &seed, "builtin-tokens").await;
    sqlx::query(
        r#"
        INSERT INTO harness_models (id, organization_id, adapter_kind, model_id, provider_data)
        SELECT gen_random_uuid(), $1, account.adapter_kind, 'priced-model',
               '{"id": "priced-model", "displayName": "Priced",
                 "prices": {"currency": "USD", "inputPerMillion": 2.0, "outputPerMillion": 10.0}}'::jsonb
        FROM execution_profiles profile
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE profile.id = $2
        "#,
    )
    .bind(seed.organization_id)
    .bind(seed.execution_profile_id)
    .execute(pool)
    .await
    .expect("a priced model");
    let state = app_state(pool);
    started(&state, run_id).await;
    let reports = agent_workspace_server::hosts::builtin::report::open(&state, run_id)
        .await
        .expect("the run's reports");
    let tokens = agent_workspace_builtin_host::runtime::report::Usage {
        model: "priced-model",
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        cached_input_tokens: 0,
        cost_usd: None,
    };
    reports.usage("turn-1", &tokens).await.expect("reported");
    // The same report again (a retry) adds nothing.
    reports
        .usage("turn-1", &tokens)
        .await
        .expect("reported again");
    // A later report that brings its own cost is priced too, so the
    // estimate the totals show covers every token of the model.
    let with_cost = agent_workspace_builtin_host::runtime::report::Usage {
        cost_usd: Some(0.5),
        ..tokens
    };
    reports
        .usage("turn-2", &with_cost)
        .await
        .expect("reported with a cost");

    let (input, output, estimated, currency) =
        sqlx::query_as::<_, (i64, i64, Option<f64>, Option<String>)>(
            "SELECT input_tokens, output_tokens, estimated_cost, currency FROM run_usage WHERE run_id = $1 AND model = 'priced-model'",
        )
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("the run's usage");
    assert_eq!((input, output), (2_000_000, 200_000));
    // Twice 1M input at 2.00 and 100k output at 10.00 a million.
    assert!(
        estimated.is_some_and(|cost| (cost - 6.0).abs() < 1e-9),
        "{estimated:?}"
    );
    assert_eq!(currency.as_deref(), Some("USD"));
    let counted = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT sum(quantity)::bigint FROM usage_events WHERE workspace_id = $1 AND meter = 'reported_tokens'",
    )
    .bind(seed.organization_id)
    .fetch_one(pool)
    .await
    .expect("the budget meter");
    assert_eq!(counted, Some(2_200_000));
}

/// A1.17: a run's reply streams live only while its agent can read the
/// conversation. One taken out of it (the channel made private without
/// it) streams nothing more there, though its token still holds.
#[tokio::test]
async fn a_reply_streams_only_while_its_agent_can_read_the_conversation() {
    let Some((setup, seed, _temporary)) = seeded("a1_builtin_stream_gate").await else {
        return;
    };
    let pool = &setup.pool;
    let run_id = blocked_run(pool, &seed, "builtin-stream").await;
    let state = app_state(pool);
    started(&state, run_id).await;
    let reports = agent_workspace_server::hosts::builtin::report::open(&state, run_id)
        .await
        .expect("the run's reports");
    reports
        .stream(Uuid::now_v7(), "Working on it".to_owned())
        .await
        .expect("shown while the agent reads the channel");
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1")
        .bind(seed.channel_id)
        .execute(pool)
        .await
        .expect("the channel made private");
    assert!(
        reports
            .stream(Uuid::now_v7(), "Still here".to_owned())
            .await
            .is_err(),
        "a run whose agent can't read its conversation streams nothing there"
    );
}

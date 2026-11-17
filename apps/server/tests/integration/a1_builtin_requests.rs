//! A1.14: a run on the built-in host asks through requests, like a run on
//! any host. The silent failures: a denied approval that leaves the run
//! waiting, an answer the harness is handed twice or never, and an approval
//! that reaches nobody's inbox.

use agent_workspace_server::hosts::builtin::requests::{Outcome, claim_delivery, outcome};
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

#[tokio::test]
async fn an_approval_reaches_the_owner_and_the_harness_hears_the_answer_once() {
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
    assert_eq!(
        outcome(pool, approval).await.expect("outcome"),
        Outcome::Waiting
    );
    assert!(
        !claim_delivery(pool, approval).await.expect("claim"),
        "nothing to hand over yet"
    );

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
        run_state(pool, run_id).await,
        "working",
        "an approved run goes on"
    );
    assert_eq!(
        outcome(pool, approval).await.expect("outcome"),
        Outcome::Approve
    );
    assert!(claim_delivery(pool, approval).await.expect("claim"));
    assert!(
        !claim_delivery(pool, approval).await.expect("claim again"),
        "the harness is handed an answer once"
    );
    let resolved = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM run_activities WHERE run_id = $1 AND payload->>'step' = 'approval_resolved'",
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
    let approval =
        crate::support::requests::approval(pool, run_id, "builtin-deny", "Force-push main?").await;
    let question = crate::support::requests::question(
        pool,
        run_id,
        "builtin-deny-question",
        "Which branch?",
        &[("main", "main")],
    )
    .await;

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
    assert_eq!(
        run_state(pool, run_id).await,
        "interrupted",
        "a denied run stops"
    );
    // The harness is told no, and what else the run asked is withdrawn.
    assert_eq!(
        outcome(pool, approval).await.expect("outcome"),
        Outcome::Interrupted
    );
    let left = sqlx::query_scalar::<_, String>("SELECT status FROM requests WHERE id = $1")
        .bind(question)
        .fetch_one(pool)
        .await
        .expect("question");
    assert_eq!(left, "cancelled");
}

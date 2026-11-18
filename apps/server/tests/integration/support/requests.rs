//! What a run on the built-in host asks, raised the way the host raises it:
//! the `interrupt` activity it reports (`hosts::builtin::requests::interrupt`),
//! for tests that need one waiting.

use agent_workspace_server::hosts::builtin::requests::{Ask, interrupt};
use agent_workspace_server::runs::{activities::NewActivity, ops::Reporter};
use sqlx::PgPool;
use uuid::Uuid;

/// An approval the run waits on, with Approve (labelled `Allow`) and Deny.
pub(crate) async fn approval(pool: &PgPool, run_id: Uuid, durable_id: &str, summary: &str) -> Uuid {
    ask(
        pool,
        run_id,
        durable_id,
        Ask::Approval,
        summary,
        &[("approve", "Allow"), ("deny", "Deny")],
    )
    .await
}

/// A question the run waits on, with these options and room for words.
pub(crate) async fn question(
    pool: &PgPool,
    run_id: Uuid,
    durable_id: &str,
    summary: &str,
    options: &[(&str, &str)],
) -> Uuid {
    ask(pool, run_id, durable_id, Ask::Question, summary, options).await
}

async fn ask(
    pool: &PgPool,
    run_id: Uuid,
    durable_id: &str,
    ask: Ask,
    summary: &str,
    options: &[(&str, &str)],
) -> Uuid {
    let options = options
        .iter()
        .map(|(id, label)| ((*id).to_owned(), (*label).to_owned()))
        .collect::<Vec<_>>();
    // A run a test moved by hand on its `agent_runs` row: its record says
    // the same, as the host would have reported it.
    let detail = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("the run's detail");
    super::builtin_run_moved(pool, run_id, &detail).await;
    let activity = interrupt(durable_id, ask, summary, &options);
    let account_id = sqlx::query_scalar::<_, Uuid>("SELECT account_id FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("the run's agent");
    let mut transaction = agent_workspace_server::db::scoped_tx(pool)
        .await
        .expect("transaction");
    let appended = agent_workspace_server::runs::ops::append(
        &mut transaction,
        Reporter::Account { account_id },
        run_id,
        vec![NewActivity {
            client_id: activity["id"].as_str().expect("id").to_owned(),
            kind: "interrupt".to_owned(),
            occurred_at: chrono::Utc::now(),
            step_ref: None,
            parent_ref: None,
            summary: activity["summary"].as_str().map(str::to_owned),
            payload: activity["payload"].clone(),
        }],
    )
    .await
    .unwrap_or_else(|error| panic!("raise the request: {error:?}"));
    transaction.commit().await.expect("commit");
    appended.requests[0].request_id
}

//! What a run on the built-in host asks, raised the way the host raises it
//! (`hosts::builtin::requests::raise`), for tests that need one waiting.

use agent_workspace_server::hosts::builtin::requests::{Ask, raise};
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
    let mut transaction = agent_workspace_server::db::scoped_tx(pool)
        .await
        .expect("transaction");
    let raised = raise(&mut transaction, run_id, durable_id, ask, summary, &options)
        .await
        .expect("raise the request");
    transaction.commit().await.expect("commit");
    raised.request_id
}

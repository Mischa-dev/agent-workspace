//! A2.26: runs a host started on its own (a schedule or trigger there, not
//! a start from Workspace), adopted onto the run record once.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    ops::{self, Cause, Opened, Reporter, RunOpError, RunOpen},
    state::RunState,
};
use crate::error::AppError;

/// The run already adopted for a host's own run, if any.
pub async fn by_host_ref(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
    host_run_ref: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM runs WHERE host_id = $1 AND host_run_ref = $2",
    )
    .bind(host_id)
    .bind(host_run_ref)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Opens the record of a run the host started for one of its agents, in
/// the space given, working from now, and marks it as the host's under its
/// own reference. The agent opens it as itself, under the same checks as
/// any run it reports; a second call with the same reference finds this
/// one (the host's reference is unique per host).
pub async fn adopt(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
    account_id: Uuid,
    space_id: Uuid,
    host_run_ref: &str,
    title: &str,
) -> Result<Opened, RunOpError> {
    let opened = ops::open(
        transaction,
        Reporter::Account { account_id },
        RunOpen {
            space_id,
            client_run_id: Some(format!("host-{host_id}-{host_run_ref}")),
            title: title.to_owned(),
            cause: Cause {
                kind: "host_trigger".to_owned(),
                conversation_id: None,
                work_item_id: None,
                message_id: None,
                reference: Some(host_run_ref.to_owned()),
            },
            parent_run_id: None,
            report_interval_seconds: None,
            settings: None,
            initial: RunState::Working,
            requested_by: None,
            via_app_id: None,
            launch_id: None,
        },
    )
    .await?;
    sqlx::query(
        "UPDATE runs SET host_id = $2, host_run_ref = $3, updated_at = now() \
         WHERE id = $1 AND (host_id IS NULL OR host_id = $2)",
    )
    .bind(opened.run.id)
    .bind(host_id)
    .bind(host_run_ref)
    .execute(&mut **transaction)
    .await
    .map_err(|error| RunOpError::App(error.into()))?;
    Ok(opened)
}

//! A run's token (A1.12 on K1.24's narrow tokens). When a host fetches a
//! `start` command it gets a short-lived token for the run's agent, held to
//! the run's space, so it can report on the run (and act for the agent
//! there) without a credential of its own for every agent. The token is
//! `access::tokens`'s: shown once, kept as a digest, and it stops working
//! when it expires, when the run ends, when the agent is unbound from the
//! host or the host is unlinked, and when the agent leaves the space.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    access::tokens::{self, RunBinding, ScopedTokenRequest},
    error::AppError,
};

/// How long a run's token works.
pub const LIFETIME_MINUTES: i64 = 60;

/// A token just issued for a run: the token is in no row.
#[derive(Clone, Debug)]
pub struct Minted {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    /// What it reaches, in words a host can log.
    pub scope: String,
}

/// Issue a token for the run's agent, for the command a host fetched. None
/// when the run has no space, or its agent no longer holds that space.
pub async fn mint(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    host_id: Uuid,
    command_id: Uuid,
) -> Result<Option<Minted>, AppError> {
    let run = sqlx::query_as::<
        _,
        (
            Uuid,
            Option<Uuid>,
            Uuid,
            Option<Uuid>,
            Option<Uuid>,
            Option<Uuid>,
        ),
    >(
        "SELECT workspace_id, space_id, account_id, work_item_id, conversation_id, requested_by_account_id \
         FROM runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((workspace_id, Some(space_id), account_id, task, conversation, requested_by)) = run
    else {
        return Ok(None);
    };
    let issued = tokens::issue_scoped(
        transaction,
        ScopedTokenRequest {
            account_id,
            workspace_id,
            home_space: space_id,
            permissions: None,
            spaces: Some(vec![space_id]),
            minutes: LIFETIME_MINUTES,
            requested_by,
            run: Some(RunBinding {
                run_id,
                host_id,
                command_id,
            }),
            credential_id: None,
            scopes: None,
        },
    )
    .await;
    let issued = match issued {
        Ok(issued) => issued,
        // The agent no longer holds the run's space: nothing to hand over.
        Err(AppError::Refused { .. } | AppError::Forbidden) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut scope = vec![format!("run:{run_id}"), format!("space:{space_id}")];
    if let Some(task) = task {
        scope.push(format!("task:{task}"));
    }
    if let Some(conversation) = conversation {
        scope.push(format!("conversation:{conversation}"));
    }
    Ok(Some(Minted {
        token: issued.token,
        expires_at: issued.expires_at,
        scope: scope.join(" "),
    }))
}

/// Whether a run is still going, for its token (K1.24).
pub async fn run_open(pool: &sqlx::PgPool, run_id: Uuid) -> Result<bool, AppError> {
    let query = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM runs WHERE id = $1 AND finished_at IS NULL)",
    )
    .bind(run_id);
    Ok(crate::db::scoped_one(pool, async |connection| query.fetch_one(connection).await).await?)
}

/// The run's tokens stop working: it ended.
pub async fn revoke_for_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<u64, AppError> {
    tokens::revoke_for_run(transaction, run_id).await
}

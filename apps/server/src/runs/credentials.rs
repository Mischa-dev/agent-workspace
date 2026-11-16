//! A1.12: the interim run credential (stopgap until K1.24's `issue_scoped`
//! and `revoke_for_run`). When a host fetches a `start` command it gets a
//! short-lived token for the run's agent in the run's space, so it can
//! report on the run (and act for the agent there) without a credential of
//! its own for every agent. The token is shown once, in that response; only
//! its digest is kept. It stops working when it expires, when the run ends,
//! when the agent is unbound from the host or the host is unlinked, and when
//! the agent leaves the space.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::error::AppError;

pub const TOKEN_PREFIX: &str = "awr_";
const TOKEN_PREFIX_LENGTH: usize = 12;
/// How long a run credential works.
pub const LIFETIME_MINUTES: i64 = 60;

/// A credential just minted: the token is in no row.
#[derive(Clone, Debug)]
pub struct Minted {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    /// What it reaches, in words a host can log.
    pub scope: String,
}

/// Mint a credential for the run's agent, for the command a host fetched.
pub async fn mint(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    host_id: Uuid,
    command_id: Uuid,
) -> Result<Option<Minted>, AppError> {
    let run = sqlx::query_as::<_, (Uuid, Option<Uuid>, Uuid, Option<Uuid>, Option<Uuid>)>(
        "SELECT workspace_id, space_id, account_id, work_item_id, conversation_id FROM runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((workspace_id, Some(space_id), account_id, task, conversation)) = run else {
        return Ok(None);
    };
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let expires_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        r#"
        INSERT INTO run_credentials (
            id, workspace_id, space_id, run_id, account_id, host_id, command_id,
            token_prefix, token_digest, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now() + make_interval(mins => $10::int))
        RETURNING expires_at
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(space_id)
    .bind(run_id)
    .bind(account_id)
    .bind(host_id)
    .bind(command_id)
    .bind(&token[..TOKEN_PREFIX_LENGTH])
    .bind(digest.as_slice())
    .bind(LIFETIME_MINUTES as i32)
    .fetch_one(&mut **transaction)
    .await?;
    let mut scope = vec![format!("run:{run_id}"), format!("space:{space_id}")];
    if let Some(task) = task {
        scope.push(format!("task:{task}"));
    }
    if let Some(conversation) = conversation {
        scope.push(format!("conversation:{conversation}"));
    }
    Ok(Some(Minted {
        token,
        expires_at,
        scope: scope.join(" "),
    }))
}

/// The run's credentials stop working: it ended.
pub async fn revoke_for_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<u64, AppError> {
    Ok(sqlx::query(
        "UPDATE run_credentials SET revoked_at = now() WHERE run_id = $1 AND revoked_at IS NULL",
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}

/// Who a run credential speaks for.
#[derive(Clone, Debug, FromRow)]
pub struct Holder {
    pub credential_id: Uuid,
    pub account_id: Uuid,
    pub account_kind: String,
    pub space_id: Uuid,
    pub host_name: String,
    /// The agent's own connection in the workspace, when it has one: the
    /// credential reaches what that connection reaches, no more.
    pub connection_id: Option<Uuid>,
    token_digest: Vec<u8>,
}

/// The agent and space a run credential is for, while it still works.
pub async fn resolve(pool: &PgPool, token: &str) -> Result<Option<Holder>, AppError> {
    if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
        return Ok(None);
    }
    let Some(prefix) = token.get(..TOKEN_PREFIX_LENGTH) else {
        return Ok(None);
    };
    let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let holder = sqlx::query_as::<_, Holder>(
        r#"
        SELECT credential.id AS credential_id, credential.account_id,
               account.kind AS account_kind, credential.space_id,
               host.name AS host_name, credential.token_digest,
               (SELECT connection.id FROM external_agent_connections connection
                JOIN projects home ON home.id = connection.project_id
                WHERE connection.actor_id = credential.account_id AND connection.status = 'active'
                  AND home.organization_id = credential.workspace_id
                ORDER BY connection.id LIMIT 1) AS connection_id
        FROM run_credentials credential
        JOIN runs run ON run.id = credential.run_id
        JOIN actors account ON account.id = credential.account_id
        JOIN hosts host ON host.id = credential.host_id
        JOIN host_agents binding
          ON binding.host_id = credential.host_id AND binding.account_id = credential.account_id
        JOIN projects space ON space.id = credential.space_id
        JOIN project_memberships membership
          ON membership.project_id = credential.space_id AND membership.actor_id = credential.account_id
        WHERE credential.token_prefix = $1
          AND credential.revoked_at IS NULL AND credential.expires_at > now()
          AND run.finished_at IS NULL
          AND account.status = 'active'
          AND host.status <> 'revoked' AND binding.status <> 'gone'
          AND space.archived_at IS NULL AND membership.enabled
        "#,
    )
    .bind(prefix);
    let holder = crate::db::scoped_one(pool, async |connection| holder.fetch_optional(connection).await)
        .await?;
    Ok(holder.filter(|holder| {
        holder.token_digest.len() == candidate.len()
            && bool::from(holder.token_digest.as_slice().ct_eq(&candidate))
    }))
}
